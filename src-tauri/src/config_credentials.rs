use std::{collections::BTreeMap, fmt::Write as _, path::Path, sync::Arc};

#[cfg(windows)]
use std::{fs, os::windows::fs::MetadataExt, path::PathBuf};

use camellia_nexus_core::{
    CamelliaNexusError, ConfigSourceAuthentication, ConfigSourceSpec, ErrorCode, ProgramManager,
    ProgramSpec, Result,
};
#[cfg(not(windows))]
use camellia_nexus_licensing::OsSecureStore;
use camellia_nexus_licensing::{DynSecureStore, LicensingError, SecretKey};
#[cfg(windows)]
use camellia_nexus_licensing::{SecureStore, SecureStoreMode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const CREDENTIAL_JOURNAL_SCHEMA_VERSION: u32 = 1;

#[cfg(windows)]
const WINDOWS_VAULT_DIRECTORY: &str = "secure/config-credentials-v1";
#[cfg(windows)]
const WINDOWS_MAX_CREDENTIAL_RECORDS: usize = 50 * 50;
#[cfg(windows)]
const WINDOWS_MAX_PROTECTED_FILE_BYTES: usize = 64 * 1024;

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct CredentialRecord {
    program_id: String,
    source_id: String,
    username: String,
    password: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct CredentialSet {
    entries: BTreeMap<String, CredentialRecord>,
}

impl Drop for CredentialSet {
    fn drop(&mut self) {
        self.entries.clear();
    }
}

/// Windows Credential Manager has a small per-entry blob limit, while the
/// product permits up to 50 programs with 50 sources each. Store each dynamic
/// source credential in its own CurrentUser-DPAPI envelope and atomically swap
/// a generation pointer. The recovery backup is another generation pointer,
/// not a second aggregate credential blob.
#[cfg(windows)]
struct WindowsConfigCredentialStore {
    root: PathBuf,
}

#[cfg(windows)]
impl WindowsConfigCredentialStore {
    fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join(WINDOWS_VAULT_DIRECTORY),
        }
    }

    fn generations_dir(&self) -> PathBuf {
        self.root.join("generations")
    }

    fn current_pointer(&self) -> PathBuf {
        self.root.join("current.dpapi")
    }

    fn backup_pointer(&self) -> PathBuf {
        self.root.join("backup.dpapi")
    }

    fn journal_path(&self) -> PathBuf {
        self.root.join("journal.dpapi")
    }

    fn ensure_directories(&self) -> std::result::Result<(), LicensingError> {
        fs::create_dir_all(self.generations_dir()).map_err(|_| windows_vault_error())?;
        ensure_windows_directory(&self.root)?;
        ensure_windows_directory(&self.generations_dir())
    }

    fn generation_path(&self, generation: &str) -> std::result::Result<PathBuf, LicensingError> {
        let generation = canonical_generation(generation)?;
        Ok(self.generations_dir().join(generation))
    }

    fn read_pointer(
        &self,
        path: &Path,
        purpose: &str,
    ) -> std::result::Result<Option<String>, LicensingError> {
        let Some(bytes) = read_dpapi_file(path, purpose)? else {
            return Ok(None);
        };
        let value = std::str::from_utf8(&bytes).map_err(|_| windows_vault_error())?;
        Ok(Some(canonical_generation(value)?))
    }

    fn write_pointer(
        &self,
        path: &Path,
        purpose: &str,
        generation: &str,
    ) -> std::result::Result<(), LicensingError> {
        let generation = canonical_generation(generation)?;
        write_dpapi_file(path, purpose, generation.as_bytes())
    }

    fn read_generation(
        &self,
        generation: &str,
    ) -> std::result::Result<CredentialSet, LicensingError> {
        let path = self.generation_path(generation)?;
        ensure_windows_directory(&path)?;
        let mut credentials = CredentialSet::default();
        for (index, entry) in fs::read_dir(path)
            .map_err(|_| windows_vault_error())?
            .enumerate()
        {
            if index >= WINDOWS_MAX_CREDENTIAL_RECORDS {
                return Err(windows_vault_error());
            }
            let entry = entry.map_err(|_| windows_vault_error())?;
            ensure_windows_regular_file(&entry.path())?;
            let file_name = entry
                .file_name()
                .into_string()
                .map_err(|_| windows_vault_error())?;
            let credential_id = file_name
                .strip_suffix(".dpapi")
                .filter(|value| valid_credential_id(value))
                .ok_or_else(windows_vault_error)?;
            let purpose = format!("credential:{credential_id}");
            let bytes =
                read_dpapi_file(&entry.path(), &purpose)?.ok_or_else(windows_vault_error)?;
            let record: CredentialRecord =
                serde_json::from_slice(&bytes).map_err(|_| windows_vault_error())?;
            if credential_id_for(&record.program_id, &record.source_id) != credential_id
                || credentials
                    .entries
                    .insert(credential_id.to_owned(), record)
                    .is_some()
            {
                return Err(windows_vault_error());
            }
        }
        Ok(credentials)
    }

    fn write_generation(
        &self,
        credentials: &CredentialSet,
    ) -> std::result::Result<String, LicensingError> {
        if credentials.entries.len() > WINDOWS_MAX_CREDENTIAL_RECORDS {
            return Err(windows_vault_error());
        }
        self.ensure_directories()?;
        let generation = uuid::Uuid::new_v4().hyphenated().to_string();
        let path = self.generation_path(&generation)?;
        fs::create_dir(&path).map_err(|_| windows_vault_error())?;
        let result = (|| {
            for (credential_id, record) in &credentials.entries {
                if !valid_credential_id(credential_id)
                    || credential_id_for(&record.program_id, &record.source_id) != *credential_id
                {
                    return Err(windows_vault_error());
                }
                let bytes =
                    Zeroizing::new(serde_json::to_vec(record).map_err(|_| windows_vault_error())?);
                let purpose = format!("credential:{credential_id}");
                write_dpapi_file(
                    &path.join(format!("{credential_id}.dpapi")),
                    &purpose,
                    bytes.as_slice(),
                )?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = remove_windows_generation(&path);
            return Err(error);
        }
        Ok(generation)
    }

    fn serialize_generation(
        &self,
        generation: &str,
    ) -> std::result::Result<Vec<u8>, LicensingError> {
        serde_json::to_vec(&self.read_generation(generation)?).map_err(|_| windows_vault_error())
    }

    fn install_credentials(&self, bytes: &[u8]) -> std::result::Result<(), LicensingError> {
        let credentials: CredentialSet =
            serde_json::from_slice(bytes).map_err(|_| windows_vault_error())?;
        let generation = self.write_generation(&credentials)?;
        if let Err(error) =
            self.write_pointer(&self.current_pointer(), "current-generation", &generation)
        {
            let _ = remove_windows_generation(&self.generation_path(&generation)?);
            return Err(error);
        }
        self.cleanup_generations()
    }

    fn remove_current(&self) -> std::result::Result<(), LicensingError> {
        remove_windows_file(&self.current_pointer())?;
        self.cleanup_generations()
    }

    fn cleanup_generations(&self) -> std::result::Result<(), LicensingError> {
        if !self.generations_dir().exists() {
            return Ok(());
        }
        ensure_windows_directory(&self.generations_dir())?;
        let current = self.read_pointer(&self.current_pointer(), "current-generation")?;
        let backup = self.read_pointer(&self.backup_pointer(), "backup-generation")?;
        for entry in fs::read_dir(self.generations_dir()).map_err(|_| windows_vault_error())? {
            let entry = entry.map_err(|_| windows_vault_error())?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| windows_vault_error())?;
            let name = canonical_generation(&name)?;
            if current.as_deref() != Some(name.as_str()) && backup.as_deref() != Some(name.as_str())
            {
                remove_windows_generation(&entry.path())?;
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
impl SecureStore for WindowsConfigCredentialStore {
    fn mode(&self) -> SecureStoreMode {
        SecureStoreMode::Persistent
    }

    fn get_secret(&self, key: SecretKey) -> std::result::Result<Option<Vec<u8>>, LicensingError> {
        self.ensure_directories()?;
        match key {
            SecretKey::ConfigSourceCredentials => self
                .read_pointer(&self.current_pointer(), "current-generation")?
                .map(|generation| self.serialize_generation(&generation))
                .transpose(),
            SecretKey::ConfigSourceCredentialBackup => self
                .read_pointer(&self.backup_pointer(), "backup-generation")?
                .map(|generation| self.serialize_generation(&generation))
                .transpose(),
            SecretKey::ConfigSourceCredentialJournal => {
                read_dpapi_file(&self.journal_path(), "credential-journal")
            }
            _ => Err(windows_vault_error()),
        }
    }

    fn put_secret(&self, key: SecretKey, value: &[u8]) -> std::result::Result<(), LicensingError> {
        self.ensure_directories()?;
        match key {
            SecretKey::ConfigSourceCredentials => self.install_credentials(value),
            SecretKey::ConfigSourceCredentialBackup => {
                let generation = self
                    .read_pointer(&self.current_pointer(), "current-generation")?
                    .ok_or_else(windows_vault_error)?;
                if self.serialize_generation(&generation)? != value {
                    return Err(windows_vault_error());
                }
                self.write_pointer(&self.backup_pointer(), "backup-generation", &generation)
            }
            SecretKey::ConfigSourceCredentialJournal => {
                write_dpapi_file(&self.journal_path(), "credential-journal", value)
            }
            _ => Err(windows_vault_error()),
        }
    }

    fn delete_secret(&self, key: SecretKey) -> std::result::Result<(), LicensingError> {
        self.ensure_directories()?;
        match key {
            SecretKey::ConfigSourceCredentials => self.remove_current(),
            SecretKey::ConfigSourceCredentialBackup => {
                remove_windows_file(&self.backup_pointer())?;
                self.cleanup_generations()
            }
            SecretKey::ConfigSourceCredentialJournal => remove_windows_file(&self.journal_path()),
            _ => Err(windows_vault_error()),
        }
    }
}

#[cfg(windows)]
fn valid_credential_id(value: &str) -> bool {
    value.len() == 68
        && value.starts_with("cfg-")
        && value[4..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(windows)]
fn canonical_generation(value: &str) -> std::result::Result<String, LicensingError> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| windows_vault_error())?;
    let canonical = parsed.hyphenated().to_string();
    if canonical != value {
        return Err(windows_vault_error());
    }
    Ok(canonical)
}

#[cfg(windows)]
fn ensure_windows_directory(path: &Path) -> std::result::Result<(), LicensingError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| windows_vault_error())?;
    if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err(windows_vault_error());
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_windows_regular_file(path: &Path) -> std::result::Result<(), LicensingError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| windows_vault_error())?;
    if !metadata.is_file() || metadata.file_attributes() & 0x400 != 0 {
        return Err(windows_vault_error());
    }
    Ok(())
}

#[cfg(windows)]
fn remove_windows_file(path: &Path) -> std::result::Result<(), LicensingError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            ensure_windows_regular_file(path)?;
            fs::remove_file(path).map_err(|_| windows_vault_error())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(windows_vault_error()),
    }
}

#[cfg(windows)]
fn remove_windows_generation(path: &Path) -> std::result::Result<(), LicensingError> {
    ensure_windows_directory(path)?;
    fs::remove_dir_all(path).map_err(|_| windows_vault_error())
}

#[cfg(windows)]
fn read_dpapi_file(
    path: &Path,
    purpose: &str,
) -> std::result::Result<Option<Vec<u8>>, LicensingError> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_windows_regular_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(windows_vault_error()),
    }
    let protected = Zeroizing::new(
        crate::storage::read_with_overflow_byte(path, WINDOWS_MAX_PROTECTED_FILE_BYTES as u64)
            .map_err(|_| windows_vault_error())?,
    );
    if protected.is_empty() || protected.len() > WINDOWS_MAX_PROTECTED_FILE_BYTES {
        return Err(windows_vault_error());
    }
    dpapi_unprotect(&protected, purpose).map(Some)
}

#[cfg(windows)]
fn write_dpapi_file(
    path: &Path,
    purpose: &str,
    plaintext: &[u8],
) -> std::result::Result<(), LicensingError> {
    if plaintext.is_empty() || plaintext.len() > WINDOWS_MAX_PROTECTED_FILE_BYTES {
        return Err(windows_vault_error());
    }
    let protected = dpapi_protect(plaintext, purpose)?;
    crate::storage::write_bytes_atomic(path, &protected).map_err(|_| windows_vault_error())
}

#[cfg(windows)]
fn dpapi_protect(plaintext: &[u8], purpose: &str) -> std::result::Result<Vec<u8>, LicensingError> {
    use windows::{
        Win32::Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
        },
        core::PCWSTR,
    };

    let mut plaintext = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plaintext.len()).map_err(|_| windows_vault_error())?,
        pbData: plaintext.as_ptr().cast_mut(),
    };
    let entropy_bytes = format!("camellia.nexus.config-credentials.v1\0{purpose}");
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(entropy_bytes.len()).map_err(|_| windows_vault_error())?,
        pbData: entropy_bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &raw mut plaintext,
            PCWSTR::null(),
            Some(&raw const entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output,
        )
    }
    .map_err(|_| windows_vault_error())?;
    copy_and_free_dpapi_blob(output, WINDOWS_MAX_PROTECTED_FILE_BYTES)
}

#[cfg(windows)]
fn dpapi_unprotect(
    protected: &[u8],
    purpose: &str,
) -> std::result::Result<Vec<u8>, LicensingError> {
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    let mut protected = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(protected.len()).map_err(|_| windows_vault_error())?,
        pbData: protected.as_ptr().cast_mut(),
    };
    let entropy_bytes = format!("camellia.nexus.config-credentials.v1\0{purpose}");
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(entropy_bytes.len()).map_err(|_| windows_vault_error())?,
        pbData: entropy_bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &raw mut protected,
            None,
            Some(&raw const entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output,
        )
    }
    .map_err(|_| windows_vault_error())?;
    copy_and_free_dpapi_blob(output, WINDOWS_MAX_PROTECTED_FILE_BYTES)
}

#[cfg(windows)]
fn copy_and_free_dpapi_blob(
    output: windows::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB,
    maximum: usize,
) -> std::result::Result<Vec<u8>, LicensingError> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};

    let length = usize::try_from(output.cbData).map_err(|_| windows_vault_error())?;
    let result = if output.pbData.is_null() || length == 0 || length > maximum {
        Err(windows_vault_error())
    } else {
        Ok(unsafe { std::slice::from_raw_parts(output.pbData, length) }.to_vec())
    };
    let _ = unsafe { LocalFree(Some(HLOCAL(output.pbData.cast()))) };
    result
}

#[cfg(windows)]
fn windows_vault_error() -> LicensingError {
    LicensingError::SecureStoreBackend
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct CredentialJournal {
    schema_version: u32,
    program_id: String,
    target_binding_digest: Option<String>,
    previous_present: bool,
}

#[derive(Clone)]
pub struct CredentialSnapshot(Arc<CredentialSet>);

impl CredentialSnapshot {
    pub fn empty() -> Self {
        Self(Arc::new(CredentialSet::default()))
    }

    pub fn basic_password(
        &self,
        credential_id: Option<&str>,
        username: &str,
    ) -> Result<Zeroizing<String>> {
        let credential_id = credential_id.ok_or_else(missing_credentials)?;
        let record = self
            .0
            .entries
            .get(credential_id)
            .filter(|record| record.username == username)
            .ok_or_else(missing_credentials)?;
        Ok(Zeroizing::new(record.password.clone()))
    }
}

pub fn has_credentials(spec: &ProgramSpec) -> bool {
    spec.managed_config.as_ref().is_some_and(|managed| {
        managed.sources.iter().any(|source| {
            matches!(
                source,
                ConfigSourceSpec::Remote {
                    authentication: Some(ConfigSourceAuthentication::Basic { .. }),
                    ..
                }
            )
        })
    })
}

pub struct ConfigCredentialVault {
    store: DynSecureStore,
    mutation: tokio::sync::Mutex<()>,
}

impl ConfigCredentialVault {
    pub fn new(data_dir: &Path) -> Self {
        #[cfg(windows)]
        let store: DynSecureStore = Arc::new(WindowsConfigCredentialStore::new(data_dir));
        #[cfg(not(windows))]
        let store: DynSecureStore = {
            let _ = data_dir;
            #[cfg(feature = "desktop-e2e")]
            {
                let namespace = std::env::var("CAMELLIA_NEXUS_E2E_NAMESPACE")
                    .expect("CAMELLIA_NEXUS_E2E_NAMESPACE is required by desktop-e2e builds");
                Arc::new(
                    OsSecureStore::for_test_namespace(&namespace)
                        .expect("CAMELLIA_NEXUS_E2E_NAMESPACE must be a safe credential namespace"),
                )
            }
            #[cfg(not(feature = "desktop-e2e"))]
            Arc::new(OsSecureStore::new())
        };
        Self::with_store(store)
    }

    fn with_store(store: DynSecureStore) -> Self {
        Self {
            store,
            mutation: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn snapshot(&self) -> Result<CredentialSnapshot> {
        let _guard = self.mutation.lock().await;
        self.ensure_ready()?;
        Ok(CredentialSnapshot(Arc::new(self.load()?)))
    }

    pub async fn recover(&self, manager: &ProgramManager) -> Result<bool> {
        let _guard = self.mutation.lock().await;
        let Some(journal) = self.read_journal()? else {
            self.delete_backup()?;
            return Ok(false);
        };
        if journal.schema_version != CREDENTIAL_JOURNAL_SCHEMA_VERSION {
            return Err(corrupt_credential_journal());
        }
        let program_id = camellia_nexus_core::ProgramId::parse(&journal.program_id)
            .map_err(|_| corrupt_credential_journal())?;
        let summaries = manager.list().await;
        let current = if summaries.iter().any(|summary| summary.id == program_id) {
            Some(manager.get(&program_id).await?.0)
        } else {
            None
        };
        let target_matches = self.recover_journal(&journal, current.as_ref())?;
        tracing::warn!(
            program = %program_id,
            committed = target_matches,
            "recovered an interrupted configuration credential update"
        );
        Ok(true)
    }

    fn recover_journal(
        &self,
        journal: &CredentialJournal,
        current: Option<&ProgramSpec>,
    ) -> Result<bool> {
        let target_matches = match (&journal.target_binding_digest, current) {
            (Some(expected), Some(spec)) => credential_binding_digest(spec)? == *expected,
            (None, None) => true,
            _ => false,
        };
        if target_matches {
            let credentials = self.load()?;
            if let Some(spec) = current.as_ref()
                && !credentials_match_spec(&credentials, spec)
            {
                return Err(corrupt_credential_journal());
            }
        } else {
            let previous = self.read_backup(journal.previous_present)?;
            self.restore(previous.as_deref())?;
        }
        self.finish_journal()?;
        Ok(target_matches)
    }

    #[cfg(test)]
    async fn recover_with_current(&self, current: Option<&ProgramSpec>) -> Result<bool> {
        let _guard = self.mutation.lock().await;
        let Some(journal) = self.read_journal()? else {
            self.delete_backup()?;
            return Ok(false);
        };
        if journal.schema_version != CREDENTIAL_JOURNAL_SCHEMA_VERSION {
            return Err(corrupt_credential_journal());
        }
        self.recover_journal(&journal, current)
    }

    pub async fn reconcile<'a>(
        &'a self,
        spec: &mut ProgramSpec,
    ) -> Result<CredentialTransaction<'a>> {
        let guard = self.mutation.lock().await;
        self.ensure_ready()?;
        let old_bytes = self.read_bytes()?;
        let mut credentials = decode(old_bytes.as_deref())?;
        let program_id = spec.id.as_str().to_owned();
        let previous = credentials.clone();
        credentials
            .entries
            .retain(|_, record| record.program_id != program_id);

        if let Some(managed) = spec.managed_config.as_mut() {
            for source in &mut managed.sources {
                let ConfigSourceSpec::Remote {
                    id,
                    authentication:
                        Some(ConfigSourceAuthentication::Basic {
                            username,
                            credential_id,
                            password,
                        }),
                    ..
                } = source
                else {
                    continue;
                };
                let derived_id = credential_id_for(&program_id, id);
                let password = match password.take() {
                    Some(password) if !password.is_empty() => password,
                    _ => previous
                        .entries
                        .get(&derived_id)
                        .filter(|record| record.username == *username)
                        .map(|record| record.password.clone())
                        .ok_or_else(missing_credentials)?,
                };
                credentials.entries.insert(
                    derived_id.clone(),
                    CredentialRecord {
                        program_id: program_id.clone(),
                        source_id: id.clone(),
                        username: username.clone(),
                        password,
                    },
                );
                *credential_id = Some(derived_id);
            }
        }
        self.prepare_journal(
            &program_id,
            Some(credential_binding_digest(spec)?),
            old_bytes.as_deref(),
        )?;
        self.write(&credentials)?;
        let snapshot = CredentialSnapshot(Arc::new(credentials));
        Ok(CredentialTransaction {
            vault: self,
            _guard: guard,
            old_bytes: old_bytes.map(Zeroizing::new),
            snapshot,
            committed: false,
        })
    }

    pub async fn remove_program<'a>(
        &'a self,
        program_id: &str,
    ) -> Result<CredentialTransaction<'a>> {
        let guard = self.mutation.lock().await;
        self.ensure_ready()?;
        let old_bytes = self.read_bytes()?;
        let mut credentials = decode(old_bytes.as_deref())?;
        credentials
            .entries
            .retain(|_, record| record.program_id != program_id);
        self.prepare_journal(program_id, None, old_bytes.as_deref())?;
        self.write(&credentials)?;
        let snapshot = CredentialSnapshot(Arc::new(credentials));
        Ok(CredentialTransaction {
            vault: self,
            _guard: guard,
            old_bytes: old_bytes.map(Zeroizing::new),
            snapshot,
            committed: false,
        })
    }

    fn load(&self) -> Result<CredentialSet> {
        decode(self.read_bytes()?.as_deref())
    }

    fn read_bytes(&self) -> Result<Option<Vec<u8>>> {
        self.store
            .get_secret(SecretKey::ConfigSourceCredentials)
            .map_err(credential_store_error)
    }

    fn ensure_ready(&self) -> Result<()> {
        if self.read_journal()?.is_some() {
            return Err(pending_credential_recovery());
        }
        self.delete_backup()
    }

    fn read_journal(&self) -> Result<Option<CredentialJournal>> {
        self.store
            .get_secret(SecretKey::ConfigSourceCredentialJournal)
            .map_err(credential_store_error)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| corrupt_credential_journal()))
            .transpose()
    }

    fn prepare_journal(
        &self,
        program_id: &str,
        target_binding_digest: Option<String>,
        previous: Option<&[u8]>,
    ) -> Result<()> {
        match previous {
            Some(previous) => self
                .store
                .put_secret(SecretKey::ConfigSourceCredentialBackup, previous)
                .map_err(credential_store_error)?,
            None => self.delete_backup()?,
        }
        let journal = CredentialJournal {
            schema_version: CREDENTIAL_JOURNAL_SCHEMA_VERSION,
            program_id: program_id.to_owned(),
            target_binding_digest,
            previous_present: previous.is_some(),
        };
        let bytes =
            Zeroizing::new(serde_json::to_vec(&journal).map_err(|_| corrupt_credential_journal())?);
        if let Err(error) = self
            .store
            .put_secret(SecretKey::ConfigSourceCredentialJournal, bytes.as_slice())
            .map_err(credential_store_error)
        {
            let _ = self.delete_backup();
            return Err(error);
        }
        Ok(())
    }

    fn read_backup(&self, expected: bool) -> Result<Option<Vec<u8>>> {
        let backup = self
            .store
            .get_secret(SecretKey::ConfigSourceCredentialBackup)
            .map_err(credential_store_error)?;
        if expected {
            backup.map(Some).ok_or_else(corrupt_credential_journal)
        } else {
            Ok(None)
        }
    }

    fn delete_backup(&self) -> Result<()> {
        self.store
            .delete_secret(SecretKey::ConfigSourceCredentialBackup)
            .map_err(credential_store_error)
    }

    fn finish_journal(&self) -> Result<()> {
        self.store
            .delete_secret(SecretKey::ConfigSourceCredentialJournal)
            .map_err(credential_store_error)?;
        self.delete_backup()
    }

    fn rollback_journal(&self, previous: Option<&[u8]>) -> Result<()> {
        self.restore(previous)?;
        self.finish_journal()
    }

    fn write(&self, credentials: &CredentialSet) -> Result<()> {
        if credentials.entries.is_empty() {
            return self
                .store
                .delete_secret(SecretKey::ConfigSourceCredentials)
                .map_err(credential_store_error);
        }
        let bytes =
            Zeroizing::new(serde_json::to_vec(credentials).map_err(|_| corrupt_credentials())?);
        self.store
            .put_secret(SecretKey::ConfigSourceCredentials, bytes.as_slice())
            .map_err(credential_store_error)
    }

    fn restore(&self, bytes: Option<&[u8]>) -> Result<()> {
        match bytes {
            Some(bytes) => self
                .store
                .put_secret(SecretKey::ConfigSourceCredentials, bytes)
                .map_err(credential_store_error),
            None => self
                .store
                .delete_secret(SecretKey::ConfigSourceCredentials)
                .map_err(credential_store_error),
        }
    }
}

pub struct CredentialTransaction<'a> {
    vault: &'a ConfigCredentialVault,
    _guard: tokio::sync::MutexGuard<'a, ()>,
    old_bytes: Option<Zeroizing<Vec<u8>>>,
    snapshot: CredentialSnapshot,
    committed: bool,
}

impl CredentialTransaction<'_> {
    pub fn snapshot(&self) -> &CredentialSnapshot {
        &self.snapshot
    }

    pub fn commit(mut self) -> Result<()> {
        self.committed = true;
        self.vault.finish_journal()
    }

    pub fn rollback(mut self) -> Result<()> {
        let result = self
            .vault
            .rollback_journal(self.old_bytes.as_deref().map(AsRef::as_ref));
        self.committed = true;
        result
    }

    #[cfg(test)]
    fn abandon(mut self) {
        self.committed = true;
    }
}

impl Drop for CredentialTransaction<'_> {
    fn drop(&mut self) {
        if !self.committed
            && let Err(error) = self
                .vault
                .rollback_journal(self.old_bytes.as_deref().map(AsRef::as_ref))
        {
            tracing::error!(%error, "failed to roll back configuration source credentials");
        }
    }
}

fn credential_id_for(program_id: &str, source_id: &str) -> String {
    let digest = Sha256::digest(format!("{program_id}\0{source_id}").as_bytes());
    format_digest("cfg-", &digest)
}

fn credential_binding_digest(spec: &ProgramSpec) -> Result<String> {
    let bindings = credential_bindings(spec)?;
    let bytes = Zeroizing::new(
        serde_json::to_vec(&(spec.id.as_str(), bindings)).map_err(|_| corrupt_credentials())?,
    );
    Ok(format_digest("sha256:", &Sha256::digest(bytes.as_slice())))
}

fn credential_bindings(spec: &ProgramSpec) -> Result<BTreeMap<String, (String, String)>> {
    let mut bindings = BTreeMap::new();
    if let Some(managed) = spec.managed_config.as_ref() {
        for source in &managed.sources {
            let ConfigSourceSpec::Remote {
                id,
                authentication:
                    Some(ConfigSourceAuthentication::Basic {
                        username,
                        credential_id,
                        ..
                    }),
                ..
            } = source
            else {
                continue;
            };
            let credential_id = credential_id.as_ref().ok_or_else(corrupt_credentials)?;
            if bindings
                .insert(id.clone(), (username.clone(), credential_id.clone()))
                .is_some()
            {
                return Err(corrupt_credentials());
            }
        }
    }
    Ok(bindings)
}

fn credentials_match_spec(credentials: &CredentialSet, spec: &ProgramSpec) -> bool {
    let Ok(expected) = credential_bindings(spec) else {
        return false;
    };
    let actual = credentials
        .entries
        .iter()
        .filter(|(_, record)| record.program_id == spec.id.as_str())
        .map(|(credential_id, record)| {
            (
                record.source_id.clone(),
                (record.username.clone(), credential_id.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    actual.len() == expected.len() && actual == expected
}

fn format_digest(prefix: &str, digest: &[u8]) -> String {
    let mut id = String::with_capacity(prefix.len() + digest.len() * 2);
    id.push_str(prefix);
    for byte in digest {
        write!(id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    id
}

fn decode(bytes: Option<&[u8]>) -> Result<CredentialSet> {
    bytes
        .map(|bytes| serde_json::from_slice(bytes).map_err(|_| corrupt_credentials()))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn missing_credentials() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::Storage,
        "The remote configuration credential is unavailable",
    )
    .with_details("Enter the Basic authentication password again and save the program")
}

fn corrupt_credentials() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::Storage,
        "Remote configuration credentials are corrupt",
    )
}

fn pending_credential_recovery() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::Storage,
        "A configuration credential update needs recovery",
    )
    .with_details("Restart Camellia Nexus before changing remote configuration credentials again")
}

fn corrupt_credential_journal() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::Storage,
        "Configuration credential recovery data is corrupt",
    )
}

fn credential_store_error(error: LicensingError) -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::Storage,
        "The operating-system credential store is unavailable",
    )
    .with_details(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camellia_nexus_core::{
        ExecutableSpec, ManagedConfigSpec, ProgramId, ProgramType, RestartPolicy, SCHEMA_VERSION,
    };
    use camellia_nexus_licensing::SessionSecureStore;

    fn spec(password: Option<&str>) -> ProgramSpec {
        ProgramSpec {
            schema_version: SCHEMA_VERSION,
            id: ProgramId::parse("credential-test").expect("id"),
            name: "Credential test".into(),
            executable: ExecutableSpec::External {
                path: "/opt/example/program".into(),
                metadata: None,
            },
            program_type: ProgramType::Generic { args: Vec::new() },
            managed_config: Some(ManagedConfigSpec {
                sources: vec![ConfigSourceSpec::Remote {
                    id: "primary".into(),
                    name: "Primary".into(),
                    enabled: true,
                    url: "https://example.test/config.json".into(),
                    authentication: Some(ConfigSourceAuthentication::Basic {
                        username: "subscriber".into(),
                        credential_id: None,
                        password: password.map(str::to_owned),
                    }),
                }],
                remote_update: None,
                sing_box_dashboard: None,
                sing_box_clash_dashboard: None,
                xray_dashboard: None,
                mihomo_dashboard: None,
            }),
            working_directory: "/opt/example".into(),
            environment: BTreeMap::new(),
            auto_start: false,
            restart_policy: RestartPolicy::Never,
            privilege_policy: Default::default(),
        }
    }

    #[tokio::test]
    async fn passwords_are_replaced_by_bound_credential_references() {
        let vault = ConfigCredentialVault::with_store(Arc::new(SessionSecureStore::default()));
        let mut spec = spec(Some("secret"));
        let transaction = vault.reconcile(&mut spec).await.expect("reconcile");
        let authentication = match &spec.managed_config.as_ref().expect("managed").sources[0] {
            ConfigSourceSpec::Remote { authentication, .. } => {
                authentication.as_ref().expect("auth")
            }
            _ => panic!("remote source expected"),
        };
        let ConfigSourceAuthentication::Basic {
            credential_id,
            password,
            username,
        } = authentication;
        assert!(
            credential_id
                .as_deref()
                .is_some_and(|id| id.starts_with("cfg-"))
        );
        assert!(password.is_none());
        assert_eq!(
            transaction
                .snapshot()
                .basic_password(credential_id.as_deref(), username)
                .expect("password")
                .as_str(),
            "secret"
        );
        let serialized = serde_json::to_string(&spec).expect("serialize");
        assert!(!serialized.contains("secret"));
        transaction.commit().expect("commit");
    }

    #[tokio::test]
    async fn rollback_restores_the_previous_password() {
        let vault = ConfigCredentialVault::with_store(Arc::new(SessionSecureStore::default()));
        let mut initial = spec(Some("first"));
        vault
            .reconcile(&mut initial)
            .await
            .expect("initial")
            .commit()
            .expect("initial commit");
        let credential_id = match &initial.managed_config.as_ref().expect("managed").sources[0] {
            ConfigSourceSpec::Remote {
                authentication: Some(ConfigSourceAuthentication::Basic { credential_id, .. }),
                ..
            } => credential_id.clone(),
            _ => panic!("authentication expected"),
        };
        let mut changed = initial.clone();
        if let ConfigSourceSpec::Remote {
            authentication: Some(ConfigSourceAuthentication::Basic { password, .. }),
            ..
        } = &mut changed.managed_config.as_mut().expect("managed").sources[0]
        {
            *password = Some("second".into());
        }
        vault
            .reconcile(&mut changed)
            .await
            .expect("changed")
            .rollback()
            .expect("rollback");
        assert_eq!(
            vault
                .snapshot()
                .await
                .expect("snapshot")
                .basic_password(credential_id.as_deref(), "subscriber")
                .expect("password")
                .as_str(),
            "first"
        );
    }

    #[tokio::test]
    async fn crash_recovery_restores_credentials_when_the_program_update_did_not_commit() {
        let vault = ConfigCredentialVault::with_store(Arc::new(SessionSecureStore::default()));
        let mut initial = spec(Some("first"));
        vault
            .reconcile(&mut initial)
            .await
            .expect("initial")
            .commit()
            .expect("initial commit");
        let credential_id = basic_credential_id(&initial);
        let mut changed = initial.clone();
        set_basic_authentication(&mut changed, "replacement", "second");
        vault
            .reconcile(&mut changed)
            .await
            .expect("changed")
            .abandon();

        assert!(
            !vault
                .recover_with_current(Some(&initial))
                .await
                .expect("recover")
        );
        let snapshot = vault.snapshot().await.expect("snapshot");
        assert_eq!(
            snapshot
                .basic_password(Some(&credential_id), "subscriber")
                .expect("original password")
                .as_str(),
            "first"
        );
        assert!(
            snapshot
                .basic_password(Some(&credential_id), "replacement")
                .is_err()
        );
    }

    #[tokio::test]
    async fn crash_recovery_keeps_credentials_when_the_program_update_committed() {
        let vault = ConfigCredentialVault::with_store(Arc::new(SessionSecureStore::default()));
        let mut initial = spec(Some("first"));
        vault
            .reconcile(&mut initial)
            .await
            .expect("initial")
            .commit()
            .expect("initial commit");
        let mut changed = initial.clone();
        set_basic_authentication(&mut changed, "replacement", "second");
        vault
            .reconcile(&mut changed)
            .await
            .expect("changed")
            .abandon();

        assert!(
            vault
                .recover_with_current(Some(&changed))
                .await
                .expect("recover")
        );
        assert_eq!(
            vault
                .snapshot()
                .await
                .expect("snapshot")
                .basic_password(Some(&basic_credential_id(&changed)), "replacement")
                .expect("replacement password")
                .as_str(),
            "second"
        );
    }

    #[tokio::test]
    async fn crash_recovery_removes_credentials_for_an_uncommitted_program_creation() {
        let vault = ConfigCredentialVault::with_store(Arc::new(SessionSecureStore::default()));
        let mut created = spec(Some("secret"));
        vault
            .reconcile(&mut created)
            .await
            .expect("create credentials")
            .abandon();

        assert!(!vault.recover_with_current(None).await.expect("recover"));
        assert!(
            vault
                .snapshot()
                .await
                .expect("snapshot")
                .basic_password(Some(&basic_credential_id(&created)), "subscriber")
                .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_dpapi_vault_shards_boundary_sized_credentials_and_keeps_pointer_backup() {
        let directory = tempfile::tempdir().expect("temporary vault");
        let store = WindowsConfigCredentialStore::new(directory.path());
        let entries = (0..50)
            .map(|index| {
                let source_id = format!("source-{index}");
                let record = CredentialRecord {
                    program_id: "team-program".into(),
                    source_id: source_id.clone(),
                    username: format!("subscriber-{index}"),
                    password: "s".repeat(4096),
                };
                (credential_id_for("team-program", &source_id), record)
            })
            .collect();
        let original = CredentialSet { entries };
        let original_bytes = serde_json::to_vec(&original).expect("serialize credentials");

        store
            .put_secret(SecretKey::ConfigSourceCredentials, &original_bytes)
            .expect("install credentials");
        store
            .put_secret(SecretKey::ConfigSourceCredentialBackup, &original_bytes)
            .expect("backup pointer");
        let mut replacement = original.clone();
        replacement
            .entries
            .values_mut()
            .next()
            .expect("record")
            .password = "replacement".into();
        store
            .put_secret(
                SecretKey::ConfigSourceCredentials,
                &serde_json::to_vec(&replacement).expect("serialize replacement"),
            )
            .expect("replace credentials");

        let current: CredentialSet = serde_json::from_slice(
            &store
                .get_secret(SecretKey::ConfigSourceCredentials)
                .expect("read current")
                .expect("current"),
        )
        .expect("decode current");
        let backup: CredentialSet = serde_json::from_slice(
            &store
                .get_secret(SecretKey::ConfigSourceCredentialBackup)
                .expect("read backup")
                .expect("backup"),
        )
        .expect("decode backup");
        assert_eq!(current.entries.len(), 50);
        assert_eq!(backup.entries.len(), 50);
        assert!(
            backup
                .entries
                .values()
                .all(|record| record.password.len() == 4096)
        );
        assert_eq!(
            fs::read_dir(store.generations_dir())
                .expect("generations")
                .count(),
            2
        );

        store
            .delete_secret(SecretKey::ConfigSourceCredentialBackup)
            .expect("delete backup pointer");
        assert_eq!(
            fs::read_dir(store.generations_dir())
                .expect("generations")
                .count(),
            1
        );
    }

    fn basic_credential_id(spec: &ProgramSpec) -> String {
        match &spec.managed_config.as_ref().expect("managed").sources[0] {
            ConfigSourceSpec::Remote {
                authentication: Some(ConfigSourceAuthentication::Basic { credential_id, .. }),
                ..
            } => credential_id.clone().expect("credential id"),
            _ => panic!("authentication expected"),
        }
    }

    fn set_basic_authentication(spec: &mut ProgramSpec, username: &str, password: &str) {
        match &mut spec.managed_config.as_mut().expect("managed").sources[0] {
            ConfigSourceSpec::Remote {
                authentication:
                    Some(ConfigSourceAuthentication::Basic {
                        username: current_username,
                        password: current_password,
                        ..
                    }),
                ..
            } => {
                *current_username = username.to_owned();
                *current_password = Some(password.to_owned());
            }
            _ => panic!("authentication expected"),
        }
    }
}
