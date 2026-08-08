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
    CamelliaNexusError, ConfigStore, CreateAssets, ErrorCode, ExecutableMetadata, InvalidProgram,
    LoadReport, LogChunk, LogStream, MAX_CONFIG_BYTES, ProgramConfigTransaction, ProgramId,
    ProgramSpec, ProgramStore, RawConfig, Result, StagedConfig, StagedPackage, StoredProgram,
    config_service::hash_bytes,
};
use uuid::Uuid;

const PACKAGE_MAX_BYTES: u64 = 512 * 1024 * 1024;
const PACKAGE_MAX_ENTRIES: usize = 4096;
const PROGRAM_SPEC_MAX_BYTES: u64 = 1024 * 1024;
const PROGRAM_CONFIG_TRANSACTION_VERSION: u32 = 1;
const PROGRAM_CONFIG_TRANSACTION_MARKER: &str = ".program-config-transaction.json";
const PROGRAM_CONFIG_SPEC_BACKUP: &str = ".program-config-program.json.bak";
const PROGRAM_CONFIG_NEXT_SPEC: &str = ".program-config-program.json.next";
const PROGRAM_PACKAGE_TRANSACTION_VERSION: u32 = 1;
const PROGRAM_PACKAGE_TRANSACTION_MARKER: &str = ".program-package-transaction.json";
const PROGRAM_PACKAGE_SPEC_BACKUP: &str = ".program-package-program.json.bak";
const PROGRAM_PACKAGE_NEXT_SPEC: &str = ".program-package-program.json.next";
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
struct ProgramConfigTransactionMarker {
    version: u32,
    config_relative_path: PathBuf,
    staged_config_file_name: String,
    committed: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProgramPackageTransactionMarker {
    version: u32,
    committed: bool,
}

#[derive(Clone)]
pub struct FileStore {
    root: Arc<PathBuf>,
}

impl FileStore {
    pub fn new(root: PathBuf) -> Result<Self> {
        let store = Self {
            root: Arc::new(root),
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

    fn program_root(&self, id: &ProgramId) -> PathBuf {
        self.programs_root().join(id.as_str())
    }

    fn config_path(&self, spec: &ProgramSpec) -> Result<PathBuf> {
        let root = self.program_root(&spec.id);
        config_path_in_root(&root, spec)
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
                    recover_program_config_transaction(&path)?;
                    recover_program_package_transaction(&path)?;
                    clean_atomic_temps(&path);
                    let content = read_with_overflow_byte(&spec_path, PROGRAM_SPEC_MAX_BYTES)?;
                    if content.len() as u64 > PROGRAM_SPEC_MAX_BYTES {
                        return Err(CamelliaNexusError::invalid_spec(
                            "program.json exceeds the 1 MiB limit",
                        ));
                    }
                    let (spec, migrated) = decode_program_spec(&content)?;
                    spec.validate()?;
                    if spec.id != workspace_id {
                        return Err(CamelliaNexusError::invalid_spec(
                            "Workspace name does not match Program id",
                        ));
                    }
                    if migrated {
                        write_json_atomic(&spec_path, &spec)?;
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
        let path = spec.executable_path(&self.program_root(&spec.id));
        blocking(move || executable_metadata(&path)).await
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
    ) -> Result<()> {
        let root = self.program_root(&staged.program_id);
        let expected_spec = expected_spec.clone();
        let next_spec = next_spec.clone();
        blocking(move || {
            let expected = root.join("bin.new");
            if staged.staged_directory != expected
                || staged.program_id != expected_spec.id
                || staged.program_id != next_spec.id
            {
                return Err(CamelliaNexusError::new(
                    ErrorCode::InvalidPath,
                    "Invalid staged package path",
                ));
            }
            recover_program_package_transaction(&root)?;
            let marker_path = root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER);
            let spec_path = root.join("program.json");
            let spec_backup = root.join(PROGRAM_PACKAGE_SPEC_BACKUP);
            let next_spec_path = root.join(PROGRAM_PACKAGE_NEXT_SPEC);
            let active = root.join("bin");
            let backup = root.join("bin.old");
            if marker_path.exists()
                || spec_backup.exists()
                || next_spec_path.exists()
                || backup.exists()
            {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ProgramBusy,
                    "A previous managed package transaction still requires recovery",
                ));
            }
            let stored_bytes = read_with_overflow_byte(&spec_path, PROGRAM_SPEC_MAX_BYTES)?;
            if stored_bytes.len() as u64 > PROGRAM_SPEC_MAX_BYTES {
                return Err(CamelliaNexusError::invalid_spec(
                    "program.json exceeds the 1 MiB limit",
                ));
            }
            let (stored_spec, _) = decode_program_spec(&stored_bytes)?;
            if stored_spec != expected_spec {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Program settings changed while the package was being prepared",
                ));
            }
            write_json_atomic(&next_spec_path, &next_spec)?;
            let marker = ProgramPackageTransactionMarker {
                version: PROGRAM_PACKAGE_TRANSACTION_VERSION,
                committed: false,
            };
            if let Err(error) = write_json_atomic(&marker_path, &marker) {
                let recovery = if marker_path.exists() {
                    recover_program_package_transaction(&root)
                } else {
                    if next_spec_path.exists() {
                        let _ = fs::remove_file(&next_spec_path);
                    }
                    if expected.exists() {
                        let _ = discard_directory_background(&expected);
                    }
                    Ok(())
                };
                return match recovery {
                    Ok(()) => Err(error),
                    Err(recovery_error) => Err(CamelliaNexusError::new(
                        ErrorCode::Storage,
                        "The managed package transaction could not be prepared or recovered",
                    )
                    .with_details(format!("prepare: {error}; recovery: {recovery_error}"))),
                };
            }

            let result = (|| -> Result<()> {
                replace_with_backup(&next_spec_path, &spec_path, &spec_backup)?;
                fs::rename(&active, &backup)?;
                if let Err(error) = fs::rename(&expected, &active) {
                    let _ = fs::rename(&backup, &active);
                    return Err(error.into());
                }
                sync_directory(&root)
            })();
            if let Err(error) = result {
                let recovery = recover_program_package_transaction(&root);
                return match recovery {
                    Ok(()) => Err(error),
                    Err(recovery_error) => Err(CamelliaNexusError::new(
                        ErrorCode::Storage,
                        "The managed package and metadata could not be committed or restored",
                    )
                    .with_details(format!("commit: {error}; recovery: {recovery_error}"))),
                };
            }

            let committed = ProgramPackageTransactionMarker {
                version: PROGRAM_PACKAGE_TRANSACTION_VERSION,
                committed: true,
            };
            if let Err(error) = write_json_atomic(&marker_path, &committed) {
                let persisted = load_program_package_transaction_marker(&root)?;
                if persisted.is_none_or(|marker| !marker.committed) {
                    let recovery = recover_program_package_transaction(&root);
                    return match recovery {
                        Ok(()) => Err(error),
                        Err(recovery_error) => Err(CamelliaNexusError::new(
                            ErrorCode::Storage,
                            "The managed package commit point could not be persisted or recovered",
                        )
                        .with_details(format!("commit: {error}; recovery: {recovery_error}"))),
                    };
                }
                tracing::warn!(%error, "managed package commit marker was installed but its directory sync reported an error");
            }
            cleanup_committed_program_package_transaction(&root);
            Ok(())
        })
        .await
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

    async fn begin_program_config_update(
        &self,
        expected_spec: &ProgramSpec,
        next_spec: &ProgramSpec,
        staged_config: StagedConfig,
        expected_config_hash: &str,
    ) -> Result<ProgramConfigTransaction> {
        if expected_spec.id != next_spec.id {
            return Err(CamelliaNexusError::invalid_spec(
                "A program/configuration transaction cannot change the Program id",
            ));
        }
        let root = self.program_root(&expected_spec.id);
        let expected_spec = expected_spec.clone();
        let next_spec = next_spec.clone();
        let expected_config_hash = expected_config_hash.to_owned();
        blocking(move || {
            let expected_target = config_path_in_root(&root, &next_spec)?;
            validate_staged_config(&root, &expected_target, &staged_config)?;
            recover_program_config_transaction(&root)?;

            let marker_path = root.join(PROGRAM_CONFIG_TRANSACTION_MARKER);
            let spec_path = root.join("program.json");
            let spec_backup = root.join(PROGRAM_CONFIG_SPEC_BACKUP);
            let next_spec_path = root.join(PROGRAM_CONFIG_NEXT_SPEC);
            let config_backup = suffixed_path(&expected_target, ".bak");
            let config_pending = suffixed_path(&expected_target, ".pending");
            if marker_path.exists()
                || spec_backup.exists()
                || next_spec_path.exists()
                || config_backup.exists()
                || config_pending.exists()
            {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ProgramBusy,
                    "A previous program/configuration transaction still requires recovery",
                ));
            }

            let current_spec_bytes = read_with_overflow_byte(&spec_path, PROGRAM_SPEC_MAX_BYTES)?;
            if current_spec_bytes.len() as u64 > PROGRAM_SPEC_MAX_BYTES {
                return Err(CamelliaNexusError::invalid_spec(
                    "program.json exceeds the 1 MiB limit",
                ));
            }
            let (stored_spec, _) = decode_program_spec(&current_spec_bytes)?;
            if stored_spec != expected_spec {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Program settings changed before the configuration transaction committed",
                ));
            }
            let current_config =
                read_with_overflow_byte(&expected_target, MAX_CONFIG_BYTES as u64)?;
            if current_config.len() > MAX_CONFIG_BYTES
                || hash_bytes(&current_config) != expected_config_hash
            {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Configuration changed before the transaction committed",
                ));
            }

            write_json_atomic(&next_spec_path, &next_spec)?;
            let config_relative_path = expected_target
                .strip_prefix(&root)
                .map_err(|_| {
                    CamelliaNexusError::new(
                        ErrorCode::InvalidPath,
                        "Configuration target escaped the Program workspace",
                    )
                })?
                .to_path_buf();
            let staged_config_file_name = staged_config
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    CamelliaNexusError::new(
                        ErrorCode::InvalidPath,
                        "Staged configuration has no valid file name",
                    )
                })?
                .to_owned();
            write_json_atomic(
                &marker_path,
                &ProgramConfigTransactionMarker {
                    version: PROGRAM_CONFIG_TRANSACTION_VERSION,
                    config_relative_path,
                    staged_config_file_name,
                    committed: false,
                },
            )?;

            let result = (|| -> Result<()> {
                replace_with_backup(&next_spec_path, &spec_path, &spec_backup)?;
                replace_with_backup(&staged_config.path, &expected_target, &staged_config.backup)?;
                sync_directory(&root)?;
                if let Some(parent) = expected_target.parent()
                    && parent != root
                {
                    sync_directory(parent)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                let recovery = recover_program_config_transaction(&root);
                return match recovery {
                    Ok(()) => Err(error),
                    Err(recovery_error) => Err(CamelliaNexusError::new(
                        ErrorCode::Storage,
                        "Program settings and configuration could not be committed or restored",
                    )
                    .with_details(format!("commit: {error}; recovery: {recovery_error}"))),
                };
            }
            Ok(ProgramConfigTransaction {
                program_id: expected_spec.id,
                config_target: expected_target,
            })
        })
        .await
    }

    async fn finalize_program_config_update(
        &self,
        transaction: ProgramConfigTransaction,
    ) -> Result<()> {
        let root = self.program_root(&transaction.program_id);
        blocking(move || {
            let mut marker = load_program_config_transaction_marker(&root)?.ok_or_else(|| {
                CamelliaNexusError::new(
                    ErrorCode::InvalidState,
                    "Program/configuration transaction marker is missing",
                )
            })?;
            let config_target = transaction_config_target(&root, &marker)?;
            if config_target != transaction.config_target || marker.committed {
                return Err(CamelliaNexusError::new(
                    ErrorCode::InvalidState,
                    "Program/configuration transaction does not match the pending commit",
                ));
            }
            marker.committed = true;
            if let Err(error) =
                write_json_atomic(&root.join(PROGRAM_CONFIG_TRANSACTION_MARKER), &marker)
            {
                let persisted = load_program_config_transaction_marker(&root)?;
                if persisted.is_none_or(|persisted| !persisted.committed) {
                    return Err(error);
                }
                tracing::warn!(%error, "program/configuration commit marker was installed but its directory sync reported an error");
            }
            cleanup_committed_program_config_transaction(&root, &marker);
            Ok(())
        })
        .await
    }

    async fn rollback_program_config_update(
        &self,
        transaction: ProgramConfigTransaction,
    ) -> Result<()> {
        let root = self.program_root(&transaction.program_id);
        blocking(move || {
            let marker = load_program_config_transaction_marker(&root)?.ok_or_else(|| {
                CamelliaNexusError::new(
                    ErrorCode::InvalidState,
                    "Program/configuration transaction marker is missing",
                )
            })?;
            let config_target = transaction_config_target(&root, &marker)?;
            if config_target != transaction.config_target || marker.committed {
                return Err(CamelliaNexusError::new(
                    ErrorCode::InvalidState,
                    "Program/configuration transaction can no longer be rolled back",
                ));
            }
            rollback_program_config_transaction(&root, &marker)
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
            let old = root.join("bin.old");
            let new = root.join("bin.new");
            if !bin.exists() && old.exists() {
                fs::rename(&old, &bin)?;
            }
            if bin.exists() {
                if old.exists() {
                    discard_directory_background(&old)?;
                }
                if new.exists() {
                    discard_directory_background(&new)?;
                }
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

fn decode_program_spec(content: &[u8]) -> Result<(ProgramSpec, bool)> {
    let spec: ProgramSpec = serde_json::from_slice(content)?;
    if spec.schema_version != camellia_nexus_core::SCHEMA_VERSION {
        return Err(CamelliaNexusError::invalid_spec(format!(
            "Unsupported schema version {}; this pre-release product accepts only schema {}",
            spec.schema_version,
            camellia_nexus_core::SCHEMA_VERSION
        )));
    }
    Ok((spec, false))
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
        let target = self.config_path(spec)?;
        blocking(move || {
            let backup = suffixed_path(&target, ".bak");
            let pending = suffixed_path(&target, ".pending");
            if pending.exists() {
                fs::remove_file(&pending)?;
            }
            if let Some(parent) = target.parent() {
                // Removing and syncing the pending marker is the durable commit point. The
                // backup is cleanup only after this point, so a cleanup error can never make
                // startup recovery roll back a configuration already reported as committed.
                sync_directory(parent)?;
                if backup.exists()
                    && let Err(error) = fs::remove_file(&backup)
                {
                    tracing::warn!(path = %backup.display(), %error, "could not remove committed configuration backup");
                }
                sync_directory(parent)?;
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
        blocking(move || {
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
            Ok(())
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

fn validate_staged_config(root: &Path, target: &Path, staged: &StagedConfig) -> Result<()> {
    if staged.target != target
        || staged.backup != suffixed_path(target, ".bak")
        || staged.path.parent() != target.parent()
    {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Staged configuration does not belong to the expected Program target",
        ));
    }
    let file_name = staged
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::InvalidPath,
                "Staged configuration has no valid file name",
            )
        })?;
    if !file_name.starts_with(".camellia-nexus-staged-") || staged.path.strip_prefix(root).is_err()
    {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Staged configuration path is invalid",
        ));
    }
    let metadata = fs::symlink_metadata(&staged.path)?;
    if !metadata.is_file() || is_link_or_reparse(&metadata) {
        return Err(CamelliaNexusError::new(
            ErrorCode::InvalidPath,
            "Staged configuration is not a regular file",
        ));
    }
    Ok(())
}

fn load_program_config_transaction_marker(
    root: &Path,
) -> Result<Option<ProgramConfigTransactionMarker>> {
    let marker_path = root.join(PROGRAM_CONFIG_TRANSACTION_MARKER);
    if !marker_path.exists() {
        return Ok(None);
    }
    let bytes = read_with_overflow_byte(&marker_path, 64 * 1024)?;
    if bytes.len() > 64 * 1024 {
        return Err(CamelliaNexusError::new(
            ErrorCode::Storage,
            "Program/configuration transaction marker is oversized",
        ));
    }
    let marker: ProgramConfigTransactionMarker =
        serde_json::from_slice(&bytes).map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::Storage,
                "Program/configuration transaction marker is invalid",
            )
            .with_details(error.to_string())
        })?;
    if marker.version != PROGRAM_CONFIG_TRANSACTION_VERSION {
        return Err(CamelliaNexusError::new(
            ErrorCode::Storage,
            "Program/configuration transaction marker version is unsupported",
        ));
    }
    Ok(Some(marker))
}

fn transaction_config_target(
    root: &Path,
    marker: &ProgramConfigTransactionMarker,
) -> Result<PathBuf> {
    let target = safe_path(root, &marker.config_relative_path)?;
    let staged_name = Path::new(&marker.staged_config_file_name);
    if staged_name.file_name().and_then(|name| name.to_str())
        != Some(marker.staged_config_file_name.as_str())
        || !marker
            .staged_config_file_name
            .starts_with(".camellia-nexus-staged-")
        || target.parent().is_none()
    {
        return Err(CamelliaNexusError::new(
            ErrorCode::Storage,
            "Program/configuration transaction marker contains an invalid staged path",
        ));
    }
    Ok(target)
}

fn transaction_staged_config_path(
    target: &Path,
    marker: &ProgramConfigTransactionMarker,
) -> Result<PathBuf> {
    Ok(target
        .parent()
        .ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::Storage,
                "Program/configuration transaction target has no parent",
            )
        })?
        .join(&marker.staged_config_file_name))
}

fn restore_transaction_backup(backup: &Path, target: &Path) -> Result<()> {
    if backup.exists() {
        replace_file(backup, target)?;
    }
    Ok(())
}

fn rollback_program_config_transaction(
    root: &Path,
    marker: &ProgramConfigTransactionMarker,
) -> Result<()> {
    let target = transaction_config_target(root, marker)?;
    let staged = transaction_staged_config_path(&target, marker)?;
    restore_transaction_backup(
        &root.join(PROGRAM_CONFIG_SPEC_BACKUP),
        &root.join("program.json"),
    )?;
    restore_transaction_backup(&suffixed_path(&target, ".bak"), &target)?;
    for path in [root.join(PROGRAM_CONFIG_NEXT_SPEC), staged] {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    let marker_path = root.join(PROGRAM_CONFIG_TRANSACTION_MARKER);
    if marker_path.exists() {
        fs::remove_file(marker_path)?;
    }
    sync_directory(root)?;
    if let Some(parent) = target.parent()
        && parent != root
    {
        sync_directory(parent)?;
    }
    Ok(())
}

fn cleanup_committed_program_config_transaction(
    root: &Path,
    marker: &ProgramConfigTransactionMarker,
) {
    let cleanup = (|| -> Result<()> {
        let target = transaction_config_target(root, marker)?;
        let staged = transaction_staged_config_path(&target, marker)?;
        for path in [
            root.join(PROGRAM_CONFIG_SPEC_BACKUP),
            root.join(PROGRAM_CONFIG_NEXT_SPEC),
            suffixed_path(&target, ".bak"),
            staged,
        ] {
            if path.exists()
                && let Err(error) = fs::remove_file(&path)
            {
                tracing::warn!(path = %path.display(), %error, "could not remove committed program/configuration transaction artifact");
            }
        }
        let marker_path = root.join(PROGRAM_CONFIG_TRANSACTION_MARKER);
        if marker_path.exists()
            && let Err(error) = fs::remove_file(&marker_path)
        {
            tracing::warn!(path = %marker_path.display(), %error, "could not remove committed program/configuration transaction marker");
        }
        if let Err(error) = sync_directory(root) {
            tracing::warn!(path = %root.display(), %error, "could not sync committed program/configuration transaction cleanup");
        }
        if let Some(parent) = target.parent()
            && parent != root
            && let Err(error) = sync_directory(parent)
        {
            tracing::warn!(path = %parent.display(), %error, "could not sync committed configuration cleanup");
        }
        Ok(())
    })();
    if let Err(error) = cleanup {
        tracing::warn!(%error, "could not fully clean a committed program/configuration transaction");
    }
}

fn recover_program_config_transaction(root: &Path) -> Result<()> {
    let Some(marker) = load_program_config_transaction_marker(root)? else {
        return Ok(());
    };
    if marker.committed {
        cleanup_committed_program_config_transaction(root, &marker);
        Ok(())
    } else {
        rollback_program_config_transaction(root, &marker)
    }
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
        ));
    }
    let marker: ProgramPackageTransactionMarker =
        serde_json::from_slice(&bytes).map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::Storage,
                "Managed package transaction marker is invalid",
            )
            .with_details(error.to_string())
        })?;
    if marker.version != PROGRAM_PACKAGE_TRANSACTION_VERSION {
        return Err(CamelliaNexusError::new(
            ErrorCode::Storage,
            "Managed package transaction marker version is unsupported",
        ));
    }
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

fn rollback_program_package_transaction(root: &Path) -> Result<()> {
    restore_transaction_backup(
        &root.join(PROGRAM_PACKAGE_SPEC_BACKUP),
        &root.join("program.json"),
    )?;
    let active = root.join("bin");
    let backup = root.join("bin.old");
    if backup.exists() {
        if active.exists() {
            discard_directory_background(&active)?;
        }
        fs::rename(&backup, &active)?;
    }
    let next_spec = root.join(PROGRAM_PACKAGE_NEXT_SPEC);
    if next_spec.exists() {
        fs::remove_file(next_spec)?;
    }
    let staged = root.join("bin.new");
    if staged.exists() {
        discard_directory_background(&staged)?;
    }
    let marker = root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER);
    if marker.exists() {
        fs::remove_file(marker)?;
    }
    sync_directory(root)
}

fn cleanup_committed_program_package_transaction(root: &Path) {
    for path in [
        root.join(PROGRAM_PACKAGE_SPEC_BACKUP),
        root.join(PROGRAM_PACKAGE_NEXT_SPEC),
    ] {
        if path.exists()
            && let Err(error) = fs::remove_file(&path)
        {
            tracing::warn!(path = %path.display(), %error, "could not remove committed managed package metadata artifact");
        }
    }
    for path in [root.join("bin.old"), root.join("bin.new")] {
        if path.exists()
            && let Err(error) = discard_directory_background(&path)
        {
            tracing::warn!(path = %path.display(), %error, "could not remove committed managed package directory artifact");
        }
    }
    let marker = root.join(PROGRAM_PACKAGE_TRANSACTION_MARKER);
    if marker.exists()
        && let Err(error) = fs::remove_file(&marker)
    {
        tracing::warn!(path = %marker.display(), %error, "could not remove committed managed package transaction marker");
    }
    if let Err(error) = sync_directory(root) {
        tracing::warn!(path = %root.display(), %error, "could not sync committed managed package cleanup");
    }
}

fn recover_program_package_transaction(root: &Path) -> Result<()> {
    let Some(marker) = load_program_package_transaction_marker(root)? else {
        return Ok(());
    };
    if marker.committed {
        cleanup_committed_program_package_transaction(root);
        Ok(())
    } else {
        rollback_program_package_transaction(root)
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
    Ok(ExecutableMetadata {
        size: metadata.len(),
        modified_unix_ms,
        detected_version: None,
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
        ConfigStore, CreateAssets, ErrorCode, ExecutableSpec, MAX_CONFIG_BYTES, ProgramId,
        ProgramSpec, ProgramStore, ProgramType, RestartPolicy, SCHEMA_VERSION,
    };

    use super::{
        CREATE_COMMITTED_MARKER, FileStore, PROGRAM_CONFIG_TRANSACTION_MARKER,
        PROGRAM_PACKAGE_NEXT_SPEC, PROGRAM_PACKAGE_SPEC_BACKUP, PROGRAM_PACKAGE_TRANSACTION_MARKER,
        ProgramConfigTransactionMarker, ProgramPackageTransactionMarker, decode_program_spec,
        enqueue_directory_cleanup, load_program_config_transaction_marker, read_tail,
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
            schema_version: SCHEMA_VERSION,
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

    fn configured_spec() -> ProgramSpec {
        let mut spec = generic_spec();
        spec.program_type = ProgramType::Xray {
            main_config: Some("config/config.json".into()),
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

    #[test]
    fn rejects_prelaunch_program_schema_without_migrating_it() {
        let spec = generic_spec();
        let mut stale = serde_json::to_value(&spec).expect("serialize");
        stale["schemaVersion"] = serde_json::Value::from(SCHEMA_VERSION - 1);

        let error = decode_program_spec(&serde_json::to_vec(&stale).expect("bytes"))
            .expect_err("old pre-release schemas must be rejected");

        assert!(error.to_string().contains("accepts only schema"));
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
                version: super::PROGRAM_PACKAGE_TRANSACTION_VERSION,
                committed: true,
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
            main_config: Some("config/config.json".into()),
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
            main_config: Some("config/config.json".into()),
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
            main_config: Some("config/config.json".into()),
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
    async fn pending_program_config_transaction_recovers_both_old_files() {
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
        let mut next = spec.clone();
        next.name = "Prepared name".into();
        let base_hash = store.current_hash(&spec).await.expect("base hash");
        let staged = store.stage(&next, br#"{"new":true}"#).await.expect("stage");
        store
            .begin_program_config_update(&spec, &next, staged, &base_hash)
            .await
            .expect("begin transaction");

        let report = store.load_all().await.expect("startup recovery");
        assert!(report.invalid.is_empty(), "{:?}", report.invalid);
        assert_eq!(report.valid[0].spec.name, spec.name);
        assert_eq!(
            store
                .load(&report.valid[0].spec)
                .await
                .expect("config")
                .content,
            r#"{"old":true}"#
        );
    }

    #[tokio::test]
    async fn committed_program_config_marker_keeps_both_new_files_after_crash() {
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
        let mut next = spec.clone();
        next.name = "Committed name".into();
        let base_hash = store.current_hash(&spec).await.expect("base hash");
        let staged = store.stage(&next, br#"{"new":true}"#).await.expect("stage");
        store
            .begin_program_config_update(&spec, &next, staged, &base_hash)
            .await
            .expect("begin transaction");

        let workspace = store.workspace(&spec.id).await.expect("workspace");
        let marker = load_program_config_transaction_marker(&workspace)
            .expect("load marker")
            .expect("pending marker");
        write_json_atomic(
            &workspace.join(PROGRAM_CONFIG_TRANSACTION_MARKER),
            &ProgramConfigTransactionMarker {
                committed: true,
                ..marker
            },
        )
        .expect("persist commit point");

        let report = store.load_all().await.expect("startup recovery");
        assert!(report.invalid.is_empty(), "{:?}", report.invalid);
        assert_eq!(report.valid[0].spec.name, next.name);
        assert_eq!(
            store
                .load(&report.valid[0].spec)
                .await
                .expect("config")
                .content,
            r#"{"new":true}"#
        );
        assert!(!workspace.join(PROGRAM_CONFIG_TRANSACTION_MARKER).exists());
    }

    #[tokio::test]
    async fn staged_config_enforces_limits_before_and_after_external_tools() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = FileStore::new(directory.path().to_path_buf()).expect("store");
        let mut spec = generic_spec();
        spec.program_type = ProgramType::Xray {
            main_config: Some("config/config.json".into()),
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
        store
            .commit_package(staged, &spec, &spec)
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
                version: super::PROGRAM_PACKAGE_TRANSACTION_VERSION,
                committed: false,
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
                version: super::PROGRAM_PACKAGE_TRANSACTION_VERSION,
                committed: true,
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
