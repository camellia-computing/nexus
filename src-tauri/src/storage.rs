use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use camellia_nexus_core::{
    CamelliaNexusError, ConfigStore, ConfigurationFormat, ConfigurationState,
    CoreBinaryFingerprint, CreateAssets, ErrorCode, ExecutableMetadata, FinalEditorSession,
    InvalidProgram, LoadReport, LogChunk, LogStream, MAX_CONFIG_BYTES, ProgramId, ProgramSpec,
    ProgramStore, RawConfig, Result, StagedConfig, StagedPackage, StoredProgram,
    config_service::hash_bytes,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const PACKAGE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const PACKAGE_MAX_ENTRIES: usize = 4096;
const PROGRAM_SPEC_MAX_BYTES: u64 = 1024 * 1024;
const CONFIGURATION_STATE_MAX_BYTES: u64 = 32 * 1024 * 1024;
const CONFIGURATION_STATE_DIRECTORY: &str = "configuration";
const CONFIGURATION_STATE_FILE: &str = "state.json";
const CONFIGURATION_SIDECAR_DIRECTORY: &str = "sidecars";
const CONFIGURATION_LKG_JSON: &str = "last-known-good.json";
const CONFIGURATION_LKG_YAML: &str = "last-known-good.yaml";
const CONFIGURATION_APPLY_MARKER: &str = ".configuration-apply.json";
const CONFIGURATION_WORKSPACE_TRANSACTION_MARKER: &str =
    ".configuration-workspace-transaction.json";
const CONFIGURATION_WORKSPACE_STATE_BACKUP: &str = ".configuration-workspace-state.json.bak";
const PROGRAM_PACKAGE_TRANSACTION_MARKER: &str = ".program-package-transaction.json";
const PROGRAM_PACKAGE_SPEC_BACKUP: &str = ".program-package-program.json.bak";
const PROGRAM_PACKAGE_NEXT_SPEC: &str = ".program-package-program.json.next";
const PROGRAM_PACKAGE_STATE_BACKUP: &str = ".program-package-state.json.bak";
const PROGRAM_PACKAGE_NEXT_STATE: &str = ".program-package-state.json.next";
const CREATE_PENDING_MARKER: &[u8] = b"pending\n";
const CREATE_COMMITTED_MARKER: &[u8] = b"committed\n";
const DISCARDED_PACKAGE_PREFIX: &str = ".camellia-nexus-package-discard-";
const DIRECTORY_CLEANUP_QUEUE_CAPACITY: usize = 16;

struct DirectoryCleanupWorker {
    sender: Option<mpsc::SyncSender<PathBuf>>,
}

static DIRECTORY_CLEANUP_WORKER: OnceLock<DirectoryCleanupWorker> = OnceLock::new();

fn directory_cleanup_worker() -> &'static DirectoryCleanupWorker {
    DIRECTORY_CLEANUP_WORKER.get_or_init(|| {
        let (sender, receiver) =
            mpsc::sync_channel::<PathBuf>(DIRECTORY_CLEANUP_QUEUE_CAPACITY);
        let spawned = std::thread::Builder::new()
            .name("camellia-directory-cleanup".to_owned())
            .spawn(move || {
                while let Ok(path) = receiver.recv() {
                    if let Err(error) = fs::remove_dir_all(&path) {
                        tracing::warn!(path = %path.display(), %error, "could not remove discarded directory");
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the directory cleanup worker; cleanup will run inline");
            DirectoryCleanupWorker { sender: None }
        } else {
            DirectoryCleanupWorker {
                sender: Some(sender),
            }
        }
    })
}

fn enqueue_directory_cleanup(
    sender: &mpsc::SyncSender<PathBuf>,
    path: PathBuf,
) -> std::result::Result<(), PathBuf> {
    sender.try_send(path).map_err(|error| match error {
        mpsc::TrySendError::Full(path) | mpsc::TrySendError::Disconnected(path) => path,
    })
}

fn remove_directory_bounded(path: PathBuf) {
    let queued = directory_cleanup_worker()
        .sender
        .as_ref()
        .is_some_and(|sender| enqueue_directory_cleanup(sender, path.clone()).is_ok());
    if !queued {
        tracing::warn!(
            path = %path.display(),
            "directory cleanup is unavailable or saturated; leaving discarded directory for a later cleanup pass"
        );
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProgramPackageTransactionMarker {
    phase: ProgramPackageTransactionPhase,
    workspace: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum ProgramPackageTransactionPhase {
    Prepared,
    Committed,
    Restored,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigurationApplyMarker {
    state: ConfigurationState,
    phase: ConfigurationApplyPhase,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum ConfigurationApplyPhase {
    Prepared,
    Committed,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigurationWorkspaceTransactionMarker {
    previous_spec: ProgramSpec,
    next_spec: ProgramSpec,
    expected_state_revision: u64,
    previous_state_present: bool,
    state_committed: bool,
}

#[derive(Clone)]
enum EditorCommit {
    Rebase,
    Consume(u64),
    Replace(Option<u64>),
}

pub struct FileStore {
    root: Arc<PathBuf>,
    configuration_writes: Arc<tokio::sync::Mutex<()>>,
    #[cfg(test)]
    fail_configuration_write: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    fail_apply_commit: Arc<std::sync::atomic::AtomicU8>,
    #[cfg(test)]
    fail_package_commit: Arc<std::sync::atomic::AtomicU8>,
}

impl FileStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        let store = Self {
            root: Arc::new(root),
            configuration_writes: Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(test)]
            fail_configuration_write: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(test)]
            fail_apply_commit: Arc::new(std::sync::atomic::AtomicU8::new(0)),
            #[cfg(test)]
            fail_package_commit: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        };
        fs::create_dir_all(store.programs_root().join(".trash"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(store.root.as_ref(), fs::Permissions::from_mode(0o700))?;
        }
        clean_atomic_temps(store.root.as_ref());
        store.clean_trash();
        Ok(store)
    }

    fn programs_root(&self) -> PathBuf {
        self.root.join("programs")
    }

    #[cfg(all(test, feature = "desktop", unix))]
    pub(crate) fn fail_next_configuration_write(&self) {
        self.fail_configuration_write
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(all(test, feature = "desktop", unix))]
    pub(crate) fn fail_apply_commit(&self, committed: bool) {
        self.fail_apply_commit.store(
            if committed { 2 } else { 1 },
            std::sync::atomic::Ordering::SeqCst,
        );
    }

    #[cfg(test)]
    pub(crate) fn fail_package_commit_at(&self, stage: u8) {
        self.fail_package_commit
            .store(stage, std::sync::atomic::Ordering::SeqCst);
    }

    fn program_root(&self, id: &ProgramId) -> PathBuf {
        self.programs_root().join(id.as_str())
    }

    fn config_path(&self, spec: &ProgramSpec) -> Result<PathBuf> {
        let root = self.program_root(&spec.id);
        config_path_in_root(&root, spec)
    }

    fn configuration_state_path(&self, id: &ProgramId) -> PathBuf {
        self.program_root(id)
            .join(CONFIGURATION_STATE_DIRECTORY)
            .join(CONFIGURATION_STATE_FILE)
    }

    fn configuration_sidecar_path(&self, id: &ProgramId, hash: &str) -> PathBuf {
        self.program_root(id)
            .join(CONFIGURATION_STATE_DIRECTORY)
            .join(CONFIGURATION_SIDECAR_DIRECTORY)
            .join(format!("{hash}.source"))
    }

    pub async fn load_configuration_state(
        &self,
        id: &ProgramId,
    ) -> Result<Option<ConfigurationState>> {
        let path = self.configuration_state_path(id);
        blocking(move || {
            if !path.exists() {
                return Ok(None);
            }
            let bytes = read_with_overflow_byte(&path, CONFIGURATION_STATE_MAX_BYTES)?;
            if bytes.len() as u64 > CONFIGURATION_STATE_MAX_BYTES {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Configuration state exceeds the 32 MiB limit",
                ));
            }
            let mut state = decode_configuration_state(&bytes)?;
            for snapshot in state.source_snapshots.values_mut() {
                if snapshot.content.is_empty() {
                    if !valid_content_hash(&snapshot.content_hash) {
                        return Err(CamelliaNexusError::new(
                            ErrorCode::ConfigInvalid,
                            "Configuration source sidecar reference is invalid",
                        ));
                    }
                    let sidecar = path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(CONFIGURATION_SIDECAR_DIRECTORY)
                        .join(format!("{}.source", snapshot.content_hash));
                    let content = fs::read(&sidecar).map_err(|error| {
                        CamelliaNexusError::new(
                            ErrorCode::Storage,
                            "Configuration source sidecar is unavailable",
                        )
                        .with_details(error.to_string())
                    })?;
                    if content.len() > MAX_CONFIG_BYTES {
                        return Err(CamelliaNexusError::new(
                            ErrorCode::Storage,
                            "Configuration source sidecar exceeds the 4 MiB limit",
                        ));
                    }
                    if hash_bytes(&content) != snapshot.content_hash {
                        return Err(CamelliaNexusError::new(
                            ErrorCode::Storage,
                            "Configuration source sidecar hash does not match its snapshot",
                        ));
                    }
                    snapshot.content = String::from_utf8(content).map_err(|_| {
                        CamelliaNexusError::new(
                            ErrorCode::ConfigInvalid,
                            "Configuration source sidecar is not UTF-8",
                        )
                    })?;
                }
            }
            Ok(Some(state))
        })
        .await
    }

    pub async fn save_configuration_state(
        &self,
        id: &ProgramId,
        state: &ConfigurationState,
        expected_state_revision: Option<u64>,
    ) -> Result<()> {
        self.save_configuration_workspace(id, state, expected_state_revision, EditorCommit::Rebase)
            .await
    }

    /// Commit the candidate and consume exactly the draft that produced it.
    pub async fn save_configuration_candidate_state(
        &self,
        id: &ProgramId,
        state: &ConfigurationState,
        expected_state_revision: u64,
        expected_draft_revision: Option<u64>,
    ) -> Result<()> {
        self.save_configuration_workspace(
            id,
            state,
            Some(expected_state_revision),
            expected_draft_revision.map_or(EditorCommit::Rebase, EditorCommit::Consume),
        )
        .await
    }

    pub async fn save_configuration_conflict_state(
        &self,
        id: &ProgramId,
        state: &ConfigurationState,
        expected_state_revision: u64,
        expected_draft_revision: Option<u64>,
    ) -> Result<()> {
        self.save_configuration_workspace(
            id,
            state,
            Some(expected_state_revision),
            EditorCommit::Replace(expected_draft_revision),
        )
        .await
    }

    async fn save_configuration_workspace(
        &self,
        id: &ProgramId,
        state: &ConfigurationState,
        expected_state_revision: Option<u64>,
        editor_commit: EditorCommit,
    ) -> Result<()> {
        let _write = self.configuration_writes.lock().await;
        let path = self.configuration_state_path(id);
        let mut state = state.clone();
        #[cfg(test)]
        let fail_write = self.fail_configuration_write.clone();
        let sidecar_root = self
            .program_root(id)
            .join(CONFIGURATION_STATE_DIRECTORY)
            .join(CONFIGURATION_SIDECAR_DIRECTORY);
        blocking(move || {
            let current = read_configuration_state_file(&path)?;
            if let Some(expected) = expected_state_revision
                && current.as_ref().map(|state| state.state_revision) != Some(expected)
            {
                return Err(configuration_state_stale());
            }
            let current_draft = current
                .as_ref()
                .and_then(|state| state.editor_session.clone());
            match editor_commit {
                EditorCommit::Replace(expected) => {
                    if current_draft.as_ref().map(|draft| draft.draft_revision) != expected {
                        return Err(configuration_draft_stale());
                    }
                }
                EditorCommit::Consume(expected) => {
                    if current_draft.as_ref().map(|draft| draft.draft_revision) != Some(expected) {
                        return Err(configuration_draft_stale());
                    }
                    state.editor_session = None;
                }
                EditorCommit::Rebase => {
                    state.editor_session = current_draft;
                    if let Some(draft) = state.editor_session.as_mut() {
                        let before = draft.clone();
                        camellia_nexus_core::rebase_final_editor_session(
                            draft,
                            state.format,
                            &state.desired.content,
                            state.state_revision,
                            state.generation,
                        )?;
                        if *draft != before {
                            draft.draft_revision = before.draft_revision.saturating_add(1);
                        }
                    }
                }
            }
            fs::create_dir_all(&sidecar_root)?;
            compact_configuration_state(&sidecar_root, &mut state)?;
            #[cfg(test)]
            if fail_write.swap(false, std::sync::atomic::Ordering::SeqCst) {
                return Err(CamelliaNexusError::new(
                    ErrorCode::Storage,
                    "Injected workspace commit failure",
                ));
            }
            write_configuration_state_file(&path, &state)
        })
        .await
    }

    /// Stores an original observation or translated fragment by content hash.
    /// The state file only keeps the reference, which prevents a collection of
    /// 4 MiB sources from expanding the state beyond its durable limit.
    pub async fn save_configuration_sidecar(
        &self,
        id: &ProgramId,
        hash: &str,
        content: &[u8],
    ) -> Result<()> {
        if content.len() > MAX_CONFIG_BYTES {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Configuration sidecar exceeds the 4 MiB limit",
            ));
        }
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Configuration sidecar reference is invalid",
            ));
        }
        let path = self.configuration_sidecar_path(id, hash);
        let bytes = content.to_vec();
        blocking(move || write_bytes_atomic(&path, &bytes)).await
    }

    pub async fn load_configuration_sidecar(&self, id: &ProgramId, hash: &str) -> Result<Vec<u8>> {
        if !valid_content_hash(hash) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Configuration sidecar reference is invalid",
            ));
        }
        let path = self.configuration_sidecar_path(id, hash);
        let expected = hash.to_owned();
        blocking(move || {
            let bytes = read_with_overflow_byte(&path, MAX_CONFIG_BYTES as u64)?;
            if bytes.len() > MAX_CONFIG_BYTES {
                return Err(CamelliaNexusError::new(
                    ErrorCode::Storage,
                    "Configuration sidecar exceeds the 4 MiB limit",
                ));
            }
            if hash_bytes(&bytes) != expected {
                return Err(CamelliaNexusError::new(
                    ErrorCode::Storage,
                    "Configuration sidecar hash does not match its reference",
                ));
            }
            Ok(bytes)
        })
        .await
    }

    /// Starts a ProgramSpec/workspace transaction. The previous state is kept
    /// as a bounded atomic backup until `finish_configuration_workspace_update`.
    pub async fn begin_configuration_workspace_update(
        &self,
        id: &ProgramId,
        previous_spec: &ProgramSpec,
        next_spec: &ProgramSpec,
        expected_state_revision: u64,
    ) -> Result<()> {
        previous_spec.validate()?;
        next_spec.validate()?;
        if previous_spec.id != *id || next_spec.id != *id {
            return Err(CamelliaNexusError::invalid_spec(
                "Configuration workspace program does not match",
            ));
        }
        let _write = self.configuration_writes.lock().await;
        let root = self.program_root(id);
        let state_path = self.configuration_state_path(id);
        let marker_path = root.join(CONFIGURATION_WORKSPACE_TRANSACTION_MARKER);
        let backup_path = root.join(CONFIGURATION_WORKSPACE_STATE_BACKUP);
        let previous_spec = previous_spec.clone();
        let next_spec = next_spec.clone();
        blocking(move || {
            if marker_path.exists() || backup_path.exists() {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ProgramBusy,
                    "A configuration workspace transaction requires recovery",
                )
                .with_message_key("CONFIGURATION_RECOVERY_REQUIRED"));
            }
            let spec_bytes =
                read_with_overflow_byte(&root.join("program.json"), PROGRAM_SPEC_MAX_BYTES)?;
            if spec_bytes.len() as u64 > PROGRAM_SPEC_MAX_BYTES {
                return Err(configuration_recovery_required());
            }
            let persisted_spec: ProgramSpec = serde_json::from_slice(&spec_bytes)?;
            if persisted_spec != previous_spec {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Program settings changed before the workspace transaction began",
                )
                .with_message_key("CONFIGURATION_STATE_STALE"));
            }
            let previous_state_present = state_path.exists();
            if previous_state_present {
                let bytes = read_with_overflow_byte(&state_path, CONFIGURATION_STATE_MAX_BYTES)?;
                if bytes.len() as u64 > CONFIGURATION_STATE_MAX_BYTES {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::ConfigInvalid,
                        "Configuration state exceeds the 32 MiB limit",
                    ));
                }
                let current = decode_configuration_state(&bytes)?;
                if current.state_revision != expected_state_revision {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::ConfigConflict,
                        "Configuration state changed before the workspace transaction began",
                    )
                    .with_message_key("CONFIGURATION_STATE_STALE"));
                }
                write_bytes_atomic(&backup_path, &bytes)?;
            } else if expected_state_revision != 0 {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Configuration state disappeared before the workspace transaction began",
                )
                .with_message_key("CONFIGURATION_STATE_STALE"));
            }
            let marker = ConfigurationWorkspaceTransactionMarker {
                previous_spec,
                next_spec,
                expected_state_revision,
                previous_state_present,
                state_committed: false,
            };
            write_json_atomic(&marker_path, &marker)
        })
        .await
    }

    pub async fn mark_configuration_workspace_update_committed(
        &self,
        id: &ProgramId,
        next_spec: &ProgramSpec,
    ) -> Result<()> {
        let _write = self.configuration_writes.lock().await;
        let root = self.program_root(id);
        let marker_path = root.join(CONFIGURATION_WORKSPACE_TRANSACTION_MARKER);
        let next_spec = next_spec.clone();
        blocking(move || {
            let mut marker =
                load_configuration_workspace_transaction_marker(&root)?.ok_or_else(|| {
                    CamelliaNexusError::new(
                        ErrorCode::InvalidState,
                        "Configuration workspace transaction marker is missing",
                    )
                })?;
            let bytes =
                read_with_overflow_byte(&root.join("program.json"), PROGRAM_SPEC_MAX_BYTES)?;
            if bytes.len() as u64 > PROGRAM_SPEC_MAX_BYTES {
                return Err(configuration_recovery_required());
            }
            let persisted_spec: ProgramSpec = serde_json::from_slice(&bytes)?;
            if persisted_spec != next_spec || marker.next_spec.id != next_spec.id {
                return Err(configuration_recovery_required());
            }
            marker.next_spec = next_spec;
            marker.state_committed = true;
            write_json_atomic(&marker_path, &marker).map_err(|error| {
                if load_configuration_workspace_transaction_marker(&root)
                    .ok()
                    .flatten()
                    .is_some_and(|marker| marker.state_committed)
                {
                    workspace_commit_recovery(error)
                } else {
                    error
                }
            })
        })
        .await
    }

    pub async fn finish_configuration_workspace_update(&self, id: &ProgramId) -> Result<()> {
        let _write = self.configuration_writes.lock().await;
        let root = self.program_root(id);
        blocking(move || {
            for path in [
                root.join(CONFIGURATION_WORKSPACE_TRANSACTION_MARKER),
                root.join(CONFIGURATION_WORKSPACE_STATE_BACKUP),
            ] {
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                sync_directory(&root)?;
            }
            sync_directory(&root)
        })
        .await
    }

    pub async fn rollback_configuration_workspace_update(&self, id: &ProgramId) -> Result<()> {
        let _write = self.configuration_writes.lock().await;
        let root = self.program_root(id);
        blocking(move || recover_configuration_workspace_transaction(&root)).await
    }

    pub async fn reconcile_configuration_workspace(&self, spec: &ProgramSpec) -> Result<()> {
        let _write = self.configuration_writes.lock().await;
        let root = self.program_root(&spec.id);
        let spec = spec.clone();
        blocking(
            move || match load_configuration_workspace_transaction_marker(&root)? {
                None => {
                    let backup = root.join(CONFIGURATION_WORKSPACE_STATE_BACKUP);
                    match fs::remove_file(backup) {
                        Ok(()) => sync_directory(&root),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(error.into()),
                    }
                }
                Some(marker) if marker.state_committed && marker.next_spec == spec => {
                    recover_configuration_workspace_transaction(&root)
                        .map_err(workspace_commit_recovery)
                }
                Some(_) => Err(configuration_recovery_required()),
            },
        )
        .await
    }

    pub async fn load_final_editor_draft(
        &self,
        id: &ProgramId,
    ) -> Result<Option<FinalEditorSession>> {
        let path = self.configuration_state_path(id);
        blocking(move || {
            Ok(read_configuration_state_file(&path)?.and_then(|state| state.editor_session))
        })
        .await
    }

    pub async fn save_final_editor_draft(
        &self,
        id: &ProgramId,
        draft: &FinalEditorSession,
        expected_revision: Option<u64>,
    ) -> Result<()> {
        let _write = self.configuration_writes.lock().await;
        let path = self.configuration_state_path(id);
        let mut draft = draft.clone();
        blocking(move || {
            validate_editor_session_size(&draft)?;
            let mut state =
                read_configuration_state_file(&path)?.ok_or_else(configuration_state_stale)?;
            let current = state.editor_session.as_ref();
            if let Some(expected) = expected_revision
                && (current.map_or(0, |session| session.draft_revision) != expected
                    || current.is_some_and(|session| session.session_id != draft.session_id))
            {
                return Err(configuration_draft_stale());
            }
            if current == Some(&draft) {
                return Ok(());
            }
            state.state_revision = state.state_revision.saturating_add(1);
            camellia_nexus_core::rebase_final_editor_session(
                &mut draft,
                state.format,
                &state.desired.content,
                state.state_revision,
                state.generation,
            )?;
            state.editor_session = Some(draft);
            write_configuration_state_file(&path, &state)
        })
        .await
    }

    pub async fn discard_final_editor_draft(
        &self,
        id: &ProgramId,
        expected_revision: u64,
    ) -> Result<()> {
        let _write = self.configuration_writes.lock().await;
        let path = self.configuration_state_path(id);
        blocking(move || {
            let mut state =
                read_configuration_state_file(&path)?.ok_or_else(configuration_state_stale)?;
            if state
                .editor_session
                .as_ref()
                .map_or(0, |draft| draft.draft_revision)
                != expected_revision
            {
                return Err(configuration_draft_stale());
            }
            if state.editor_session.take().is_none() {
                return Ok(());
            }
            state.state_revision = state.state_revision.saturating_add(1);
            write_configuration_state_file(&path, &state)
        })
        .await
    }

    pub async fn save_last_known_good(
        &self,
        id: &ProgramId,
        format: ConfigurationFormat,
        content: &str,
    ) -> Result<()> {
        if content.len() > MAX_CONFIG_BYTES {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Last known good configuration exceeds the 4 MiB limit",
            ));
        }
        let file_name = match format {
            ConfigurationFormat::Jsonc => CONFIGURATION_LKG_JSON,
            ConfigurationFormat::Yaml => CONFIGURATION_LKG_YAML,
        };
        let path = self
            .program_root(id)
            .join(CONFIGURATION_STATE_DIRECTORY)
            .join(file_name);
        let bytes = content.as_bytes().to_vec();
        blocking(move || write_bytes_atomic(&path, &bytes)).await
    }

    pub async fn load_last_known_good(
        &self,
        id: &ProgramId,
        format: ConfigurationFormat,
    ) -> Result<Option<String>> {
        let file_name = match format {
            ConfigurationFormat::Jsonc => CONFIGURATION_LKG_JSON,
            ConfigurationFormat::Yaml => CONFIGURATION_LKG_YAML,
        };
        let path = self
            .program_root(id)
            .join(CONFIGURATION_STATE_DIRECTORY)
            .join(file_name);
        blocking(move || {
            if !path.exists() {
                return Ok(None);
            }
            let bytes = read_with_overflow_byte(&path, MAX_CONFIG_BYTES as u64)?;
            if bytes.len() > MAX_CONFIG_BYTES {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Last known good configuration exceeds the 4 MiB limit",
                ));
            }
            String::from_utf8(bytes).map(Some).map_err(|error| {
                CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Last known good configuration is not UTF-8",
                )
                .with_details(error.to_string())
            })
        })
        .await
    }

    pub async fn begin_configuration_apply(
        &self,
        id: &ProgramId,
        state: &ConfigurationState,
    ) -> Result<()> {
        let path = self.program_root(id).join(CONFIGURATION_APPLY_MARKER);
        let root = self.program_root(id);
        let sidecar_root = root
            .join(CONFIGURATION_STATE_DIRECTORY)
            .join(CONFIGURATION_SIDECAR_DIRECTORY);
        let marker = ConfigurationApplyMarker {
            state: state.clone(),
            phase: ConfigurationApplyPhase::Prepared,
        };
        blocking(move || {
            if path.exists() {
                return Err(configuration_recovery_required());
            }
            fs::create_dir_all(&sidecar_root)?;
            let mut marker = marker;
            compact_configuration_state(&sidecar_root, &mut marker.state)?;
            let bytes = serde_json::to_vec(&marker)?;
            if bytes.len() as u64 > CONFIGURATION_STATE_MAX_BYTES {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Configuration apply transaction exceeds the 32 MiB limit",
                ));
            }
            write_bytes_atomic(&path, &bytes)
        })
        .await
    }

    pub async fn finish_configuration_apply(&self, id: &ProgramId) -> Result<()> {
        let root = self.program_root(id);
        let path = root.join(CONFIGURATION_APPLY_MARKER);
        blocking(move || {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            sync_directory(&root)
        })
        .await
    }

    pub async fn reconcile_configuration_apply(&self, spec: &ProgramSpec) -> Result<()> {
        let _write = self.configuration_writes.lock().await;
        let root = self.program_root(&spec.id);
        let spec = spec.clone();
        blocking(move || match read_configuration_apply_marker(&root)? {
            None => Ok(()),
            Some(marker) if marker.phase == ConfigurationApplyPhase::Committed => {
                recover_configuration_apply_transaction(&root, &spec)
            }
            Some(_) => Err(configuration_recovery_required()),
        })
        .await
    }

    fn clean_trash(&self) {
        let trash = self.programs_root().join(".trash");
        if let Ok(entries) = fs::read_dir(trash) {
            for entry in entries.flatten() {
                remove_directory_bounded(entry.path());
            }
        }
    }
}

#[async_trait]
impl ProgramStore for FileStore {
    async fn load_all(&self) -> Result<LoadReport> {
        let _write = self.configuration_writes.lock().await;
        let root = self.programs_root();
        blocking(move || {
            fs::create_dir_all(&root)?;
            let canonical_root = fs::canonicalize(&root)?;
            let mut report = LoadReport::default();
            for entry in fs::read_dir(&root)? {
                let entry = entry?;
                let file_name = entry.file_name();
                if !entry.file_type()?.is_dir() || file_name.to_string_lossy().starts_with('.') {
                    continue;
                }
                let observed_path = root.join(&file_name);
                let (workspace_id, path) =
                    match validated_program_workspace(&root, &canonical_root, &file_name) {
                        Ok(workspace) => workspace,
                        Err(error) => {
                            report.invalid.push(InvalidProgram {
                                path: observed_path,
                                error: error.to_string(),
                            });
                            continue;
                        }
                    };
                let create_marker = path.join(".pending");
                if create_marker.exists() {
                    let committed = read_with_overflow_byte(&create_marker, 64)
                        .is_ok_and(|bytes| bytes == CREATE_COMMITTED_MARKER);
                    if !committed {
                        let _ = fs::remove_dir_all(&path);
                        continue;
                    }
                    if let Err(error) = fs::remove_file(&create_marker) {
                        tracing::warn!(path = %create_marker.display(), %error, "could not remove committed program creation marker");
                    }
                    if let Err(error) = sync_directory(&path) {
                        tracing::warn!(path = %path.display(), %error, "could not sync committed program creation cleanup");
                    }
                }
                let spec_path = path.join("program.json");
                let loaded = (|| -> Result<ProgramSpec> {
                    recover_configuration_workspace_transaction(&path)?;
                    recover_program_package_transaction(&path)?;
                    clean_atomic_temps(&path);
                    let content = read_with_overflow_byte(&spec_path, PROGRAM_SPEC_MAX_BYTES)?;
                    if content.len() as u64 > PROGRAM_SPEC_MAX_BYTES {
                        return Err(CamelliaNexusError::invalid_spec(
                            "program.json exceeds the 1 MiB limit",
                        ));
                    }
                    let spec = decode_program_spec(&content)?;
                    spec.validate()?;
                    if spec.id != workspace_id {
                        return Err(CamelliaNexusError::invalid_spec(
                            "Workspace name does not match Program id",
                        ));
                    }
                    Ok(spec)
                })();
                match loaded {
                    Ok(spec) => report.valid.push(StoredProgram {
                        spec,
                        workspace: path,
                    }),
                    Err(error) => report.invalid.push(InvalidProgram {
                        path,
                        error: error.to_string(),
                    }),
                }
            }
            Ok(report)
        })
        .await
    }

    async fn create_pending(&self, spec: &ProgramSpec, assets: CreateAssets) -> Result<PathBuf> {
        let root = self.program_root(&spec.id);
        let spec = spec.clone();
        blocking(move || {
            if root.exists() {
                return Err(CamelliaNexusError::new(
                    ErrorCode::AlreadyExists,
                    "Program workspace already exists",
                ));
            }
            fs::create_dir_all(&root)?;
            let result = (|| -> Result<()> {
                write_bytes_atomic(&root.join(".pending"), CREATE_PENDING_MARKER)?;
                if spec.executable.is_managed() {
                    fs::create_dir_all(root.join("data"))?;
                    fs::create_dir_all(root.join("logs"))?;
                    let source = assets.package_source.ok_or_else(|| {
                        CamelliaNexusError::invalid_spec(
                            "Managed executable requires a program source directory",
                        )
                    })?;
                    copy_package(&source, &root.join("bin"))?;
                }
                let executable = spec.executable_path(&root);
                if !executable.is_file() {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::UnsupportedBinary,
                        "Executable does not exist in the imported package",
                    ));
                }
                if let Some(config_path) = spec.program_type.main_config() {
                    let content = assets.initial_config.ok_or_else(|| {
                        CamelliaNexusError::invalid_spec(
                            "Program type requires initial configuration",
                        )
                    })?;
                    let target = safe_path(&root, config_path)?;
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(target, content)?;
                }
                write_json_atomic(&root.join("program.json"), &spec)?;
                Ok(())
            })();
            if let Err(error) = result {
                let _ = fs::remove_dir_all(&root);
                return Err(error);
            }
            Ok(root)
        })
        .await
    }

    async fn commit_create(&self, id: &ProgramId) -> Result<()> {
        let root = self.program_root(id);
        let pending = root.join(".pending");
        blocking(move || {
            let marker = read_with_overflow_byte(&pending, 64)?;
            if marker != CREATE_PENDING_MARKER && marker != CREATE_COMMITTED_MARKER {
                return Err(CamelliaNexusError::new(
                    ErrorCode::Storage,
                    "Program creation marker is invalid",
                ));
            }
            if marker != CREATE_COMMITTED_MARKER
                && let Err(error) = write_bytes_atomic(&pending, CREATE_COMMITTED_MARKER)
            {
                let committed = read_with_overflow_byte(&pending, 64)
                    .is_ok_and(|bytes| bytes == CREATE_COMMITTED_MARKER);
                if !committed {
                    return Err(error);
                }
                tracing::warn!(%error, "program creation commit marker was installed but its directory sync reported an error");
            }
            if let Err(error) = fs::remove_file(&pending) {
                tracing::warn!(path = %pending.display(), %error, "could not remove committed program creation marker");
            }
            if let Err(error) = sync_directory(&root) {
                tracing::warn!(path = %root.display(), %error, "could not sync committed program creation cleanup");
            }
            Ok(())
        })
        .await
    }

    async fn discard_pending(&self, id: &ProgramId) -> Result<()> {
        let root = self.program_root(id);
        let programs_root = self.programs_root();
        let id = id.clone();
        blocking(move || {
            if root.exists() {
                let trash = programs_root
                    .join(".trash")
                    .join(format!("{id}-pending-{}", Uuid::new_v4()));
                fs::rename(&root, &trash)?;
                remove_directory_bounded(trash);
                if let Err(error) = sync_directory(&programs_root) {
                    tracing::warn!(path = %programs_root.display(), %error, "could not sync discarded pending program workspace");
                }
            }
            Ok(())
        })
        .await
    }

    async fn save(&self, spec: &ProgramSpec) -> Result<()> {
        let path = self.program_root(&spec.id).join("program.json");
        let spec = spec.clone();
        blocking(move || write_json_atomic(&path, &spec)).await
    }

    async fn workspace(&self, id: &ProgramId) -> Result<PathBuf> {
        let root = self.program_root(id);
        if root.is_dir() {
            Ok(root)
        } else {
            Err(CamelliaNexusError::new(
                ErrorCode::NotFound,
                "Program workspace not found",
            ))
        }
    }

    async fn executable_metadata(&self, spec: &ProgramSpec) -> Result<ExecutableMetadata> {
        let root = self.program_root(&spec.id);
        let path = spec.executable_path(&root);
        blocking(move || {
            let marker = load_program_package_transaction_marker(&root)?;
            if marker
                .as_ref()
                .is_some_and(|marker| marker.phase == ProgramPackageTransactionPhase::Prepared)
                || (marker.is_none() && root.join("bin.old").exists())
            {
                return Err(CamelliaNexusError::new(
                    ErrorCode::Storage,
                    "Program replacement requires recovery",
                )
                .with_message_key("PROGRAM_PACKAGE_RECOVERY_REQUIRED"));
            }
            executable_metadata(&path)
        })
        .await
    }

    async fn configuration_validation_evidence(
        &self,
        spec: &ProgramSpec,
    ) -> Result<Option<camellia_nexus_core::CoreValidationEvidence>> {
        Ok(self
            .load_configuration_state(&spec.id)
            .await?
            .and_then(|state| state.applied)
            .and_then(|candidate| {
                candidate.validation_evidence.filter(|evidence| {
                    evidence.candidate_generation == candidate.revision.generation
                })
            }))
    }

    async fn stage_package(&self, spec: &ProgramSpec, source: &Path) -> Result<StagedPackage> {
        if !spec.executable.is_managed() {
            return Err(CamelliaNexusError::invalid_spec(
                "Only managed executables can replace their package",
            ));
        }
        let root = self.program_root(&spec.id);
        let source = source.to_path_buf();
        let program_id = spec.id.clone();
        let executable_relative = spec
            .executable
            .path()
            .strip_prefix("bin")
            .map_err(|_| CamelliaNexusError::invalid_spec("Managed executable must be under bin/"))?
            .to_path_buf();
        blocking(move || {
            recover_program_package_transaction(&root)?;
            let canonical_source = fs::canonicalize(&source)?;
            let canonical_root = fs::canonicalize(&root)?;
            if path_is_within(&canonical_source, &canonical_root) {
                return Err(CamelliaNexusError::new(
                    ErrorCode::InvalidPath,
                    "Program source cannot be inside the Program workspace",
                ));
            }
            let staged_directory = root.join("bin.new");
            if staged_directory.exists() {
                discard_directory_background(&staged_directory)?;
            }
            if let Err(error) = copy_package(&source, &staged_directory) {
                let _ = fs::remove_dir_all(&staged_directory);
                return Err(error);
            }
            let executable = safe_path(&staged_directory, &executable_relative)?;
            let metadata = match executable_metadata(&executable) {
                Ok(metadata) => metadata,
                Err(error) => {
                    let _ = fs::remove_dir_all(&staged_directory);
                    return Err(error);
                }
            };
            Ok(StagedPackage {
                program_id,
                staged_directory,
                executable,
                metadata,
            })
        })
        .await
    }

    async fn commit_package(
        &self,
        staged: StagedPackage,
        expected_spec: &ProgramSpec,
        next_spec: &ProgramSpec,
        configuration: Option<camellia_nexus_core::PackageConfigurationUpdate>,
    ) -> Result<()> {
        let _write = self.configuration_writes.lock().await;
        let root = self.program_root(&staged.program_id);
        let expected_spec = expected_spec.clone();
        let next_spec = next_spec.clone();
        #[cfg(test)]
        let fail_commit = self.fail_package_commit.clone();
        blocking(move || {
            let expected = root.join("bin.new");
            if staged.staged_directory != expected || staged.program_id != expected_spec.id || staged.program_id != next_spec.id {
                return Err(CamelliaNexusError::new(ErrorCode::InvalidPath, "Invalid staged package path"));
            }
            if next_spec.program_type.main_config().is_some() != configuration.is_some() {
                return Err(CamelliaNexusError::new(ErrorCode::InvalidState, "Package replacement requires its configuration workspace"));
            }
            recover_program_package_transaction(&root)?;
            let marker_path = root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER);
            let spec_path = root.join("program.json");
            let spec_backup = root.join(PROGRAM_PACKAGE_SPEC_BACKUP);
            let next_spec_path = root.join(PROGRAM_PACKAGE_NEXT_SPEC);
            let state_path = root.join(CONFIGURATION_STATE_DIRECTORY).join(CONFIGURATION_STATE_FILE);
            let state_backup = root.join(PROGRAM_PACKAGE_STATE_BACKUP);
            let next_state_path = root.join(PROGRAM_PACKAGE_NEXT_STATE);
            let active = root.join("bin");
            let backup = root.join("bin.old");
            if [ &marker_path, &spec_backup, &next_spec_path, &state_backup, &next_state_path, &backup ].iter().any(|path| path.exists()) {
                return Err(CamelliaNexusError::new(ErrorCode::ProgramBusy, "Managed package recovery materials require attention"));
            }
            let stored_bytes = read_with_overflow_byte(&spec_path, PROGRAM_SPEC_MAX_BYTES)?;
            if stored_bytes.len() as u64 > PROGRAM_SPEC_MAX_BYTES { return Err(configuration_recovery_required()); }
            if decode_program_spec(&stored_bytes)? != expected_spec {
                return Err(configuration_state_stale());
            }
            // Recheck the exact staged file at the commit boundary, not only its metadata captured during copying.
            let camellia_nexus_core::ExecutableSpec::Managed { path, .. } = &next_spec.executable else {
                return Err(CamelliaNexusError::invalid_spec("Package replacement requires a managed executable"));
            };
            let relative = path.strip_prefix("bin").map_err(|_| CamelliaNexusError::new(ErrorCode::InvalidPath, "Managed executable must be inside bin"))?;
            if staged.executable != safe_path(&expected, relative)? {
                return Err(CamelliaNexusError::new(ErrorCode::InvalidPath, "Invalid staged executable path"));
            }
            let actual = executable_metadata(&staged.executable)?;
            if next_spec.executable.metadata().is_none_or(|metadata| metadata.fingerprint != actual.fingerprint) {
                return Err(CamelliaNexusError::new(ErrorCode::ConfigConflict, "Prepared executable changed").with_message_key("CORE_BINARY_IDENTITY_MISMATCH"));
            }
            let has_workspace = configuration.is_some();
            let prepare = (|| -> Result<()> {
                write_bytes_atomic(&spec_backup, &stored_bytes)?;
                write_json_atomic(&next_spec_path, &next_spec)?;
                if let Some(update) = configuration {
                    let current = read_configuration_state_file(&state_path)?.ok_or_else(configuration_state_stale)?;
                    if current.state_revision != update.expected_state_revision { return Err(configuration_state_stale()); }
                    let backup_bytes = read_with_overflow_byte(&state_path, CONFIGURATION_STATE_MAX_BYTES)?;
                    if backup_bytes.len() as u64 > CONFIGURATION_STATE_MAX_BYTES { return Err(configuration_recovery_required()); }
                    let mut next = update.state;
                    next.editor_session = current.editor_session.clone();
                    if let Some(draft) = next.editor_session.as_mut() {
                        let previous = draft.clone();
                        camellia_nexus_core::rebase_final_editor_session(draft, next.format, &next.desired.content, next.state_revision, next.generation)?;
                        if *draft != previous { draft.draft_revision = previous.draft_revision.saturating_add(1); }
                    }
                    let sidecars = root.join(CONFIGURATION_STATE_DIRECTORY).join(CONFIGURATION_SIDECAR_DIRECTORY);
                    fs::create_dir_all(&sidecars)?;
                    compact_configuration_state(&sidecars, &mut next)?;
                    write_bytes_atomic(&state_backup, &backup_bytes)?;
                    write_configuration_state_file(&next_state_path, &next)?;
                }
                write_json_atomic(&marker_path, &ProgramPackageTransactionMarker {
                    phase: ProgramPackageTransactionPhase::Prepared, workspace: has_workspace,
                })
            })();
            if let Err(error) = prepare {
                let recovery = if marker_path.exists() {
                    recover_program_package_transaction(&root)
                } else {
                    // No active file has changed before the prepared marker becomes durable.
                    for path in [&spec_backup, &next_spec_path, &state_backup, &next_state_path] {
                        if path.exists() { fs::remove_file(path)?; }
                    }
                    Ok(())
                };
                return Err(match recovery { Ok(()) => error, Err(recovery) => package_recovery_required(&error, &recovery) });
            }
            let commit = (|| -> Result<()> {
                replace_file(&next_spec_path, &spec_path)?;
                if has_workspace { replace_file(&next_state_path, &state_path)?; }
                #[cfg(test)]
                if fail_commit.compare_exchange(1,0,std::sync::atomic::Ordering::SeqCst,std::sync::atomic::Ordering::SeqCst).is_ok() {
                    return Err(CamelliaNexusError::new(ErrorCode::Storage, "Injected package workspace write failure"));
                }
                fs::rename(&active, &backup)?;
                fs::rename(&expected, &active)?;
                sync_directory(&root)?;
                #[cfg(test)]
                if fail_commit.compare_exchange(2,0,std::sync::atomic::Ordering::SeqCst,std::sync::atomic::Ordering::SeqCst).is_ok() {
                    return Err(CamelliaNexusError::new(ErrorCode::Storage, "Injected package swap failure"));
                }
                write_json_atomic(&marker_path, &ProgramPackageTransactionMarker {
                    phase: ProgramPackageTransactionPhase::Committed, workspace: has_workspace,
                })?;
                Ok(())
            })();
            if let Err(error) = commit {
                if load_program_package_transaction_marker(&root)?.is_some_and(|marker| marker.phase == ProgramPackageTransactionPhase::Committed) {
                    tracing::warn!(code = ?error.code, "managed package committed; cleanup requires recovery");
                } else {
                    return Err(match recover_program_package_transaction(&root) { Ok(()) => error, Err(recovery) => package_recovery_required(&error, &recovery) });
                }
            }
            cleanup_finished_program_package_transaction(&root);
            Ok(())
        }).await
    }

    async fn discard_package(&self, staged: StagedPackage) -> Result<()> {
        let root = self.program_root(&staged.program_id);
        blocking(move || {
            if staged.staged_directory != root.join("bin.new") {
                return Err(CamelliaNexusError::new(
                    ErrorCode::InvalidPath,
                    "Invalid staged package path",
                ));
            }
            if staged.staged_directory.exists() {
                discard_directory_background(&staged.staged_directory)?;
            }
            Ok(())
        })
        .await
    }

    async fn read_log(
        &self,
        spec: &ProgramSpec,
        stream: LogStream,
        max_bytes: usize,
    ) -> Result<LogChunk> {
        let name = match stream {
            LogStream::Stdout => "stdout.log",
            LogStream::Stderr => "stderr.log",
        };
        let workspace = self.program_root(&spec.id);
        let path = spec.log_path(&workspace, name);
        blocking(move || read_tail(&path, max_bytes)).await
    }

    async fn clear_logs(&self, spec: &ProgramSpec) -> Result<()> {
        let workspace = self.program_root(&spec.id);
        let paths = [
            spec.log_path(&workspace, "stdout.log"),
            spec.log_path(&workspace, "stderr.log"),
        ];
        blocking(move || {
            for path in paths {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&path)?
                    .flush()?;
                for index in 1..=3 {
                    let mut rotated = path.as_os_str().to_os_string();
                    rotated.push(format!(".{index}"));
                    match fs::remove_file(PathBuf::from(rotated)) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            Ok(())
        })
        .await
    }

    async fn recover_workspace(&self, spec: &ProgramSpec) -> Result<()> {
        let root = self.program_root(&spec.id);
        let managed = spec.executable.is_managed();
        blocking(move || {
            clean_discarded_package_directories(&root);
            recover_program_package_transaction(&root)?;
            if !managed {
                return Ok(());
            }
            let bin = root.join("bin");
            let new = root.join("bin.new");
            if bin.exists() && new.exists() {
                discard_directory_background(&new)?;
            }
            Ok(())
        })
        .await
    }

    async fn remove_workspace(&self, id: &ProgramId) -> Result<()> {
        let root = self.program_root(id);
        let trash = self.programs_root().join(".trash").join(format!(
            "{}-{}",
            id,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));
        blocking(move || {
            fs::rename(&root, &trash)?;
            remove_directory_bounded(trash);
            Ok(())
        })
        .await
    }
}

fn decode_program_spec(content: &[u8]) -> Result<ProgramSpec> {
    serde_json::from_slice(content)
        .map_err(|error| CamelliaNexusError::invalid_spec(error.to_string()))
}

fn clean_atomic_temps(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".camellia-nexus-write-") && name.ends_with(".tmp") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[async_trait]
impl ConfigStore for FileStore {
    async fn load(&self, spec: &ProgramSpec) -> Result<RawConfig> {
        let path = self.config_path(spec)?;
        blocking(move || {
            let bytes = read_with_overflow_byte(&path, MAX_CONFIG_BYTES as u64)?;
            if bytes.len() > MAX_CONFIG_BYTES {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Configuration exceeds the 4 MiB limit",
                ));
            }
            let content = String::from_utf8(bytes.clone()).map_err(|error| {
                CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Configuration is not UTF-8")
                    .with_details(error.to_string())
            })?;
            Ok(RawConfig {
                content,
                base_hash: hash_bytes(&bytes),
            })
        })
        .await
    }

    async fn stage(&self, spec: &ProgramSpec, content: &[u8]) -> Result<StagedConfig> {
        if content.len() > MAX_CONFIG_BYTES {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Configuration exceeds the 4 MiB limit",
            ));
        }
        let target = self.config_path(spec)?;
        let content = content.to_vec();
        blocking(move || {
            let parent = target.parent().ok_or_else(|| {
                CamelliaNexusError::new(ErrorCode::InvalidPath, "Configuration has no parent")
            })?;
            fs::create_dir_all(parent)?;
            let extension = target
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("json");
            let path = parent.join(format!(
                ".camellia-nexus-staged-{}.{}",
                Uuid::new_v4(),
                extension
            ));
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)?;
            file.write_all(&content)?;
            file.sync_all()?;
            Ok(StagedConfig {
                path,
                backup: suffixed_path(&target, ".bak"),
                target,
            })
        })
        .await
    }

    async fn read_staged(&self, staged: &StagedConfig) -> Result<String> {
        let path = staged.path.clone();
        blocking(move || {
            let bytes = read_with_overflow_byte(&path, MAX_CONFIG_BYTES as u64)?;
            if bytes.len() > MAX_CONFIG_BYTES {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Formatted configuration exceeds the 4 MiB limit",
                ));
            }
            String::from_utf8(bytes).map_err(|error| {
                CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Formatted configuration is not UTF-8",
                )
                .with_details(error.to_string())
            })
        })
        .await
    }

    async fn current_hash(&self, spec: &ProgramSpec) -> Result<String> {
        let path = self.config_path(spec)?;
        blocking(move || {
            let bytes = read_with_overflow_byte(&path, MAX_CONFIG_BYTES as u64)?;
            if bytes.len() > MAX_CONFIG_BYTES {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Configuration exceeds the 4 MiB limit",
                ));
            }
            Ok(hash_bytes(&bytes))
        })
        .await
    }

    async fn atomic_replace_with_backup(
        &self,
        staged: StagedConfig,
        expected_hash: &str,
    ) -> Result<()> {
        let expected_hash = expected_hash.to_owned();
        blocking(move || {
            let current = read_with_overflow_byte(&staged.target, MAX_CONFIG_BYTES as u64)?;
            if current.len() > MAX_CONFIG_BYTES || hash_bytes(&current) != expected_hash {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Configuration changed before the prepared content committed",
                ));
            }
            let pending = suffixed_path(&staged.target, ".pending");
            write_bytes_atomic(&pending, b"pending\n")?;
            if let Err(error) = replace_with_backup(&staged.path, &staged.target, &staged.backup) {
                let _ = fs::remove_file(pending);
                return Err(error);
            }
            Ok(())
        })
        .await
    }

    async fn finalize_replace(&self, spec: &ProgramSpec) -> Result<()> {
        let _write = self.configuration_writes.lock().await;
        let target = self.config_path(spec)?;
        let root = self.program_root(&spec.id);
        let spec = spec.clone();
        #[cfg(test)]
        let fail_commit = self.fail_apply_commit.clone();
        blocking(move || {
            let backup = suffixed_path(&target, ".bak");
            let pending = suffixed_path(&target, ".pending");
            #[cfg(test)]
            let fail_stage = fail_commit.swap(0, std::sync::atomic::Ordering::SeqCst);
            #[cfg(test)]
            if fail_stage == 1 {
                return Err(CamelliaNexusError::new(ErrorCode::Storage, "Injected failure before apply commit"));
            }
            match mark_configuration_apply_committed(&root, &spec) {
                Ok(true) => {
                    #[cfg(test)]
                    if fail_stage == 2 {
                        return Err(committed_apply_recovery(CamelliaNexusError::new(ErrorCode::Storage, "Injected failure after apply commit")));
                    }
                    recover_configuration_apply_transaction(&root, &spec)
                        .map_err(committed_apply_recovery)?;
                }
                Ok(false) => {}
                Err(error) => {
                    return Err(if read_configuration_apply_marker(&root)?.is_some_and(|marker|
                        marker.phase == ConfigurationApplyPhase::Committed)
                    { committed_apply_recovery(error) } else { error });
                }
            }
            if pending.exists() {
                fs::remove_file(&pending)?;
            }
            if let Some(parent) = target.parent() {
                // Removing and syncing the pending marker is the durable commit point. The
                // backup is cleanup only after this point, so a cleanup error can never make
                // startup recovery roll back a configuration already reported as committed.
                sync_directory(parent).map_err(committed_apply_recovery)?;
                if backup.exists()
                    && let Err(error) = fs::remove_file(&backup)
                {
                    tracing::warn!(path = %backup.display(), %error, "could not remove committed configuration backup");
                }
                // Backup deletion is cleanup after the durable commit.
                let _ = sync_directory(parent);
            }
            Ok(())
        })
        .await
    }

    async fn restore_backup(&self, spec: &ProgramSpec) -> Result<()> {
        let target = self.config_path(spec)?;
        blocking(move || {
            let backup = suffixed_path(&target, ".bak");
            let pending = suffixed_path(&target, ".pending");
            if !backup.exists() {
                return Err(CamelliaNexusError::new(
                    ErrorCode::Storage,
                    "Configuration backup is missing",
                ));
            }
            let failed = suffixed_path(&target, ".failed");
            let had_target = target.exists();
            if had_target {
                let _ = fs::remove_file(&failed);
                fs::rename(&target, &failed)?;
            }
            if let Err(error) = fs::rename(&backup, &target) {
                if had_target {
                    let _ = fs::rename(&failed, &target);
                }
                return Err(error.into());
            }
            if let Some(parent) = target.parent() {
                if pending.exists() {
                    fs::remove_file(&pending)?;
                }
                if failed.exists()
                    && let Err(error) = fs::remove_file(&failed)
                {
                    tracing::warn!(path = %failed.display(), %error, "could not remove rejected configuration after rollback");
                }
                sync_directory(parent)?;
            }
            Ok(())
        })
        .await
    }

    async fn discard_staged(&self, staged: StagedConfig) -> Result<()> {
        blocking(move || {
            if staged.path.exists() {
                fs::remove_file(staged.path)?;
            }
            Ok(())
        })
        .await
    }

    async fn recover(&self, spec: &ProgramSpec) -> Result<()> {
        let Some(_) = spec.program_type.main_config() else {
            return Ok(());
        };
        let target = self.config_path(spec)?;
        let root = self.program_root(&spec.id);
        let spec = spec.clone();
        blocking(move || {
            if read_configuration_apply_marker(&root)?.is_some_and(|marker|
                marker.phase == ConfigurationApplyPhase::Committed)
            {
                recover_configuration_apply_transaction(&root, &spec)?;
            }
            let backup = suffixed_path(&target, ".bak");
            let pending = suffixed_path(&target, ".pending");
            let failed = suffixed_path(&target, ".failed");
            if pending.exists() {
                if backup.exists() {
                    if target.exists() {
                        fs::remove_file(&target)?;
                    }
                    fs::rename(&backup, &target)?;
                }
                fs::remove_file(&pending)?;
            } else if !target.exists() && backup.exists() {
                fs::rename(&backup, &target)?;
            } else if target.exists()
                && backup.exists()
                && let Err(error) = fs::remove_file(&backup)
            {
                tracing::warn!(path = %backup.display(), %error, "could not remove stale committed configuration backup during recovery");
            }
            if failed.exists()
                && let Err(error) = fs::remove_file(&failed)
            {
                tracing::warn!(path = %failed.display(), %error, "could not remove stale rejected configuration during recovery");
            }
            if let Some(parent) = target.parent() {
                for entry in fs::read_dir(parent)? {
                    let entry = entry?;
                    if entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".camellia-nexus-staged-")
                    {
                        let _ = fs::remove_file(entry.path());
                    }
                }
                sync_directory(parent)?;
            }
            recover_configuration_apply_transaction(&root, &spec)
        })
        .await
    }
}

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(CamelliaNexusError::internal)?
}

fn safe_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    camellia_nexus_core::model::validate_relative_path(relative, false)?;
    let joined = root.join(relative);
    let mut cursor = root.to_path_buf();
    for component in relative.components() {
        cursor.push(component.as_os_str());
        if let Ok(metadata) = fs::symlink_metadata(&cursor)
            && is_link_or_reparse(&metadata)
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::InvalidPath,
                "Workspace path contains a link or reparse point",
            ));
        }
    }
    Ok(joined)
}

fn decode_configuration_state(bytes: &[u8]) -> Result<ConfigurationState> {
    serde_json::from_slice(bytes).map_err(|error| {
        CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Configuration state is invalid")
            .with_details(error.to_string())
    })
}

fn configuration_state_stale() -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::ConfigConflict, "Configuration workspace changed")
        .with_message_key("CONFIGURATION_STATE_STALE")
}

fn configuration_draft_stale() -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::ConfigConflict, "Final editor draft changed")
        .with_message_key("CONFIGURATION_DRAFT_STALE")
}

fn read_configuration_state_file(path: &Path) -> Result<Option<ConfigurationState>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = read_with_overflow_byte(path, CONFIGURATION_STATE_MAX_BYTES)?;
    if bytes.len() as u64 > CONFIGURATION_STATE_MAX_BYTES {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Configuration workspace exceeds the 32 MiB limit",
        ));
    }
    decode_configuration_state(&bytes).map(Some)
}

fn validate_editor_session_size(draft: &FinalEditorSession) -> Result<()> {
    if draft.working_content.len() > MAX_CONFIG_BYTES || draft.base_content.len() > MAX_CONFIG_BYTES
    {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Final editor document exceeds the 4 MiB limit",
        ));
    }
    Ok(())
}

fn write_configuration_state_file(path: &Path, state: &ConfigurationState) -> Result<()> {
    if let Some(draft) = &state.editor_session {
        validate_editor_session_size(draft)?;
    }
    let bytes = serde_json::to_vec_pretty(state)?;
    if bytes.len() as u64 > CONFIGURATION_STATE_MAX_BYTES {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigInvalid,
            "Configuration workspace exceeds the 32 MiB limit",
        ));
    }
    write_bytes_atomic(path, &bytes)
}

fn valid_content_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Remove large source bodies from a crash-recovery marker while ensuring the
/// referenced content has already been durably materialized in its sidecar.
fn compact_configuration_state(sidecar_root: &Path, state: &mut ConfigurationState) -> Result<()> {
    for snapshot in state.source_snapshots.values_mut() {
        if !valid_content_hash(&snapshot.content_hash) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Configuration source snapshot hash is invalid",
            ));
        }
        if snapshot.content.len() > MAX_CONFIG_BYTES {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "Configuration source exceeds the 4 MiB limit",
            ));
        }
        if !snapshot.content.is_empty() {
            let path = sidecar_root.join(format!("{}.source", snapshot.content_hash));
            write_bytes_atomic(&path, snapshot.content.as_bytes())?;
            snapshot.content.clear();
        }
    }
    Ok(())
}

fn validated_program_workspace(
    root: &Path,
    canonical_root: &Path,
    file_name: &OsStr,
) -> Result<(ProgramId, PathBuf)> {
    let file_name = file_name
        .to_str()
        .ok_or_else(|| CamelliaNexusError::invalid_spec("Workspace name is not valid Unicode"))?;
    let id = ProgramId::parse(file_name.to_owned())?;
    let path = root.join(id.as_str());
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Program workspace must be a real directory",
        ));
    }
    let canonical_path = fs::canonicalize(&path)?;
    if canonical_path.parent() != Some(canonical_root) {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Program workspace must remain directly under the programs directory",
        ));
    }
    Ok((id, path))
}

fn config_path_in_root(root: &Path, spec: &ProgramSpec) -> Result<PathBuf> {
    let relative = spec.program_type.main_config().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::InvalidState,
            "Program has no managed configuration",
        )
    })?;
    safe_path(root, relative)
}

fn configuration_recovery_required() -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::Storage, "Configuration workspace needs recovery")
        .with_message_key("CONFIGURATION_RECOVERY_REQUIRED")
}

fn committed_apply_recovery(error: CamelliaNexusError) -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::Storage,
        "Configuration commit needs to be reconciled",
    )
    .with_message_key("CONFIGURATION_COMMIT_RECOVERY_REQUIRED")
    .with_details(format!("Commit recovery: {:?}", error.code))
}

fn read_configuration_apply_marker(root: &Path) -> Result<Option<ConfigurationApplyMarker>> {
    let path = root.join(CONFIGURATION_APPLY_MARKER);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = read_with_overflow_byte(&path, CONFIGURATION_STATE_MAX_BYTES)?;
    if bytes.len() as u64 > CONFIGURATION_STATE_MAX_BYTES {
        return Err(configuration_recovery_required());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| configuration_recovery_required())
}

fn verify_applying_content(
    root: &Path,
    spec: &ProgramSpec,
    marker: &ConfigurationApplyMarker,
) -> Result<()> {
    marker.state.ensure_apply_ready()?;
    let target = config_path_in_root(root, spec)?;
    let active = read_with_overflow_byte(&target, MAX_CONFIG_BYTES as u64)?;
    if active.len() > MAX_CONFIG_BYTES {
        return Err(configuration_recovery_required());
    }
    if hash_bytes(&active) != marker.state.desired.revision.content_hash {
        return Err(configuration_recovery_required());
    }
    Ok(())
}

/// This durable phase is written only after the Controller accepts stabilization.
fn mark_configuration_apply_committed(root: &Path, spec: &ProgramSpec) -> Result<bool> {
    let Some(mut marker) = read_configuration_apply_marker(root)? else {
        return Ok(false);
    };
    verify_applying_content(root, spec, &marker)?;
    if marker.phase != ConfigurationApplyPhase::Committed {
        marker.phase = ConfigurationApplyPhase::Committed;
        write_json_atomic(&root.join(CONFIGURATION_APPLY_MARKER), &marker)?;
    }
    Ok(true)
}

fn recover_configuration_apply_transaction(root: &Path, spec: &ProgramSpec) -> Result<()> {
    let Some(marker) = read_configuration_apply_marker(root)? else {
        return Ok(());
    };
    let target = config_path_in_root(root, spec)?;
    let pending = suffixed_path(&target, ".pending");
    if marker.phase == ConfigurationApplyPhase::Committed {
        verify_applying_content(root, spec, &marker)?;
        let state_path = root
            .join(CONFIGURATION_STATE_DIRECTORY)
            .join(CONFIGURATION_STATE_FILE);
        let mut state =
            read_configuration_state_file(&state_path)?.unwrap_or_else(|| marker.state.clone());
        let consume_draft =
            state.editor_session.is_some() && state.editor_session == marker.state.editor_session;
        if state.applied.as_ref() != Some(&marker.state.desired)
            || state.last_known_good.as_ref() != Some(&marker.state.desired)
            || consume_draft
        {
            state.applied = Some(marker.state.desired.clone());
            state.last_known_good = Some(marker.state.desired.clone());
            state.state_revision = state.state_revision.saturating_add(1);
        }
        if consume_draft {
            state.editor_session = None;
        }
        // The request result commits with Applied/LKG, before any recovery material is removed.
        for receipt in &marker.state.operation_receipts {
            if receipt.result.status == camellia_nexus_core::ConfigurationOperationStatus::Pending {
                state.finish_operation(
                    &receipt.request.operation_id,
                    camellia_nexus_core::ConfigurationOperationStatus::Applied,
                    marker.state.generation,
                    None,
                );
            }
        }
        write_configuration_state_file(&state_path, &state)?;
        let lkg_name = match marker.state.format {
            ConfigurationFormat::Jsonc => CONFIGURATION_LKG_JSON,
            ConfigurationFormat::Yaml => CONFIGURATION_LKG_YAML,
        };
        write_bytes_atomic(
            &root.join(CONFIGURATION_STATE_DIRECTORY).join(lkg_name),
            marker.state.desired.content.as_bytes(),
        )?;
        if pending.exists() {
            fs::remove_file(&pending)?;
        }
        if let Some(parent) = target.parent() {
            sync_directory(parent)?;
        }
    } else if pending.exists() {
        // Prepared transactions must first restore their active-file backup.
        return Err(configuration_recovery_required());
    }
    fs::remove_file(root.join(CONFIGURATION_APPLY_MARKER))?;
    sync_directory(root)
}

fn load_configuration_workspace_transaction_marker(
    root: &Path,
) -> Result<Option<ConfigurationWorkspaceTransactionMarker>> {
    let path = root.join(CONFIGURATION_WORKSPACE_TRANSACTION_MARKER);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = read_with_overflow_byte(&path, PROGRAM_SPEC_MAX_BYTES * 2)?;
    if bytes.len() as u64 > PROGRAM_SPEC_MAX_BYTES * 2 {
        return Err(CamelliaNexusError::new(
            ErrorCode::Storage,
            "Configuration workspace transaction marker is oversized",
        ));
    }
    let marker: ConfigurationWorkspaceTransactionMarker =
        serde_json::from_slice(&bytes).map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::Storage,
                "Configuration workspace transaction marker is invalid",
            )
            .with_details(error.to_string())
        })?;
    Ok(Some(marker))
}

fn recover_configuration_workspace_transaction(root: &Path) -> Result<()> {
    let Some(marker) = load_configuration_workspace_transaction_marker(root)? else {
        return Ok(());
    };
    restore_configuration_workspace_files(root, &marker)?;
    for path in [
        root.join(CONFIGURATION_WORKSPACE_TRANSACTION_MARKER),
        root.join(CONFIGURATION_WORKSPACE_STATE_BACKUP),
    ] {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        sync_directory(root)?;
    }
    Ok(())
}

fn restore_configuration_workspace_files(
    root: &Path,
    marker: &ConfigurationWorkspaceTransactionMarker,
) -> Result<()> {
    let state_path = root
        .join(CONFIGURATION_STATE_DIRECTORY)
        .join(CONFIGURATION_STATE_FILE);
    let backup_path = root.join(CONFIGURATION_WORKSPACE_STATE_BACKUP);
    if !marker.state_committed {
        if marker.previous_state_present {
            if backup_path.exists() {
                let bytes = read_with_overflow_byte(&backup_path, CONFIGURATION_STATE_MAX_BYTES)?;
                if bytes.len() as u64 > CONFIGURATION_STATE_MAX_BYTES {
                    return Err(configuration_recovery_required());
                }
                decode_configuration_state(&bytes)?;
                write_bytes_atomic(&state_path, &bytes)?;
            } else {
                return Err(CamelliaNexusError::new(
                    ErrorCode::Storage,
                    "Configuration workspace transaction state backup is missing",
                )
                .with_message_key("CONFIGURATION_RECOVERY_REQUIRED"));
            }
        } else if state_path.exists() {
            fs::remove_file(&state_path)?;
        }
        write_json_atomic(&root.join("program.json"), &marker.previous_spec)?;
    } else {
        write_json_atomic(&root.join("program.json"), &marker.next_spec)?;
    }
    sync_directory(root)
}

fn workspace_commit_recovery(error: CamelliaNexusError) -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::Storage,
        "Configuration update needs to be reconciled",
    )
    .with_message_key("CONFIGURATION_WORKSPACE_COMMIT_RECOVERY_REQUIRED")
    .with_details(format!("Workspace recovery: {:?}", error.code))
}

fn load_program_package_transaction_marker(
    root: &Path,
) -> Result<Option<ProgramPackageTransactionMarker>> {
    let marker_path = root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER);
    if !marker_path.exists() {
        return Ok(None);
    }
    let bytes = read_with_overflow_byte(&marker_path, 64 * 1024)?;
    if bytes.len() > 64 * 1024 {
        return Err(CamelliaNexusError::new(
            ErrorCode::Storage,
            "Managed package transaction marker is oversized",
        )
        .with_message_key("PROGRAM_PACKAGE_RECOVERY_REQUIRED"));
    }
    let marker: ProgramPackageTransactionMarker =
        serde_json::from_slice(&bytes).map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::Storage,
                "Managed package transaction marker is invalid",
            )
            .with_message_key("PROGRAM_PACKAGE_RECOVERY_REQUIRED")
            .with_details(error.to_string())
        })?;
    Ok(Some(marker))
}

fn discard_directory_background(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Managed package cleanup target must be a real directory",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Managed package directory has no parent",
        )
    })?;
    let discarded = parent.join(format!("{DISCARDED_PACKAGE_PREFIX}{}", Uuid::new_v4()));
    fs::rename(path, &discarded)?;
    remove_directory_bounded(discarded);
    if let Err(error) = sync_directory(parent) {
        tracing::warn!(path = %parent.display(), %error, "could not sync discarded managed package directory");
    }
    Ok(())
}

fn clean_discarded_package_directories(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(DISCARDED_PACKAGE_PREFIX)
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            remove_directory_bounded(entry.path());
        }
    }
}

fn package_recovery_required(
    cause: &CamelliaNexusError,
    recovery: &CamelliaNexusError,
) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::Storage, "Program replacement requires recovery")
        .with_message_key("PROGRAM_PACKAGE_RECOVERY_REQUIRED")
        .with_details(format!(
            "commit: {:?}; recovery: {:?}",
            cause.code, recovery.code
        ))
}

fn rollback_program_package_transaction(
    root: &Path,
    marker: &ProgramPackageTransactionMarker,
) -> Result<()> {
    // Keep recovery copies until the Restored phase is durable, so interrupted rollback is repeatable.
    let spec_backup = root.join(PROGRAM_PACKAGE_SPEC_BACKUP);
    let bytes = read_with_overflow_byte(&spec_backup, PROGRAM_SPEC_MAX_BYTES)?;
    if bytes.len() as u64 > PROGRAM_SPEC_MAX_BYTES {
        return Err(configuration_recovery_required());
    }
    write_bytes_atomic(&root.join("program.json"), &bytes)?;
    if marker.workspace {
        let bytes = read_with_overflow_byte(
            &root.join(PROGRAM_PACKAGE_STATE_BACKUP),
            CONFIGURATION_STATE_MAX_BYTES,
        )?;
        if bytes.len() as u64 > CONFIGURATION_STATE_MAX_BYTES {
            return Err(configuration_recovery_required());
        }
        write_bytes_atomic(
            &root
                .join(CONFIGURATION_STATE_DIRECTORY)
                .join(CONFIGURATION_STATE_FILE),
            &bytes,
        )?;
    }
    let active = root.join("bin");
    let backup = root.join("bin.old");
    if backup.exists() {
        if active.exists() {
            discard_directory_background(&active)?;
        }
        fs::rename(&backup, &active)?;
    }
    sync_directory(root)?;
    write_json_atomic(
        &root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER),
        &ProgramPackageTransactionMarker {
            phase: ProgramPackageTransactionPhase::Restored,
            workspace: marker.workspace,
        },
    )?;
    cleanup_finished_program_package_transaction(root);
    Ok(())
}

fn cleanup_finished_program_package_transaction(root: &Path) {
    let cleanup = (|| -> Result<()> {
        for path in [
            root.join(PROGRAM_PACKAGE_SPEC_BACKUP),
            root.join(PROGRAM_PACKAGE_NEXT_SPEC),
            root.join(PROGRAM_PACKAGE_STATE_BACKUP),
            root.join(PROGRAM_PACKAGE_NEXT_STATE),
        ] {
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        for path in [root.join("bin.old"), root.join("bin.new")] {
            if path.exists() {
                discard_directory_background(&path)?;
            }
        }
        sync_directory(root)?;
        let marker = root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER);
        if marker.exists() {
            fs::remove_file(marker)?;
        }
        sync_directory(root)
    })();
    if let Err(error) = cleanup {
        tracing::warn!(code = ?error.code, "managed package cleanup will resume from its durable phase");
    }
}

fn recover_program_package_transaction(root: &Path) -> Result<()> {
    let Some(marker) = load_program_package_transaction_marker(root)? else {
        if root.join("bin.old").exists() {
            return Err(CamelliaNexusError::new(
                ErrorCode::Storage,
                "Package recovery phase is unavailable",
            )
            .with_message_key("PROGRAM_PACKAGE_RECOVERY_REQUIRED"));
        }
        // Preparation copies precede the durable marker; no active files have changed here.
        for file in [
            PROGRAM_PACKAGE_SPEC_BACKUP,
            PROGRAM_PACKAGE_NEXT_SPEC,
            PROGRAM_PACKAGE_STATE_BACKUP,
            PROGRAM_PACKAGE_NEXT_STATE,
        ] {
            let path = root.join(file);
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        sync_directory(root)?;
        return Ok(());
    };
    match marker.phase {
        ProgramPackageTransactionPhase::Committed | ProgramPackageTransactionPhase::Restored => {
            cleanup_finished_program_package_transaction(root);
            Ok(())
        }
        ProgramPackageTransactionPhase::Prepared => {
            rollback_program_package_transaction(root, &marker)
        }
    }
}

fn copy_package(source: &Path, target: &Path) -> Result<()> {
    if !source.is_dir() {
        return Err(CamelliaNexusError::invalid_spec(
            "Managed program source must be a directory",
        ));
    }
    if is_link_or_reparse(&fs::symlink_metadata(source)?) {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Managed program source cannot be a link or reparse point",
        ));
    }
    let canonical_source = fs::canonicalize(source)?;
    let target_parent = target.parent().ok_or_else(|| {
        CamelliaNexusError::new(ErrorCode::InvalidPath, "Program target has no parent")
    })?;
    let canonical_target_parent = fs::canonicalize(target_parent)?;
    if path_is_within(&canonical_target_parent, &canonical_source)
        || path_is_within(&canonical_source, &canonical_target_parent)
    {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Program source and managed workspace cannot contain each other",
        ));
    }
    let mut stack = vec![(source.to_path_buf(), target.to_path_buf())];
    let mut entries = 0usize;
    let mut bytes = 0u64;
    while let Some((from, to)) = stack.pop() {
        fs::create_dir_all(&to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            entries += 1;
            if entries > PACKAGE_MAX_ENTRIES {
                return Err(CamelliaNexusError::invalid_spec(
                    "Managed program directory exceeds 4096 entries",
                ));
            }
            let kind = entry.file_type()?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if is_link_or_reparse(&metadata) {
                return Err(CamelliaNexusError::new(
                    ErrorCode::InvalidPath,
                    "Managed program directory cannot contain links or reparse points",
                ));
            }
            let destination = to.join(entry.file_name());
            if kind.is_dir() {
                stack.push((entry.path(), destination));
            } else if kind.is_file() {
                bytes = bytes.saturating_add(metadata.len());
                if bytes > PACKAGE_MAX_BYTES {
                    return Err(CamelliaNexusError::invalid_spec(
                        "Managed program directory exceeds 512 MiB",
                    ));
                }
                fs::copy(entry.path(), destination)?;
            } else {
                return Err(CamelliaNexusError::new(
                    ErrorCode::InvalidPath,
                    "Managed program directory contains an unsupported file type",
                ));
            }
        }
    }
    Ok(())
}

fn executable_metadata(path: &Path) -> Result<ExecutableMetadata> {
    let metadata = fs::metadata(path).map_err(|error| {
        CamelliaNexusError::new(ErrorCode::UnsupportedBinary, "Executable is not readable")
            .with_details(error.to_string())
    })?;
    if !metadata.is_file() {
        return Err(CamelliaNexusError::new(
            ErrorCode::UnsupportedBinary,
            "Executable path is not a regular file",
        ));
    }
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_millis() as u64);
    let mut file = File::open(path).map_err(|error| {
        CamelliaNexusError::new(ErrorCode::UnsupportedBinary, "Executable is not readable")
            .with_details(error.to_string())
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::UnsupportedBinary,
                "Executable could not be fingerprinted",
            )
            .with_details(error.to_string())
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let after = fs::metadata(path).map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::UnsupportedBinary,
            "Executable metadata could not be read",
        )
        .with_details(error.to_string())
    })?;
    let after_modified_unix_ms = after
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_millis() as u64);
    if after.len() != metadata.len() || after_modified_unix_ms != modified_unix_ms {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigConflict,
            "Executable changed while its fingerprint was computed",
        ));
    }
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(ExecutableMetadata {
        fingerprint: CoreBinaryFingerprint {
            sha256,
            size: metadata.len(),
            modified_unix_ms,
        },
        probe: None,
        core_target: None,
    })
}

fn suffixed_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(not(windows))]
fn path_is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(windows)]
fn path_is_within(path: &Path, root: &Path) -> bool {
    let mut path_components = path.components();
    root.components().all(|root_component| {
        path_components.next().is_some_and(|path_component| {
            path_component
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&root_component.as_os_str().to_string_lossy())
        })
    })
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

fn write_json_atomic(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    write_bytes_atomic(path, &bytes)
}

pub(crate) fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CamelliaNexusError::new(ErrorCode::InvalidPath, "File has no parent"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".camellia-nexus-write-{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if let Err(error) = replace_file(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    sync_directory(parent)?;
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> Result<()> {
    fs::rename(source, target)?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> Result<()> {
    move_file_replace(source, target)
}

#[cfg(not(windows))]
fn replace_with_backup(source: &Path, target: &Path, backup: &Path) -> Result<()> {
    if backup.exists() {
        fs::remove_file(backup)?;
    }
    fs::rename(target, backup)?;
    if let Err(error) = fs::rename(source, target) {
        let _ = fs::rename(backup, target);
        return Err(error.into());
    }
    if let Some(parent) = target.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

#[cfg(windows)]
fn replace_with_backup(source: &Path, target: &Path, backup: &Path) -> Result<()> {
    if backup.exists() {
        fs::remove_file(backup)?;
    }
    move_file_replace(target, backup)?;
    if let Err(error) = move_file_replace(source, target) {
        let _ = move_file_replace(backup, target);
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn move_file_replace(source: &Path, target: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(target.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(CamelliaNexusError::storage)
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn read_tail(path: &Path, max_bytes: usize) -> Result<LogChunk> {
    if !path.exists() {
        return Ok(LogChunk {
            content: String::new(),
            truncated: false,
        });
    }
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(max_bytes as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(max_bytes as u64).read_to_end(&mut bytes)?;
    Ok(LogChunk {
        content: String::from_utf8_lossy(&bytes).into_owned(),
        truncated: start > 0,
    })
}

pub(crate) fn read_with_overflow_byte(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut bytes = Vec::with_capacity(file.metadata()?.len().min(max_bytes) as usize);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf, sync::mpsc};

    use camellia_nexus_core::{
        CandidateValidationStatus, ConfigStore, ConfigurationCandidate, ConfigurationDiagnostic,
        ConfigurationFormat, ConfigurationRevision, ConfigurationState, CoreBinaryFingerprint,
        CoreCompatibilityProfile, CoreTargetIdentity, CoreValidationEvidence, CreateAssets,
        ErrorCode, ExecutableSpec, FinalEditorSession, MAX_CONFIG_BYTES, ProgramId, ProgramKind,
        ProgramSpec, ProgramStore, ProgramType, RestartPolicy, SourceSnapshot,
        merge_configuration_sources,
    };

    use super::{
        CREATE_COMMITTED_MARKER, FileStore, PROGRAM_PACKAGE_NEXT_SPEC, PROGRAM_PACKAGE_SPEC_BACKUP,
        PROGRAM_PACKAGE_TRANSACTION_MARKER, ProgramPackageTransactionMarker,
        ProgramPackageTransactionPhase, enqueue_directory_cleanup, read_tail,
        read_with_overflow_byte, replace_with_backup, suffixed_path, write_bytes_atomic,
        write_json_atomic,
    };

    #[test]
    fn directory_cleanup_queue_returns_backpressure_at_capacity() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let first = PathBuf::from("first");
        let second = PathBuf::from("second");

        assert_eq!(enqueue_directory_cleanup(&sender, first.clone()), Ok(()));
        assert_eq!(
            enqueue_directory_cleanup(&sender, second.clone()),
            Err(second)
        );
        assert_eq!(receiver.try_recv(), Ok(first));
    }

    fn generic_spec() -> ProgramSpec {
        let executable = std::env::current_exe().expect("current test executable");
        let working_directory = executable
            .parent()
            .expect("executable parent")
            .to_path_buf();
        ProgramSpec {
            id: ProgramId::parse("fixture").expect("id"),
            name: "Fixture".into(),
            executable: ExecutableSpec::External {
                path: executable,
                metadata: None,
            },
            program_type: ProgramType::Generic { args: Vec::new() },
            managed_config: None,
            working_directory,
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        }
    }

    fn xray_profile() -> CoreCompatibilityProfile {
        let fingerprint = CoreBinaryFingerprint {
            sha256: "a".repeat(64),
            size: 1,
            modified_unix_ms: 1,
        };
        CoreCompatibilityProfile::resolve(
            &CoreTargetIdentity::unknown(ProgramKind::Xray, None).bind_fingerprint(&fingerprint),
        )
        .expect("profile")
    }

    fn xray_state(
        generation: u64,
        merge: camellia_nexus_core::SemanticMergeResult,
    ) -> ConfigurationState {
        ConfigurationState::from_merge(ProgramKind::Xray, generation, 1, merge, xray_profile())
            .expect("configuration state")
    }

    fn validation_evidence(state: &ConfigurationState) -> CoreValidationEvidence {
        CoreValidationEvidence {
            binary_sha256: state
                .compatibility_profile
                .target
                .fingerprint_sha256
                .clone()
                .expect("fingerprint"),
            profile_hash: state.compatibility_profile.profile_hash.clone(),
            config_hash: state.desired.revision.content_hash.clone(),
            candidate_generation: state.generation,
            validator_contract_revision: camellia_nexus_core::CORE_IMPLEMENTATION_REVISION.into(),
            native_accepted: true,
            validated_unix_ms: 1,
        }
    }

    fn configured_spec() -> ProgramSpec {
        let mut spec = generic_spec();
        spec.program_type = ProgramType::Xray {
            extra_args: Vec::new(),
        };
        spec
    }

    async fn managed_package_fixture(
        directory: &tempfile::TempDir,
    ) -> (FileStore, ProgramSpec, PathBuf) {
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir_all(&first).expect("first package");
        std::fs::create_dir_all(&second).expect("second package");
        std::fs::write(first.join("tool"), b"old").expect("old executable");
        std::fs::write(second.join("tool"), b"new-content").expect("new executable");
        let store = FileStore::new(directory.path().join("store")).expect("store");
        let mut spec = generic_spec();
        spec.executable = ExecutableSpec::Managed {
            path: PathBuf::from("bin/tool"),
            metadata: None,
        };
        spec.working_directory = PathBuf::from("bin");
        spec.validate().expect("managed spec");
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: Some(first),
                    initial_config: None,
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit create");
        (store, spec, second)
    }

    #[test]
    fn bounded_reads_never_return_more_than_the_requested_window() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("growing.log");
        std::fs::write(&path, b"0123456789").expect("log");

        let tail = read_tail(&path, 4).expect("tail");
        assert_eq!(tail.content, "6789");
        assert!(tail.truncated);
        assert_eq!(
            read_with_overflow_byte(&path, 4)
                .expect("overflow probe")
                .len(),
            5
        );
    }

    #[tokio::test]
    async fn creates_commits_and_loads_program() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = generic_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: None,
                },
            )
            .await
            .expect("create");
        let workspace = store.workspace(&spec.id).await.expect("workspace");
        for redundant in ["bin", "config", "data", "logs", "tmp"] {
            assert!(
                !workspace.join(redundant).exists(),
                "external program created redundant {redundant}"
            );
        }
        store.commit_create(&spec.id).await.expect("commit");
        store.save(&spec).await.expect("replace existing metadata");
        let report = store.load_all().await.expect("load");
        assert_eq!(report.valid.len(), 1);
        assert!(
            report.invalid.is_empty(),
            "{:?}; persisted={}",
            report.invalid,
            std::fs::read_to_string(workspace.join("program.json")).expect("persisted metadata")
        );
        assert_eq!(report.valid[0].spec.id, spec.id);
    }

    #[tokio::test]
    async fn startup_discards_an_uncommitted_program_creation() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = generic_spec();
        let workspace = store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: None,
                },
            )
            .await
            .expect("create pending");

        let report = store.load_all().await.expect("startup recovery");
        assert!(report.valid.is_empty());
        assert!(report.invalid.is_empty());
        assert!(!workspace.exists());
    }

    #[tokio::test]
    async fn startup_keeps_a_durable_program_creation_commit_point() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = generic_spec();
        let workspace = store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: None,
                },
            )
            .await
            .expect("create pending");
        write_bytes_atomic(&workspace.join(".pending"), CREATE_COMMITTED_MARKER)
            .expect("creation commit point");

        let report = store.load_all().await.expect("startup recovery");
        assert!(report.invalid.is_empty());
        assert_eq!(report.valid.len(), 1);
        assert_eq!(report.valid[0].spec.id, spec.id);
        assert!(!workspace.join(".pending").exists());
    }

    #[tokio::test]
    async fn clears_current_and_rotated_logs() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = generic_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: None,
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let workspace = store.workspace(&spec.id).await.expect("workspace");
        std::fs::write(workspace.join("stdout.log"), b"current output").expect("current");
        std::fs::write(workspace.join("stdout.log.1"), b"rotated output").expect("rotated");
        std::fs::write(workspace.join("stderr.log"), b"current error").expect("error");

        store.clear_logs(&spec).await.expect("clear logs");

        assert_eq!(
            std::fs::read(workspace.join("stdout.log")).expect("stdout"),
            b""
        );
        assert_eq!(
            std::fs::read(workspace.join("stderr.log")).expect("stderr"),
            b""
        );
        assert!(!workspace.join("stdout.log.1").exists());
    }

    #[tokio::test]
    async fn managed_package_preserves_nested_entry_paths() {
        let directory = tempfile::tempdir().expect("tempdir");
        let package = directory.path().join("package");
        std::fs::create_dir_all(package.join("bin")).expect("package tree");
        std::fs::write(package.join("bin/tool"), b"nested executable").expect("executable");
        std::fs::write(package.join("config.json"), b"{}").expect("sidecar");

        let store = FileStore::new(directory.path().join("store")).expect("store");
        let mut spec = generic_spec();
        spec.executable = ExecutableSpec::Managed {
            path: PathBuf::from("bin/bin/tool"),
            metadata: None,
        };
        spec.working_directory = PathBuf::from("bin/bin");
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: Some(package),
                    initial_config: None,
                },
            )
            .await
            .expect("create nested package");
        let workspace = store.workspace(&spec.id).await.expect("workspace");
        assert!(workspace.join("data").is_dir());
        assert!(workspace.join("logs").is_dir());
        assert!(!workspace.join("tmp").exists());
        assert_eq!(
            std::fs::read(workspace.join("bin/bin/tool")).expect("nested entry"),
            b"nested executable"
        );
        assert_eq!(
            spec.working_directory_path(&workspace),
            workspace.join("bin/bin")
        );
        assert!(spec.working_directory_path(&workspace).is_dir());
        assert_eq!(
            std::fs::read(workspace.join("bin/config.json")).expect("sidecar"),
            b"{}"
        );
    }

    #[tokio::test]
    async fn invalid_workspace_does_not_block_valid_programs() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = generic_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: None,
                },
            )
            .await
            .expect("create valid");
        store.commit_create(&spec.id).await.expect("commit valid");
        let invalid = directory.path().join("programs/broken");
        std::fs::create_dir_all(&invalid).expect("invalid directory");
        std::fs::write(invalid.join("program.json"), b"{").expect("invalid spec");

        let report = store.load_all().await.expect("load report");
        assert_eq!(report.valid.len(), 1);
        assert_eq!(report.invalid.len(), 1);
    }

    #[tokio::test]
    async fn invalid_workspace_name_is_rejected_before_transaction_recovery() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let workspace = directory.path().join("programs/INVALID");
        let backup_package = workspace.join("bin.old");
        std::fs::create_dir_all(&backup_package).expect("backup package");
        std::fs::write(backup_package.join("sentinel"), b"preserve").expect("sentinel");
        write_json_atomic(&workspace.join("program.json"), &generic_spec())
            .expect("program metadata");
        write_json_atomic(
            &workspace.join(PROGRAM_PACKAGE_TRANSACTION_MARKER),
            &ProgramPackageTransactionMarker {
                phase: ProgramPackageTransactionPhase::Committed,
                workspace: false,
            },
        )
        .expect("committed marker");

        let report = store.load_all().await.expect("load report");

        assert!(report.valid.is_empty());
        assert_eq!(report.invalid.len(), 1);
        assert!(backup_package.join("sentinel").is_file());
        assert!(workspace.join(PROGRAM_PACKAGE_TRANSACTION_MARKER).is_file());
    }

    #[tokio::test]
    async fn load_rejects_noncanonical_runtime_directories() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = generic_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: None,
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");

        let spec_path = directory.path().join("programs/fixture/program.json");
        let mut stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&spec_path).expect("read spec"))
                .expect("parse spec");
        stored["workingDirectory"] = serde_json::Value::String(".".to_owned());
        std::fs::write(
            &spec_path,
            serde_json::to_vec(&stored).expect("serialize spec"),
        )
        .expect("write invalid spec");

        let report = store.load_all().await.expect("load report");
        assert!(report.valid.is_empty());
        assert_eq!(report.invalid.len(), 1);
        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(spec_path).expect("read persisted spec"))
                .expect("parse persisted spec");
        assert_eq!(persisted["workingDirectory"], ".");
    }

    #[tokio::test]
    async fn config_replace_keeps_backup_and_restores_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let mut spec = generic_spec();
        spec.program_type = ProgramType::Xray {
            extra_args: Vec::new(),
        };
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"old":true}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let base_hash = store.current_hash(&spec).await.expect("base hash");
        let staged = store.stage(&spec, br#"{"new":true}"#).await.expect("stage");
        store
            .atomic_replace_with_backup(staged, &base_hash)
            .await
            .expect("replace");
        assert!(
            store
                .load(&spec)
                .await
                .expect("load")
                .content
                .contains("new")
        );
        store.restore_backup(&spec).await.expect("restore");
        assert!(
            store
                .load(&spec)
                .await
                .expect("load")
                .content
                .contains("old")
        );
        let target = store.config_path(&spec).expect("config path");
        assert!(!suffixed_path(&target, ".failed").exists());
    }

    #[tokio::test]
    async fn pending_config_replace_recovers_old_content_after_a_crash() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let mut spec = generic_spec();
        spec.program_type = ProgramType::Xray {
            extra_args: Vec::new(),
        };
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"old":true}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let base_hash = store.current_hash(&spec).await.expect("base hash");
        let staged = store.stage(&spec, br#"{"new":true}"#).await.expect("stage");
        store
            .atomic_replace_with_backup(staged, &base_hash)
            .await
            .expect("replace");

        store.recover(&spec).await.expect("recover pending replace");
        assert!(
            store
                .load(&spec)
                .await
                .expect("load")
                .content
                .contains("old")
        );
    }

    #[tokio::test]
    async fn finalized_config_replace_survives_recovery() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let mut spec = generic_spec();
        spec.program_type = ProgramType::Xray {
            extra_args: Vec::new(),
        };
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"old":true}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let base_hash = store.current_hash(&spec).await.expect("base hash");
        let staged = store.stage(&spec, br#"{"new":true}"#).await.expect("stage");
        store
            .atomic_replace_with_backup(staged, &base_hash)
            .await
            .expect("replace");
        store.finalize_replace(&spec).await.expect("finalize");

        let target = store.config_path(&spec).expect("config path");
        assert!(!suffixed_path(&target, ".pending").exists());
        assert!(!suffixed_path(&target, ".bak").exists());

        store
            .recover(&spec)
            .await
            .expect("recover finalized replace");
        assert!(
            store
                .load(&spec)
                .await
                .expect("load")
                .content
                .contains("new")
        );
    }

    #[tokio::test]
    async fn configuration_apply_marker_recovers_state_after_active_commit() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"old":true}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");

        let snapshot = SourceSnapshot::parse(
            "manual",
            "Manual",
            ConfigurationFormat::Jsonc,
            br#"{"new":true}"#,
            1,
            false,
        )
        .expect("snapshot");
        let merge = merge_configuration_sources(ProgramKind::Xray, &[snapshot]).expect("merge");
        let mut state = xray_state(2, merge);
        state.mark_candidate_saved().expect("save candidate");
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("validation");
        store
            .begin_configuration_apply(&spec.id, &state)
            .await
            .expect("begin apply");
        let hash = store.current_hash(&spec).await.unwrap();
        let staged = store
            .stage(&spec, state.desired.content.as_bytes())
            .await
            .unwrap();
        store
            .atomic_replace_with_backup(staged, &hash)
            .await
            .unwrap();

        super::mark_configuration_apply_committed(&store.program_root(&spec.id), &spec)
            .expect("durable commit phase");

        assert!(suffixed_path(&store.config_path(&spec).unwrap(), ".pending").exists());

        store.recover(&spec).await.expect("recover apply");
        assert!(store.load(&spec).await.unwrap().content.contains("new"));

        let recovered = store
            .load_configuration_state(&spec.id)
            .await
            .expect("load state")
            .expect("recovered state");
        assert_eq!(
            recovered
                .applied
                .as_ref()
                .map(|candidate| &candidate.revision),
            Some(&recovered.desired.revision)
        );
        assert!(recovered.last_known_good.is_some());
        assert!(
            !store
                .program_root(&spec.id)
                .join(super::CONFIGURATION_APPLY_MARKER)
                .exists()
        );
    }

    #[tokio::test]
    async fn prepared_apply_never_infers_commit_from_matching_active_content() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileStore::new(directory.path().to_owned()).unwrap();
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(b"{}".to_vec()),
                },
            )
            .await
            .unwrap();
        store.commit_create(&spec.id).await.unwrap();
        let snapshot = SourceSnapshot::parse(
            "source",
            "Source",
            ConfigurationFormat::Jsonc,
            b"{}",
            1,
            false,
        )
        .unwrap();
        let mut state = xray_state(
            1,
            merge_configuration_sources(ProgramKind::Xray, &[snapshot]).unwrap(),
        );
        state.mark_candidate_saved().unwrap();
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .unwrap();
        store
            .save_configuration_state(&spec.id, &state, None)
            .await
            .unwrap();
        store
            .begin_configuration_apply(&spec.id, &state)
            .await
            .unwrap();
        store.recover(&spec).await.unwrap();
        let recovered = store
            .load_configuration_state(&spec.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered, state);
        assert!(recovered.applied.is_none());
        assert!(recovered.last_known_good.is_none());
    }

    #[tokio::test]
    async fn configuration_state_save_uses_state_revision_compare_and_swap() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"log":{"loglevel":"info"}}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let snapshot = SourceSnapshot::parse(
            "manual",
            "Manual",
            ConfigurationFormat::Jsonc,
            br#"{"log":{"loglevel":"info"}}"#,
            1,
            false,
        )
        .expect("snapshot");
        let merge = merge_configuration_sources(ProgramKind::Xray, &[snapshot]).expect("merge");
        let state = xray_state(1, merge);
        store
            .save_configuration_state(&spec.id, &state, None)
            .await
            .expect("initial state");
        let mut next = state.clone();
        next.generation = 2;
        next.state_revision = 2;
        next.desired.revision.generation = 2;
        store
            .save_configuration_state(&spec.id, &next, Some(1))
            .await
            .expect("new generation");
        let mut stale = state;
        stale.generation = 3;
        stale.state_revision = 3;
        stale.desired.revision.generation = 3;
        let error = store
            .save_configuration_state(&spec.id, &stale, Some(1))
            .await
            .expect_err("stale generation must be rejected");
        assert_eq!(error.code, ErrorCode::ConfigConflict);
        assert_eq!(
            store
                .load_configuration_state(&spec.id)
                .await
                .expect("load")
                .expect("state")
                .generation,
            2
        );
    }

    #[tokio::test]
    async fn configuration_state_requires_the_current_contract() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"log":{"loglevel":"info"}}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let snapshot = SourceSnapshot::parse(
            "manual",
            "Manual",
            ConfigurationFormat::Jsonc,
            br#"{"log":{"loglevel":"info"}}"#,
            1,
            false,
        )
        .expect("snapshot");
        let merge = merge_configuration_sources(ProgramKind::Xray, &[snapshot]).expect("merge");
        let state = xray_state(1, merge);
        let mut value = serde_json::to_value(state).expect("state value");
        value
            .as_object_mut()
            .expect("state object")
            .remove("finalEdit");
        write_json_atomic(&store.configuration_state_path(&spec.id), &value).expect("write state");

        let error = store
            .load_configuration_state(&spec.id)
            .await
            .expect_err("invalid state contract");

        assert_eq!(error.code, ErrorCode::ConfigInvalid);
        assert_eq!(error.message, "Configuration state is invalid");
    }

    #[tokio::test]
    async fn last_known_good_round_trips_for_workspace_recreation() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"log":{"loglevel":"info"}}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let content = r#"{"log":{"loglevel":"warning"}}"#;

        store
            .save_last_known_good(&spec.id, ConfigurationFormat::Jsonc, content)
            .await
            .expect("save lkg");

        assert_eq!(
            store
                .load_last_known_good(&spec.id, ConfigurationFormat::Jsonc)
                .await
                .expect("load lkg")
                .as_deref(),
            Some(content)
        );
    }

    #[tokio::test]
    async fn configuration_state_sidecar_round_trip_hydrates_and_compacts_state() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"old":true}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let snapshot = SourceSnapshot::parse(
            "source",
            "Source",
            ConfigurationFormat::Jsonc,
            br#"{"large":"value"}"#,
            1,
            false,
        )
        .expect("snapshot");
        let merge = merge_configuration_sources(ProgramKind::Xray, std::slice::from_ref(&snapshot))
            .expect("merge");
        let mut state = xray_state(1, merge);
        state
            .source_snapshots
            .insert(snapshot.source_id.clone(), snapshot.clone());
        store
            .save_configuration_state(&spec.id, &state, None)
            .await
            .expect("save");
        let state_path = store.configuration_state_path(&spec.id);
        let persisted = std::fs::read_to_string(&state_path).expect("state file");
        let persisted_value: serde_json::Value =
            serde_json::from_str(&persisted).expect("persisted state json");
        assert_eq!(
            persisted_value["sourceSnapshots"]["source"]["content"],
            serde_json::Value::String(String::new())
        );
        let loaded = store
            .load_configuration_state(&spec.id)
            .await
            .expect("load")
            .expect("state");
        assert_eq!(loaded.source_snapshots["source"].content, snapshot.content);

        let mut marker_state = state;
        marker_state.generation = 2;
        marker_state.desired.revision.generation = 2;
        store
            .begin_configuration_apply(&spec.id, &marker_state)
            .await
            .expect("marker");
        let marker = std::fs::read_to_string(
            store
                .program_root(&spec.id)
                .join(super::CONFIGURATION_APPLY_MARKER),
        )
        .expect("marker file");
        let marker_value: serde_json::Value = serde_json::from_str(&marker).expect("marker json");
        assert_eq!(
            marker_value["state"]["sourceSnapshots"]["source"]["content"],
            serde_json::Value::String(String::new())
        );
    }

    #[tokio::test]
    async fn final_editor_draft_initial_revision_can_be_autosaved_once() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let snapshot = SourceSnapshot::parse(
            "source",
            "Source",
            ConfigurationFormat::Jsonc,
            b"{}",
            1,
            false,
        )
        .unwrap();
        let state = xray_state(
            1,
            merge_configuration_sources(ProgramKind::Xray, &[snapshot]).unwrap(),
        );
        store
            .save_configuration_state(&spec.id, &state, None)
            .await
            .unwrap();
        let draft = FinalEditorSession {
            session_id: "session".into(),
            draft_revision: 1,
            based_on_state_revision: 1,
            based_on_candidate_generation: 1,
            base_content: "{}".into(),
            working_content: "{}".into(),
            conflicts: Vec::new(),
            resolutions: BTreeMap::new(),
            unresolved_conflict_ids: Vec::new(),
            rebase_required: false,
            updated_unix_ms: 1,
        };
        store
            .save_final_editor_draft(&spec.id, &draft, Some(0))
            .await
            .expect("initial autosave");
        let second = store
            .save_final_editor_draft(&spec.id, &draft, Some(0))
            .await
            .expect_err("repeated initial revision must conflict");
        assert_eq!(second.code, ErrorCode::ConfigConflict);
    }

    #[tokio::test]
    async fn stale_final_editor_discard_preserves_the_current_draft() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let snapshot = SourceSnapshot::parse(
            "source",
            "Source",
            ConfigurationFormat::Jsonc,
            b"{}",
            1,
            false,
        )
        .unwrap();
        let state = xray_state(
            1,
            merge_configuration_sources(ProgramKind::Xray, &[snapshot]).unwrap(),
        );
        store
            .save_configuration_state(&spec.id, &state, None)
            .await
            .unwrap();
        let mut draft = FinalEditorSession {
            session_id: "current-session".into(),
            draft_revision: 2,
            based_on_state_revision: 1,
            based_on_candidate_generation: 1,
            base_content: "{}".into(),
            working_content: r#"{"current":true}"#.into(),
            conflicts: Vec::new(),
            resolutions: BTreeMap::new(),
            unresolved_conflict_ids: Vec::new(),
            rebase_required: false,
            updated_unix_ms: 2,
        };
        store
            .save_final_editor_draft(&spec.id, &draft, Some(0))
            .await
            .expect("save current draft");
        draft.based_on_state_revision = 2;

        let error = store
            .discard_final_editor_draft(&spec.id, 1)
            .await
            .expect_err("stale discard must fail");

        assert_eq!(error.code, ErrorCode::ConfigConflict);
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_DRAFT_STALE")
        );
        assert_eq!(
            store
                .load_final_editor_draft(&spec.id)
                .await
                .expect("load draft"),
            Some(draft)
        );
    }

    #[tokio::test]
    async fn workspace_rollback_reenters_after_restoring_files_before_cleanup() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let snapshot = SourceSnapshot::parse(
            "source",
            "Source",
            ConfigurationFormat::Jsonc,
            br#"{}"#,
            1,
            false,
        )
        .expect("snapshot");
        let merge = merge_configuration_sources(ProgramKind::Xray, &[snapshot]).expect("merge");
        let state = xray_state(4, merge);
        store
            .save_configuration_state(&spec.id, &state, None)
            .await
            .expect("state");
        let mut next_spec = spec.clone();
        next_spec.name = "new source list".into();
        store
            .begin_configuration_workspace_update(&spec.id, &spec, &next_spec, state.state_revision)
            .await
            .expect("begin");
        write_json_atomic(
            &store.program_root(&spec.id).join("program.json"),
            &next_spec,
        )
        .expect("simulate spec commit");
        let mut changed = state.clone();
        changed.generation = 5;
        changed.desired.revision.generation = 5;
        store
            .save_configuration_state(&spec.id, &changed, None)
            .await
            .expect("simulate state write");

        let root = store.program_root(&spec.id);
        let marker = super::load_configuration_workspace_transaction_marker(&root)
            .unwrap()
            .unwrap();
        for _ in 0..2 {
            super::restore_configuration_workspace_files(&root, &marker).unwrap();
            assert!(
                root.join(super::CONFIGURATION_WORKSPACE_STATE_BACKUP)
                    .exists()
            );
            assert!(
                root.join(super::CONFIGURATION_WORKSPACE_TRANSACTION_MARKER)
                    .exists()
            );
            assert_eq!(
                store.load_configuration_state(&spec.id).await.unwrap(),
                Some(state.clone())
            );
        }
        store.load_all().await.expect("recover");
        assert_eq!(store.load_all().await.expect("load").valid[0].spec, spec);
        assert_eq!(
            store
                .load_configuration_state(&spec.id)
                .await
                .expect("load state")
                .expect("state")
                .generation,
            state.generation
        );
        assert!(
            !store
                .program_root(&spec.id)
                .join(super::CONFIGURATION_WORKSPACE_TRANSACTION_MARKER)
                .exists()
        );
    }

    #[tokio::test]
    async fn source_transaction_recovery_keeps_the_committed_spec_and_state() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let snapshot = SourceSnapshot::parse(
            "source",
            "Source",
            ConfigurationFormat::Jsonc,
            br#"{}"#,
            1,
            false,
        )
        .expect("snapshot");
        let merge = merge_configuration_sources(ProgramKind::Xray, &[snapshot]).expect("merge");
        let state = xray_state(7, merge);
        store
            .save_configuration_state(&spec.id, &state, None)
            .await
            .expect("state");
        let mut next_spec = spec.clone();
        next_spec.name = "committed source list".into();
        store
            .begin_configuration_workspace_update(&spec.id, &spec, &next_spec, state.state_revision)
            .await
            .expect("begin");
        write_json_atomic(
            &store.program_root(&spec.id).join("program.json"),
            &next_spec,
        )
        .expect("simulate spec commit");
        let mut changed = state;
        changed.generation = 8;
        changed.desired.revision.generation = 8;
        store
            .save_configuration_state(&spec.id, &changed, None)
            .await
            .expect("simulate state commit");
        store
            .mark_configuration_workspace_update_committed(&spec.id, &next_spec)
            .await
            .expect("mark committed");

        let report = store.load_all().await.expect("recover");
        assert_eq!(report.valid[0].spec, next_spec);
        assert_eq!(
            store
                .load_configuration_state(&spec.id)
                .await
                .expect("load state")
                .expect("state")
                .generation,
            8
        );
    }

    #[tokio::test]
    async fn workspace_transaction_rejects_stale_revision_before_writing_recovery_evidence() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let snapshot = SourceSnapshot::parse(
            "source",
            "Source",
            ConfigurationFormat::Jsonc,
            br#"{}"#,
            1,
            false,
        )
        .expect("snapshot");
        let merge = merge_configuration_sources(ProgramKind::Xray, &[snapshot]).expect("merge");
        let state = xray_state(4, merge);
        store
            .save_configuration_state(&spec.id, &state, None)
            .await
            .expect("state");
        let mut next_spec = spec.clone();
        next_spec.name = "stale source update".into();

        let error = store
            .begin_configuration_workspace_update(&spec.id, &spec, &next_spec, 3)
            .await
            .expect_err("stale revision must not begin");

        assert_eq!(error.code, ErrorCode::ConfigConflict);
        let mut stale_spec = spec.clone();
        stale_spec.name = "concurrent details".into();
        let error = store
            .begin_configuration_workspace_update(
                &spec.id,
                &stale_spec,
                &next_spec,
                state.state_revision,
            )
            .await
            .expect_err("matching candidate state cannot overwrite different program settings");
        assert_eq!(
            error.message_key.as_deref(),
            Some("CONFIGURATION_STATE_STALE")
        );
        let root = store.program_root(&spec.id);
        assert!(
            !root
                .join(super::CONFIGURATION_WORKSPACE_TRANSACTION_MARKER)
                .exists()
        );
        assert!(
            !root
                .join(super::CONFIGURATION_WORKSPACE_STATE_BACKUP)
                .exists()
        );
    }

    #[tokio::test]
    async fn source_transaction_rollback_keeps_marker_when_state_backup_is_missing() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let snapshot = SourceSnapshot::parse(
            "source",
            "Source",
            ConfigurationFormat::Jsonc,
            br#"{}"#,
            1,
            false,
        )
        .expect("snapshot");
        let merge = merge_configuration_sources(ProgramKind::Xray, &[snapshot]).expect("merge");
        let state = xray_state(4, merge);
        store
            .save_configuration_state(&spec.id, &state, None)
            .await
            .expect("state");
        let mut next_spec = spec.clone();
        next_spec.name = "source update with lost backup".into();
        store
            .begin_configuration_workspace_update(&spec.id, &spec, &next_spec, state.state_revision)
            .await
            .expect("begin");
        let root = store.program_root(&spec.id);
        std::fs::remove_file(root.join(super::CONFIGURATION_WORKSPACE_STATE_BACKUP))
            .expect("remove backup to simulate storage failure");

        let error = store
            .rollback_configuration_workspace_update(&spec.id)
            .await
            .expect_err("missing backup must block rollback");

        assert_eq!(error.code, ErrorCode::Storage);
        assert!(
            root.join(super::CONFIGURATION_WORKSPACE_TRANSACTION_MARKER)
                .exists()
        );
        assert_eq!(
            store.load_all().await.expect("load report").invalid.len(),
            1
        );
        assert!(
            root.join(super::CONFIGURATION_WORKSPACE_TRANSACTION_MARKER)
                .exists()
        );
    }

    #[tokio::test]
    async fn invalid_candidate_persists_without_replacing_applied_or_lkg() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"valid":true}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");
        let snapshot = SourceSnapshot::parse(
            "manual",
            "Manual",
            ConfigurationFormat::Jsonc,
            br#"{"valid":true}"#,
            1,
            false,
        )
        .expect("snapshot");
        let merge = merge_configuration_sources(ProgramKind::Xray, &[snapshot]).expect("merge");
        let mut state = xray_state(1, merge);
        state.mark_candidate_saved().expect("save candidate");
        state
            .mark_validation(true, Vec::new(), Some(validation_evidence(&state)))
            .expect("validation");
        state.mark_applied().expect("initial apply");
        let applied = state.applied.clone();
        state.generation = 2;
        state.desired = ConfigurationCandidate {
            revision: ConfigurationRevision::new(2, "not-json", 2),
            content: "not-json".into(),
            compatibility_profile_hash: state.compatibility_profile.profile_hash.clone(),
            validation: CandidateValidationStatus::Invalid,
            validation_evidence: None,
            diagnostics: vec![ConfigurationDiagnostic {
                location: None,
                code: "CONFIGURATION_INVALID".into(),
                message: "Final candidate is not valid JSON".into(),
                message_key: Some("CONFIGURATION_INVALID".into()),
                scope: camellia_nexus_core::ConfigurationIssueScope::configuration(),
                details: None,
            }],
            conflicts: Vec::new(),
        };

        store
            .save_configuration_state(&spec.id, &state, None)
            .await
            .expect("persist invalid desired");

        let recovered = store
            .load_configuration_state(&spec.id)
            .await
            .expect("load state")
            .expect("state");
        assert_eq!(
            recovered.desired.validation,
            CandidateValidationStatus::Invalid
        );
        assert_eq!(recovered.desired.content, "not-json");
        assert_eq!(
            recovered.desired.diagnostics[0].code,
            "CONFIGURATION_INVALID"
        );
        assert_eq!(recovered.applied, applied);
        assert_eq!(recovered.last_known_good, applied);
    }

    #[tokio::test]
    async fn configuration_apply_recovery_preserves_a_newer_desired_generation() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"old":true}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");

        let snapshot = SourceSnapshot::parse(
            "manual",
            "Manual",
            ConfigurationFormat::Jsonc,
            br#"{"new":true}"#,
            1,
            false,
        )
        .expect("snapshot");
        let merge = merge_configuration_sources(ProgramKind::Xray, &[snapshot]).expect("merge");
        let mut applying = xray_state(2, merge);
        applying.mark_candidate_saved().expect("save candidate");
        applying
            .mark_validation(true, Vec::new(), Some(validation_evidence(&applying)))
            .expect("validation");
        store
            .begin_configuration_apply(&spec.id, &applying)
            .await
            .expect("begin apply");
        std::fs::write(
            store.config_path(&spec).expect("config path"),
            applying.desired.content.as_bytes(),
        )
        .expect("simulate active commit");

        super::mark_configuration_apply_committed(&store.program_root(&spec.id), &spec)
            .expect("durable commit phase");

        let applied_revision = applying.desired.revision.clone();
        let mut newer = applying;
        newer
            .guided_intent
            .set("logging.level", serde_json::Value::String("debug".into()));
        newer.rebuild_desired(2).expect("newer desired");
        let newer_revision = newer.desired.revision.clone();
        store
            .save_configuration_state(&spec.id, &newer, None)
            .await
            .expect("external newer state");

        store.recover(&spec).await.expect("recover apply");

        let recovered = store
            .load_configuration_state(&spec.id)
            .await
            .expect("load state")
            .expect("recovered state");
        assert_eq!(recovered.generation, newer_revision.generation);
        assert_eq!(recovered.desired.revision, newer_revision);
        assert_eq!(
            recovered
                .applied
                .as_ref()
                .map(|candidate| &candidate.revision),
            Some(&applied_revision)
        );
        assert_eq!(
            recovered
                .last_known_good
                .as_ref()
                .map(|candidate| &candidate.revision),
            Some(&applied_revision)
        );
    }

    #[tokio::test]
    async fn prepared_config_cas_preserves_an_external_edit() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let spec = configured_spec();
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(br#"{"old":true}"#.to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");

        let base_hash = store.current_hash(&spec).await.expect("base hash");
        let staged = store
            .stage(&spec, br#"{"prepared":true}"#)
            .await
            .expect("stage");
        let target = store.config_path(&spec).expect("config path");
        std::fs::write(&target, br#"{"external":true}"#).expect("external edit");

        let error = store
            .atomic_replace_with_backup(staged.clone(), &base_hash)
            .await
            .expect_err("conflicting replace");
        assert_eq!(error.code, ErrorCode::ConfigConflict);
        assert_eq!(
            std::fs::read_to_string(target).expect("active config"),
            r#"{"external":true}"#
        );
        store.discard_staged(staged).await.expect("discard");
    }

    #[tokio::test]
    async fn staged_config_enforces_limits_before_and_after_external_tools() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let mut spec = generic_spec();
        spec.program_type = ProgramType::Xray {
            extra_args: Vec::new(),
        };
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: None,
                    initial_config: Some(b"{}".to_vec()),
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit");

        let oversized = vec![b' '; MAX_CONFIG_BYTES + 1];
        assert!(store.stage(&spec, &oversized).await.is_err());

        let staged = store.stage(&spec, b"{}").await.expect("stage");
        std::fs::write(&staged.path, &oversized).expect("simulate external formatter");
        assert!(store.read_staged(&staged).await.is_err());
        store.discard_staged(staged).await.expect("discard");
    }

    #[tokio::test]
    async fn managed_package_replacement_is_staged_then_committed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::create_dir_all(&first).expect("first package");
        std::fs::create_dir_all(&second).expect("second package");
        std::fs::write(first.join("tool"), b"old").expect("old executable");
        std::fs::write(second.join("tool"), b"new-content").expect("new executable");

        let store_root = directory.path().join("store");
        let store = FileStore::new(store_root).expect("store");
        let mut spec = generic_spec();
        spec.executable = ExecutableSpec::Managed {
            path: PathBuf::from("bin/tool"),
            metadata: None,
        };
        store
            .create_pending(
                &spec,
                CreateAssets {
                    package_source: Some(first),
                    initial_config: None,
                },
            )
            .await
            .expect("create");
        store.commit_create(&spec.id).await.expect("commit create");

        let staged = store
            .stage_package(&spec, &second)
            .await
            .expect("stage package");
        assert_eq!(
            std::fs::read(&staged.executable).expect("staged"),
            b"new-content"
        );
        let mut next = spec.clone();
        next.executable.set_metadata(staged.metadata.clone());
        store
            .commit_package(staged, &spec, &next, None)
            .await
            .expect("commit package");
        let workspace = store.workspace(&spec.id).await.expect("workspace");
        assert_eq!(
            std::fs::read(workspace.join("bin/tool")).expect("active"),
            b"new-content"
        );
        assert!(!workspace.join("bin.old").exists());
        assert!(!workspace.join("bin.new").exists());
    }

    #[tokio::test]
    async fn pending_managed_package_transaction_restores_binary_and_metadata() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (store, spec, second) = managed_package_fixture(&directory).await;
        let staged = store
            .stage_package(&spec, &second)
            .await
            .expect("stage package");
        let workspace = store.workspace(&spec.id).await.expect("workspace");
        let mut next = spec.clone();
        next.name = "Prepared package".into();
        let next_spec_path = workspace.join(PROGRAM_PACKAGE_NEXT_SPEC);
        write_json_atomic(&next_spec_path, &next).expect("next metadata");
        write_json_atomic(
            &workspace.join(PROGRAM_PACKAGE_TRANSACTION_MARKER),
            &ProgramPackageTransactionMarker {
                phase: ProgramPackageTransactionPhase::Prepared,
                workspace: false,
            },
        )
        .expect("pending marker");
        replace_with_backup(
            &next_spec_path,
            &workspace.join("program.json"),
            &workspace.join(PROGRAM_PACKAGE_SPEC_BACKUP),
        )
        .expect("swap metadata");
        std::fs::rename(workspace.join("bin"), workspace.join("bin.old")).expect("backup package");
        std::fs::rename(staged.staged_directory, workspace.join("bin")).expect("swap package");

        let report = store.load_all().await.expect("startup recovery");
        assert!(report.invalid.is_empty(), "{:?}", report.invalid);
        assert_eq!(report.valid[0].spec.name, spec.name);
        assert_eq!(
            std::fs::read(workspace.join("bin/tool")).expect("active package"),
            b"old"
        );
        assert!(!workspace.join(PROGRAM_PACKAGE_TRANSACTION_MARKER).exists());
        assert!(!workspace.join("bin.old").exists());
    }

    #[tokio::test]
    async fn package_failure_after_swap_restores_files_and_can_retry() {
        let directory = tempfile::tempdir().unwrap();
        let (store, spec, source) = managed_package_fixture(&directory).await;
        let root = store.workspace(&spec.id).await.unwrap();
        for stage in [1, 2] {
            let staged = store.stage_package(&spec, &source).await.unwrap();
            let mut next = spec.clone();
            next.executable.set_metadata(staged.metadata.clone());
            store.fail_package_commit_at(stage);
            assert_eq!(
                store
                    .commit_package(staged, &spec, &next, None)
                    .await
                    .unwrap_err()
                    .code,
                ErrorCode::Storage
            );
            assert_eq!(std::fs::read(root.join("bin/tool")).unwrap(), b"old");
            assert_eq!(store.load_all().await.unwrap().valid[0].spec, spec);
        }
        let staged = store.stage_package(&spec, &source).await.unwrap();
        let mut next = spec.clone();
        next.executable.set_metadata(staged.metadata.clone());
        store
            .commit_package(staged, &spec, &next, None)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(root.join("bin/tool")).unwrap(),
            b"new-content"
        );
    }

    #[tokio::test]
    async fn package_workspace_restart_recovers_the_durable_commit_phase() {
        for phase in [
            ProgramPackageTransactionPhase::Prepared,
            ProgramPackageTransactionPhase::Committed,
            ProgramPackageTransactionPhase::Restored,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let (store, spec, source) = managed_package_fixture(&directory).await;
            let root = store.workspace(&spec.id).await.unwrap();
            let staged = store.stage_package(&spec, &source).await.unwrap();
            let source_snapshot = SourceSnapshot::parse(
                "source",
                "Source",
                ConfigurationFormat::Jsonc,
                br#"{"log":{"loglevel":"info"}}"#,
                1,
                false,
            )
            .unwrap();
            let mut before = xray_state(
                1,
                merge_configuration_sources(ProgramKind::Xray, &[source_snapshot]).unwrap(),
            );
            before.applied = Some(before.desired.clone());
            before.last_known_good = before.applied.clone();
            store
                .save_configuration_state(&spec.id, &before, None)
                .await
                .unwrap();
            let state_path = root
                .join(super::CONFIGURATION_STATE_DIRECTORY)
                .join(super::CONFIGURATION_STATE_FILE);
            let state_bytes = std::fs::read(&state_path).unwrap();
            let mut after = before.clone();
            after
                .guided_intent
                .set("logging.level", serde_json::json!("debug"));
            after.rebuild_desired(2).unwrap();
            store
                .save_configuration_state(&spec.id, &after, Some(before.state_revision))
                .await
                .unwrap();
            let mut next = spec.clone();
            next.name = "New package".into();
            write_bytes_atomic(
                &root.join(PROGRAM_PACKAGE_SPEC_BACKUP),
                &std::fs::read(root.join("program.json")).unwrap(),
            )
            .unwrap();
            write_bytes_atomic(
                &root.join(super::PROGRAM_PACKAGE_STATE_BACKUP),
                &state_bytes,
            )
            .unwrap();
            write_json_atomic(&root.join("program.json"), &next).unwrap();
            std::fs::rename(root.join("bin"), root.join("bin.old")).unwrap();
            std::fs::rename(staged.staged_directory, root.join("bin")).unwrap();
            if phase == ProgramPackageTransactionPhase::Restored {
                // Simulate completed restoration interrupted during cleanup of its copies.
                write_json_atomic(&root.join("program.json"), &spec).unwrap();
                write_bytes_atomic(&state_path, &state_bytes).unwrap();
                super::discard_directory_background(&root.join("bin")).unwrap();
                std::fs::rename(root.join("bin.old"), root.join("bin")).unwrap();
                std::fs::remove_file(root.join(PROGRAM_PACKAGE_SPEC_BACKUP)).unwrap();
            }
            write_json_atomic(
                &root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER),
                &ProgramPackageTransactionMarker {
                    phase,
                    workspace: true,
                },
            )
            .unwrap();
            for _ in 0..2 {
                let report = store.load_all().await.unwrap();
                assert!(report.invalid.is_empty(), "{:?}", report.invalid);
                let committed = phase == ProgramPackageTransactionPhase::Committed;
                assert_eq!(
                    report.valid[0].spec,
                    if committed {
                        next.clone()
                    } else {
                        spec.clone()
                    }
                );
                assert_eq!(
                    store
                        .load_configuration_state(&spec.id)
                        .await
                        .unwrap()
                        .unwrap(),
                    if committed {
                        after.clone()
                    } else {
                        before.clone()
                    }
                );
                assert_eq!(
                    std::fs::read(root.join("bin/tool")).unwrap(),
                    if committed {
                        b"new-content".as_slice()
                    } else {
                        b"old".as_slice()
                    }
                );
                assert!(!root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER).exists());
            }
        }
    }

    #[tokio::test]
    async fn package_preparation_restart_discards_only_uncommitted_copies() {
        let directory = tempfile::tempdir().unwrap();
        let (store, spec, source) = managed_package_fixture(&directory).await;
        let root = store.workspace(&spec.id).await.unwrap();
        for name in [
            PROGRAM_PACKAGE_SPEC_BACKUP,
            PROGRAM_PACKAGE_NEXT_SPEC,
            super::PROGRAM_PACKAGE_STATE_BACKUP,
            super::PROGRAM_PACKAGE_NEXT_STATE,
        ] {
            std::fs::write(root.join(name), b"interrupted preparation").unwrap();
        }
        let report = store.load_all().await.unwrap();
        assert!(report.invalid.is_empty());
        assert_eq!(report.valid[0].spec, spec);
        assert_eq!(std::fs::read(root.join("bin/tool")).unwrap(), b"old");
        let staged = store.stage_package(&spec, &source).await.unwrap();
        store.discard_package(staged).await.unwrap();
    }

    #[tokio::test]
    async fn package_recovery_blocks_binary_use_until_both_workspace_copies_are_restored() {
        let directory = tempfile::tempdir().unwrap();
        let (store, spec, _) = managed_package_fixture(&directory).await;
        let root = store.workspace(&spec.id).await.unwrap();
        write_bytes_atomic(
            &root.join(PROGRAM_PACKAGE_SPEC_BACKUP),
            &std::fs::read(root.join("program.json")).unwrap(),
        )
        .unwrap();
        write_json_atomic(
            &root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER),
            &ProgramPackageTransactionMarker {
                phase: ProgramPackageTransactionPhase::Prepared,
                workspace: true,
            },
        )
        .unwrap();
        assert_eq!(
            store
                .executable_metadata(&spec)
                .await
                .unwrap_err()
                .message_key
                .as_deref(),
            Some("PROGRAM_PACKAGE_RECOVERY_REQUIRED")
        );
        assert_eq!(store.load_all().await.unwrap().invalid.len(), 1);
        assert!(root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER).exists());
        assert!(root.join(PROGRAM_PACKAGE_SPEC_BACKUP).exists());
        let state = xray_state(
            1,
            merge_configuration_sources(
                ProgramKind::Xray,
                &[SourceSnapshot::parse(
                    "source",
                    "Source",
                    ConfigurationFormat::Jsonc,
                    b"{}",
                    1,
                    false,
                )
                .unwrap()],
            )
            .unwrap(),
        );
        write_json_atomic(&root.join(super::PROGRAM_PACKAGE_STATE_BACKUP), &state).unwrap();
        std::fs::create_dir_all(root.join(super::CONFIGURATION_STATE_DIRECTORY)).unwrap();
        assert!(store.load_all().await.unwrap().invalid.is_empty());
        assert!(store.executable_metadata(&spec).await.is_ok());
        assert_eq!(
            store.load_configuration_state(&spec.id).await.unwrap(),
            Some(state)
        );
    }

    #[tokio::test]
    async fn package_cleanup_retains_terminal_marker_until_artifacts_are_removed() {
        let directory = tempfile::tempdir().unwrap();
        let (store, spec, _) = managed_package_fixture(&directory).await;
        let root = store.workspace(&spec.id).await.unwrap();
        // A directory where a metadata file is expected reliably fails file removal on all hosts.
        let blocked = root.join(super::PROGRAM_PACKAGE_NEXT_STATE);
        std::fs::create_dir(&blocked).unwrap();
        write_json_atomic(
            &root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER),
            &ProgramPackageTransactionMarker {
                phase: ProgramPackageTransactionPhase::Committed,
                workspace: false,
            },
        )
        .unwrap();
        assert!(store.load_all().await.unwrap().invalid.is_empty());
        assert!(root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER).exists());
        assert_eq!(std::fs::read(root.join("bin/tool")).unwrap(), b"old");
        std::fs::remove_dir(blocked).unwrap();
        for _ in 0..2 {
            assert!(store.load_all().await.unwrap().invalid.is_empty());
            assert!(!root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER).exists());
        }
    }

    #[tokio::test]
    async fn committed_managed_package_transaction_keeps_binary_and_metadata() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (store, spec, second) = managed_package_fixture(&directory).await;
        let staged = store
            .stage_package(&spec, &second)
            .await
            .expect("stage package");
        let workspace = store.workspace(&spec.id).await.expect("workspace");
        let mut next = spec.clone();
        next.name = "Committed package".into();
        let next_spec_path = workspace.join(PROGRAM_PACKAGE_NEXT_SPEC);
        write_json_atomic(&next_spec_path, &next).expect("next metadata");
        replace_with_backup(
            &next_spec_path,
            &workspace.join("program.json"),
            &workspace.join(PROGRAM_PACKAGE_SPEC_BACKUP),
        )
        .expect("swap metadata");
        std::fs::rename(workspace.join("bin"), workspace.join("bin.old")).expect("backup package");
        std::fs::rename(staged.staged_directory, workspace.join("bin")).expect("swap package");
        write_json_atomic(
            &workspace.join(PROGRAM_PACKAGE_TRANSACTION_MARKER),
            &ProgramPackageTransactionMarker {
                phase: ProgramPackageTransactionPhase::Committed,
                workspace: false,
            },
        )
        .expect("commit marker");

        let report = store.load_all().await.expect("startup recovery");
        assert!(report.invalid.is_empty(), "{:?}", report.invalid);
        assert_eq!(report.valid[0].spec.name, next.name);
        assert_eq!(
            std::fs::read(workspace.join("bin/tool")).expect("active package"),
            b"new-content"
        );
        assert!(!workspace.join(PROGRAM_PACKAGE_TRANSACTION_MARKER).exists());
        assert!(!workspace.join("bin.old").exists());
    }
}
