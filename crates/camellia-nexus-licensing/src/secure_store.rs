use std::sync::Arc;

#[cfg(all(
    feature = "os-secure-store",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
use std::borrow::Cow;

use serde::{Serialize, de::DeserializeOwned};

use crate::{LicensingError, Result};

#[cfg(all(
    feature = "os-secure-store",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
const SERVICE_NAME: &str = "com.camellia.nexus.licensing";

#[cfg(all(
    feature = "os-secure-store",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
#[derive(Debug)]
pub struct OsSecureStore {
    service_name: Cow<'static, str>,
}

#[cfg(all(
    feature = "os-secure-store",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
impl Default for OsSecureStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(
    feature = "os-secure-store",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
impl OsSecureStore {
    pub const fn new() -> Self {
        Self {
            service_name: Cow::Borrowed(SERVICE_NAME),
        }
    }

    /// Creates a credential namespace that cannot overlap with the production store.
    /// This constructor is available only to test-support builds.
    #[cfg(feature = "test-support")]
    pub fn for_test_namespace(namespace: &str) -> Result<Self> {
        if namespace.is_empty()
            || namespace.len() > 64
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(LicensingError::SecureStoreBackend);
        }
        Ok(Self {
            service_name: Cow::Owned(format!("{SERVICE_NAME}.E2E.{namespace}")),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureStoreMode {
    Persistent,
    SessionOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKey {
    RefreshSession,
    DevicePrivateKey,
    EntitlementLease,
    TrustedTime,
    DeviceRegistration,
    RefreshMetadata,
    LicenseDenial,
    RevalidationMarker,
    ClientUpgradeMarker,
    AuthorizationBlock,
    ConfigSourceCredentials,
    ConfigSourceCredentialJournal,
    ConfigSourceCredentialBackup,
}

impl SecretKey {
    const fn account(self) -> &'static str {
        match self {
            Self::RefreshSession => "refresh-session",
            Self::DevicePrivateKey => "device-private-key",
            Self::EntitlementLease => "entitlement-lease",
            Self::TrustedTime => "trusted-time",
            Self::DeviceRegistration => "device-registration",
            Self::RefreshMetadata => "refresh-metadata",
            Self::LicenseDenial => "license-denial",
            Self::RevalidationMarker => "revalidation-marker",
            Self::ClientUpgradeMarker => "client-upgrade-marker",
            Self::AuthorizationBlock => "authorization-block",
            Self::ConfigSourceCredentials => "config-source-credentials",
            Self::ConfigSourceCredentialJournal => "config-source-credential-journal",
            Self::ConfigSourceCredentialBackup => "config-source-credential-backup",
        }
    }
}

pub trait SecureStore: Send + Sync {
    fn mode(&self) -> SecureStoreMode;
    fn get_secret(&self, key: SecretKey) -> Result<Option<Vec<u8>>>;
    fn put_secret(&self, key: SecretKey, value: &[u8]) -> Result<()>;
    fn delete_secret(&self, key: SecretKey) -> Result<()>;
}

pub type DynSecureStore = Arc<dyn SecureStore>;

pub fn get_json<T: DeserializeOwned>(store: &dyn SecureStore, key: SecretKey) -> Result<Option<T>> {
    store
        .get_secret(key)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| LicensingError::SecureStoreCorrupt))
        .transpose()
}

pub fn put_json<T: Serialize>(store: &dyn SecureStore, key: SecretKey, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| LicensingError::SecureStoreCorrupt)?;
    store.put_secret(key, &bytes)
}

#[cfg(all(feature = "os-secure-store", target_os = "linux"))]
#[cfg(all(feature = "os-secure-store", target_os = "linux"))]
impl OsSecureStore {
    fn attributes(&self, key: SecretKey) -> std::collections::HashMap<&str, &str> {
        std::collections::HashMap::from([
            ("application", self.service_name.as_ref()),
            ("account", key.account()),
        ])
    }
}

#[cfg(all(feature = "os-secure-store", target_os = "linux"))]
impl SecureStore for OsSecureStore {
    fn mode(&self) -> SecureStoreMode {
        SecureStoreMode::Persistent
    }

    fn get_secret(&self, key: SecretKey) -> Result<Option<Vec<u8>>> {
        use secret_service::{EncryptionType, blocking::SecretService};

        let service = SecretService::connect(EncryptionType::Dh)
            .map_err(|_| LicensingError::SecureStoreUnavailable)?;
        let mut items = service
            .search_items(self.attributes(key))
            .map_err(|_| LicensingError::SecureStoreUnavailable)?;
        if let Some(item) = items.unlocked.pop() {
            return item
                .get_secret()
                .map(Some)
                .map_err(|_| LicensingError::SecureStoreBackend);
        }
        if let Some(item) = items.locked.pop() {
            item.unlock()
                .map_err(|_| LicensingError::SecureStoreUnavailable)?;
            return item
                .get_secret()
                .map(Some)
                .map_err(|_| LicensingError::SecureStoreBackend);
        }
        Ok(None)
    }

    fn put_secret(&self, key: SecretKey, value: &[u8]) -> Result<()> {
        use secret_service::{EncryptionType, blocking::SecretService};

        let service = SecretService::connect(EncryptionType::Dh)
            .map_err(|_| LicensingError::SecureStoreUnavailable)?;
        let collection = service
            .get_default_collection()
            .map_err(|_| LicensingError::SecureStoreUnavailable)?;
        collection
            .create_item(
                "Camellia Nexus membership credential",
                self.attributes(key),
                value,
                true,
                "application/octet-stream",
            )
            .map(|_| ())
            .map_err(|_| LicensingError::SecureStoreBackend)
    }

    fn delete_secret(&self, key: SecretKey) -> Result<()> {
        use secret_service::{EncryptionType, blocking::SecretService};

        let service = SecretService::connect(EncryptionType::Dh)
            .map_err(|_| LicensingError::SecureStoreUnavailable)?;
        let items = service
            .search_items(self.attributes(key))
            .map_err(|_| LicensingError::SecureStoreUnavailable)?;
        for item in items.unlocked.into_iter().chain(items.locked) {
            if item.is_locked().unwrap_or(true) {
                item.unlock()
                    .map_err(|_| LicensingError::SecureStoreUnavailable)?;
            }
            item.delete()
                .map_err(|_| LicensingError::SecureStoreBackend)?;
        }
        Ok(())
    }
}

#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
impl OsSecureStore {
    fn find(&self, key: SecretKey) -> Result<Option<MacKeychainItem>> {
        let service = self.service_name.as_bytes();
        let account = key.account().as_bytes();
        let mut length = 0_u32;
        let mut data = std::ptr::null_mut();
        let mut item: SecKeychainItemRef = std::ptr::null();
        // SAFETY: byte slices are valid for the call and all output pointers point
        // to initialized storage. Security.framework owns returned allocations.
        let status = unsafe {
            SecKeychainFindGenericPassword(
                std::ptr::null(),
                service.len() as u32,
                service.as_ptr().cast(),
                account.len() as u32,
                account.as_ptr().cast(),
                &mut length,
                &mut data,
                &mut item,
            )
        };
        if status == ERR_SEC_ITEM_NOT_FOUND {
            return Ok(None);
        }
        if status == ERR_SEC_INTERACTION_NOT_ALLOWED {
            return Err(LicensingError::SecureStoreUnavailable);
        }
        if status != 0 || item.is_null() {
            return Err(LicensingError::SecureStoreBackend);
        }
        Ok(Some(MacKeychainItem { item, data, length }))
    }
}

#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
impl SecureStore for OsSecureStore {
    fn mode(&self) -> SecureStoreMode {
        SecureStoreMode::Persistent
    }

    fn get_secret(&self, key: SecretKey) -> Result<Option<Vec<u8>>> {
        let Some(item) = self.find(key)? else {
            return Ok(None);
        };
        if item.length == 0 {
            return Ok(Some(Vec::new()));
        }
        if item.data.is_null() {
            return Err(LicensingError::SecureStoreBackend);
        }
        // SAFETY: Security.framework returned `length` bytes at `data`, retained
        // until MacKeychainItem drops and frees the content.
        Ok(Some(unsafe {
            std::slice::from_raw_parts(item.data.cast::<u8>(), item.length as usize).to_vec()
        }))
    }

    fn put_secret(&self, key: SecretKey, value: &[u8]) -> Result<()> {
        let length = u32::try_from(value.len()).map_err(|_| LicensingError::SecureStoreBackend)?;
        if let Some(item) = self.find(key)? {
            // SAFETY: item is a live SecKeychainItemRef and value is valid for the call.
            let status = unsafe {
                SecKeychainItemModifyAttributesAndData(
                    item.item,
                    std::ptr::null(),
                    length,
                    value.as_ptr().cast(),
                )
            };
            return status_result(status);
        }
        let service = self.service_name.as_bytes();
        let account = key.account().as_bytes();
        // SAFETY: all input buffers are valid for the duration of the call. The
        // created item is retained by the default Keychain.
        let status = unsafe {
            SecKeychainAddGenericPassword(
                std::ptr::null(),
                service.len() as u32,
                service.as_ptr().cast(),
                account.len() as u32,
                account.as_ptr().cast(),
                length,
                value.as_ptr().cast(),
                std::ptr::null_mut(),
            )
        };
        status_result(status)
    }

    fn delete_secret(&self, key: SecretKey) -> Result<()> {
        let Some(item) = self.find(key)? else {
            return Ok(());
        };
        // SAFETY: item is a live SecKeychainItemRef owned by the drop guard.
        status_result(unsafe { SecKeychainItemDelete(item.item) })
    }
}

#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
type OSStatus = i32;
#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
type SecKeychainItemRef = *const std::ffi::c_void;
#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25_300;
#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
const ERR_SEC_INTERACTION_NOT_ALLOWED: OSStatus = -25_308;

#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
struct MacKeychainItem {
    item: SecKeychainItemRef,
    data: *mut std::ffi::c_void,
    length: u32,
}

#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
impl Drop for MacKeychainItem {
    fn drop(&mut self) {
        // SAFETY: both pointers originate from Security.framework and are released
        // exactly once by this guard.
        unsafe {
            if !self.data.is_null() {
                let _ = SecKeychainItemFreeContent(std::ptr::null(), self.data);
            }
            CFRelease(self.item);
        }
    }
}

#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
fn status_result(status: OSStatus) -> Result<()> {
    match status {
        0 => Ok(()),
        ERR_SEC_INTERACTION_NOT_ALLOWED => Err(LicensingError::SecureStoreUnavailable),
        _ => Err(LicensingError::SecureStoreBackend),
    }
}

#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecKeychainFindGenericPassword(
        keychain: *const std::ffi::c_void,
        service_name_length: u32,
        service_name: *const std::ffi::c_void,
        account_name_length: u32,
        account_name: *const std::ffi::c_void,
        password_length: *mut u32,
        password_data: *mut *mut std::ffi::c_void,
        item_ref: *mut SecKeychainItemRef,
    ) -> OSStatus;
    fn SecKeychainAddGenericPassword(
        keychain: *const std::ffi::c_void,
        service_name_length: u32,
        service_name: *const std::ffi::c_void,
        account_name_length: u32,
        account_name: *const std::ffi::c_void,
        password_length: u32,
        password_data: *const std::ffi::c_void,
        item_ref: *mut SecKeychainItemRef,
    ) -> OSStatus;
    fn SecKeychainItemModifyAttributesAndData(
        item_ref: SecKeychainItemRef,
        attributes: *const std::ffi::c_void,
        length: u32,
        data: *const std::ffi::c_void,
    ) -> OSStatus;
    fn SecKeychainItemDelete(item_ref: SecKeychainItemRef) -> OSStatus;
    fn SecKeychainItemFreeContent(
        attributes: *const std::ffi::c_void,
        data: *mut std::ffi::c_void,
    ) -> OSStatus;
}

#[cfg(all(feature = "os-secure-store", target_os = "macos"))]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *const std::ffi::c_void);
}

#[cfg(all(feature = "os-secure-store", windows))]
#[cfg(all(feature = "os-secure-store", windows))]
impl OsSecureStore {
    fn target(&self, key: SecretKey) -> Vec<u16> {
        format!("{}/{}", self.service_name, key.account())
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }
}

#[cfg(all(feature = "os-secure-store", windows))]
impl SecureStore for OsSecureStore {
    fn mode(&self) -> SecureStoreMode {
        SecureStoreMode::Persistent
    }

    fn get_secret(&self, key: SecretKey) -> Result<Option<Vec<u8>>> {
        use windows::{
            Win32::{
                Foundation::{ERROR_NO_SUCH_LOGON_SESSION, ERROR_NOT_FOUND},
                Security::Credentials::{CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW},
            },
            core::{HRESULT, PCWSTR},
        };

        let target = self.target(key);
        let mut credential = std::ptr::null_mut::<CREDENTIALW>();
        // SAFETY: the target is a terminated UTF-16 buffer and Windows owns the
        // returned credential until it is released with CredFree below.
        match unsafe {
            CredReadW(
                PCWSTR(target.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut credential,
            )
        } {
            Ok(()) => {
                if credential.is_null() {
                    return Err(LicensingError::SecureStoreBackend);
                }
                // SAFETY: CredReadW returned a valid CREDENTIALW allocation and its
                // blob remains valid until CredFree.
                let secret = unsafe {
                    let value = &*credential;
                    if value.CredentialBlobSize == 0 {
                        Vec::new()
                    } else {
                        std::slice::from_raw_parts(
                            value.CredentialBlob,
                            value.CredentialBlobSize as usize,
                        )
                        .to_vec()
                    }
                };
                // SAFETY: credential is the allocation returned by CredReadW.
                unsafe { CredFree(credential.cast()) };
                Ok(Some(secret))
            }
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(None),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NO_SUCH_LOGON_SESSION.0) => {
                Err(LicensingError::SecureStoreUnavailable)
            }
            Err(_) => Err(LicensingError::SecureStoreBackend),
        }
    }

    fn put_secret(&self, key: SecretKey, value: &[u8]) -> Result<()> {
        use windows::{
            Win32::Security::Credentials::{
                CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredWriteW,
            },
            core::PWSTR,
        };

        let mut target = self.target(key);
        let size = u32::try_from(value.len()).map_err(|_| LicensingError::SecureStoreBackend)?;
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            CredentialBlobSize: size,
            CredentialBlob: value.as_ptr().cast_mut(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // SAFETY: all pointers reference live buffers for the duration of the call;
        // CredWriteW copies the credential into Windows Credential Manager.
        unsafe { CredWriteW(&credential, 0) }.map_err(|_| LicensingError::SecureStoreBackend)
    }

    fn delete_secret(&self, key: SecretKey) -> Result<()> {
        use windows::{
            Win32::{
                Foundation::ERROR_NOT_FOUND,
                Security::Credentials::{CRED_TYPE_GENERIC, CredDeleteW},
            },
            core::{HRESULT, PCWSTR},
        };

        let target = self.target(key);
        // SAFETY: target is a terminated UTF-16 buffer valid for the call.
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(()),
            Err(_) => Err(LicensingError::SecureStoreBackend),
        }
    }
}

/// Volatile fallback used when a platform keyring is unavailable. It never claims
/// persistence and therefore cannot grant offline continuity.
#[derive(Debug, Default)]
pub struct SessionSecureStore {
    values: std::sync::RwLock<std::collections::BTreeMap<&'static str, Vec<u8>>>,
}

impl SecureStore for SessionSecureStore {
    fn mode(&self) -> SecureStoreMode {
        SecureStoreMode::SessionOnly
    }

    fn get_secret(&self, key: SecretKey) -> Result<Option<Vec<u8>>> {
        Ok(self
            .values
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key.account())
            .cloned())
    }

    fn put_secret(&self, key: SecretKey, value: &[u8]) -> Result<()> {
        self.values
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.account(), value.to_vec());
        Ok(())
    }

    fn delete_secret(&self, key: SecretKey) -> Result<()> {
        self.values
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key.account());
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
pub type InMemorySecureStore = SessionSecureStore;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_store_never_claims_persistence() {
        let store = SessionSecureStore::default();
        store
            .put_secret(SecretKey::RefreshSession, b"secret")
            .expect("put");
        assert_eq!(store.mode(), SecureStoreMode::SessionOnly);
        assert_eq!(
            store.get_secret(SecretKey::RefreshSession).expect("get"),
            Some(b"secret".to_vec())
        );
    }

    #[cfg(all(
        feature = "test-support",
        feature = "os-secure-store",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    #[test]
    fn test_namespace_is_bounded_and_separate_from_production() {
        let store = OsSecureStore::for_test_namespace("run_01-safe").expect("test store");
        assert_eq!(
            store.service_name.as_ref(),
            "com.camellia.nexus.licensing.E2E.run_01-safe"
        );
        for invalid in ["", "contains space", "../escape", &"a".repeat(65)] {
            assert!(OsSecureStore::for_test_namespace(invalid).is_err());
        }
    }

    struct UnavailableStore;

    impl SecureStore for UnavailableStore {
        fn mode(&self) -> SecureStoreMode {
            SecureStoreMode::Persistent
        }

        fn get_secret(&self, _key: SecretKey) -> Result<Option<Vec<u8>>> {
            Err(LicensingError::SecureStoreUnavailable)
        }

        fn put_secret(&self, _key: SecretKey, _value: &[u8]) -> Result<()> {
            Err(LicensingError::SecureStoreUnavailable)
        }

        fn delete_secret(&self, _key: SecretKey) -> Result<()> {
            Err(LicensingError::SecureStoreUnavailable)
        }
    }

    #[test]
    fn unavailable_store_never_falls_back_to_plaintext() {
        let store = UnavailableStore;
        assert!(matches!(
            store.put_secret(SecretKey::RefreshSession, b"secret"),
            Err(LicensingError::SecureStoreUnavailable)
        ));
        assert!(matches!(
            store.get_secret(SecretKey::DevicePrivateKey),
            Err(LicensingError::SecureStoreUnavailable)
        ));
    }
}
