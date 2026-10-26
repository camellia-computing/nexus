use camellia_nexus_core::{
    CamelliaNexusError, ErrorCode, IntentHistoryPayload, IntentHistoryRecord,
    MAX_INTENT_HISTORY_BYTES, ProgramId, Result, config_service::hash_bytes,
};
#[cfg(all(not(test), not(windows)))]
use camellia_nexus_licensing::{OsSecureStore, SecretKey, SecureStore, SecureStoreMode};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

const MAX_ENVELOPE_BYTES: usize = MAX_INTENT_HISTORY_BYTES as usize + 16 * 1024;

#[derive(Clone)]
pub(crate) struct IntentHistoryVault {
    root: Arc<PathBuf>,
    #[cfg(test)]
    records: Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>,
    #[cfg(test)]
    fail_protection: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedRecord {
    program_id: ProgramId,
    record_id: String,
    payload: IntentHistoryPayload,
}

impl IntentHistoryVault {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
            #[cfg(test)]
            records: Arc::new(std::sync::Mutex::new(Default::default())),
            #[cfg(test)]
            fail_protection: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub(crate) async fn stage(
        &self,
        id: &ProgramId,
        operation_id: &str,
        label_key: String,
        subject_key: String,
        now: u64,
        payload: IntentHistoryPayload,
    ) -> Result<IntentHistoryRecord> {
        let vault = self.clone();
        let id = id.clone();
        let record_id = hash_bytes(operation_id.as_bytes());
        tokio::task::spawn_blocking(move || {
            let bytes = Zeroizing::new(serde_json::to_vec(&payload)?);
            if bytes.len() as u64 > MAX_INTENT_HISTORY_BYTES {
                return Err(protection_error());
            }
            let metadata = IntentHistoryRecord {
                record_id: record_id.clone(),
                payload_ref: record_id.clone(),
                payload_hash: hash_bytes(&bytes),
                payload_bytes: bytes.len() as u64,
                label_key,
                subject_key,
                created_unix_ms: now,
                undone: false,
            };
            let envelope = Zeroizing::new(serde_json::to_vec(&ProtectedRecord {
                program_id: id.clone(),
                record_id: record_id.clone(),
                payload,
            })?);
            if envelope.len() > MAX_ENVELOPE_BYTES {
                return Err(protection_error());
            }
            let directory = vault.directory(&id)?;
            let path = directory.join(format!("{record_id}.sealed"));
            if path.exists() {
                if vault.read_bytes(&id, &record_id, &path)?.as_slice() != envelope.as_slice() {
                    return Err(protection_error());
                }
            } else {
                vault.write_bytes(&id, &record_id, &path, &envelope)?;
                if vault.read_bytes(&id, &record_id, &path)?.as_slice() != envelope.as_slice() {
                    return Err(protection_error());
                }
            }
            Ok(metadata)
        })
        .await
        .map_err(|_| protection_error())?
    }

    pub(crate) async fn load(
        &self,
        id: &ProgramId,
        record: &IntentHistoryRecord,
    ) -> Result<IntentHistoryPayload> {
        let vault = self.clone();
        let id = id.clone();
        let record = record.clone();
        tokio::task::spawn_blocking(move || {
            if !valid_reference(&record.payload_ref) {
                return Err(protection_error());
            }
            let path = vault
                .directory(&id)?
                .join(format!("{}.sealed", record.payload_ref));
            let bytes = vault.read_bytes(&id, &record.payload_ref, &path)?;
            let protected: ProtectedRecord =
                serde_json::from_slice(&bytes).map_err(|_| protection_error())?;
            if protected.program_id != id || protected.record_id != record.record_id {
                return Err(protection_error());
            }
            let payload_bytes = Zeroizing::new(serde_json::to_vec(&protected.payload)?);
            if payload_bytes.len() as u64 != record.payload_bytes
                || hash_bytes(&payload_bytes) != record.payload_hash
            {
                return Err(protection_error());
            }
            Ok(protected.payload)
        })
        .await
        .map_err(|_| protection_error())?
    }

    pub(crate) async fn prune(&self, id: &ProgramId, retained: BTreeSet<String>) -> Result<()> {
        let vault = self.clone();
        let id = id.clone();
        tokio::task::spawn_blocking(move || {
            let directory = vault
                .root
                .join("secure")
                .join("intent-history")
                .join(id.as_str());
            if !directory.exists() {
                return Ok(());
            }
            crate::platform::validate_internal_path(&vault.root, &directory, true)?;
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(reference) = name
                    .to_str()
                    .and_then(|name| name.strip_suffix(".sealed"))
                    .filter(|reference| valid_reference(reference))
                else {
                    continue;
                };
                if !retained.contains(reference) {
                    ensure_regular(&entry.path())?;
                    vault.remove_bytes(&id, reference, &entry.path())?;
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| protection_error())?
    }

    fn directory(&self, id: &ProgramId) -> Result<PathBuf> {
        let directory = self
            .root
            .join("secure")
            .join("intent-history")
            .join(id.as_str());
        crate::platform::validate_internal_path(&self.root, &directory, false)?;
        fs::create_dir_all(&directory)?;
        crate::platform::validate_internal_path(&self.root, &directory, true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        Ok(directory)
    }

    fn namespace(&self, id: &ProgramId, reference: &str) -> String {
        hash_bytes(
            &serde_json::to_vec(&(self.root.as_os_str().to_string_lossy(), id, reference))
                .expect("history namespace"),
        )
    }

    fn read_bytes(
        &self,
        id: &ProgramId,
        reference: &str,
        path: &Path,
    ) -> Result<Zeroizing<Vec<u8>>> {
        ensure_regular(path)?;
        #[cfg(test)]
        {
            let bytes = self
                .records
                .lock()
                .map_err(|_| protection_error())?
                .get(&self.namespace(id, reference))
                .cloned()
                .ok_or_else(protection_error)?;
            Ok(Zeroizing::new(bytes))
        }
        #[cfg(all(not(test), windows))]
        {
            let bytes = crate::storage::read_with_overflow_byte(path, MAX_ENVELOPE_BYTES as u64)
                .map_err(|_| protection_error())?;
            crate::platform::protected_data::unprotect(
                &bytes,
                "camellia.nexus.intent-history",
                &self.namespace(id, reference),
                MAX_ENVELOPE_BYTES,
            )
            .map(Zeroizing::new)
            .map_err(|_| protection_error())
        }
        #[cfg(all(not(test), not(windows)))]
        {
            let store = OsSecureStore::for_configuration_history(&self.namespace(id, reference))
                .map_err(|_| protection_error())?;
            let bytes = store
                .get_secret(SecretKey::IntentHistoryPayload)
                .map_err(|_| protection_error())?
                .ok_or_else(protection_error)?;
            if bytes.len() > MAX_ENVELOPE_BYTES {
                return Err(protection_error());
            }
            Ok(Zeroizing::new(bytes))
        }
    }

    fn write_bytes(
        &self,
        id: &ProgramId,
        reference: &str,
        path: &Path,
        bytes: &[u8],
    ) -> Result<()> {
        #[cfg(test)]
        {
            if self
                .fail_protection
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(protection_error());
            }
            self.records
                .lock()
                .map_err(|_| protection_error())?
                .insert(self.namespace(id, reference), bytes.to_vec());
            crate::storage::write_bytes_atomic(path, b"")
        }
        #[cfg(all(not(test), windows))]
        {
            let protected = crate::platform::protected_data::protect(
                bytes,
                "camellia.nexus.intent-history",
                &self.namespace(id, reference),
                MAX_ENVELOPE_BYTES,
            )
            .map_err(|_| protection_error())?;
            crate::storage::write_bytes_atomic(path, &protected).map_err(|_| protection_error())
        }
        #[cfg(all(not(test), not(windows)))]
        {
            let store = OsSecureStore::for_configuration_history(&self.namespace(id, reference))
                .map_err(|_| protection_error())?;
            if store.mode() != SecureStoreMode::Persistent {
                return Err(protection_error());
            }
            store
                .put_secret(SecretKey::IntentHistoryPayload, bytes)
                .map_err(|_| protection_error())?;
            if let Err(error) = crate::storage::write_bytes_atomic(path, b"") {
                let _ = store.delete_secret(SecretKey::IntentHistoryPayload);
                return Err(error);
            }
            Ok(())
        }
    }

    fn remove_bytes(&self, id: &ProgramId, reference: &str, path: &Path) -> Result<()> {
        #[cfg(test)]
        {
            self.records
                .lock()
                .map_err(|_| protection_error())?
                .remove(&self.namespace(id, reference));
        }
        #[cfg(all(not(test), not(windows)))]
        {
            let store = OsSecureStore::for_configuration_history(&self.namespace(id, reference))
                .map_err(|_| protection_error())?;
            store
                .delete_secret(SecretKey::IntentHistoryPayload)
                .map_err(|_| protection_error())?;
        }
        #[cfg(all(not(test), windows))]
        let _ = (id, reference);
        fs::remove_file(path).map_err(|_| protection_error())
    }

    #[cfg(all(test, unix))]
    pub(crate) fn fail_next_protection(&self) {
        self.fail_protection
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn valid_reference(reference: &str) -> bool {
    reference.len() == 64
        && reference
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn ensure_regular(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| protection_error())?;
    if !metadata.is_file() || crate::platform::is_link_or_reparse(&metadata) {
        return Err(protection_error());
    }
    Ok(())
}

fn protection_error() -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::Storage, "The change could not be saved safely")
        .with_message_key("INTENT_HISTORY_PROTECTION_FAILED")
}
