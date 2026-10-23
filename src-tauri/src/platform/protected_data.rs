use camellia_nexus_licensing::{LicensingError, Result};
use windows::Win32::{
    Foundation::{HLOCAL, LocalFree},
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
};
use zeroize::Zeroize;

pub(crate) fn protect(
    bytes: &[u8],
    domain: &str,
    purpose: &str,
    maximum: usize,
) -> Result<Vec<u8>> {
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(LicensingError::SecureStoreBackend);
    }
    let mut input = blob(bytes)?;
    let entropy_bytes = format!("{domain}\0{purpose}");
    let entropy = blob(entropy_bytes.as_bytes())?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &raw mut input,
            windows::core::PCWSTR::null(),
            Some(&raw const entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output,
        )
    }
    .map_err(|_| LicensingError::SecureStoreBackend)?;
    copy_and_free(output, maximum)
}

pub(crate) fn unprotect(
    bytes: &[u8],
    domain: &str,
    purpose: &str,
    maximum: usize,
) -> Result<Vec<u8>> {
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(LicensingError::SecureStoreBackend);
    }
    let mut input = blob(bytes)?;
    let entropy_bytes = format!("{domain}\0{purpose}");
    let entropy = blob(entropy_bytes.as_bytes())?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &raw mut input,
            None,
            Some(&raw const entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output,
        )
    }
    .map_err(|_| LicensingError::SecureStoreBackend)?;
    copy_and_free(output, maximum)
}

fn blob(bytes: &[u8]) -> Result<CRYPT_INTEGER_BLOB> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(|_| LicensingError::SecureStoreBackend)?,
        pbData: bytes.as_ptr().cast_mut(),
    })
}

fn copy_and_free(output: CRYPT_INTEGER_BLOB, maximum: usize) -> Result<Vec<u8>> {
    let length = output.cbData as usize;
    if output.pbData.is_null() {
        return Err(LicensingError::SecureStoreBackend);
    }
    let bytes = unsafe { std::slice::from_raw_parts_mut(output.pbData, length) };
    let result = if length == 0 || length > maximum {
        Err(LicensingError::SecureStoreBackend)
    } else {
        Ok(bytes.to_vec())
    };
    bytes.zeroize();
    let _ = unsafe { LocalFree(Some(HLOCAL(output.pbData.cast()))) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_user_protection_binds_domain_purpose_and_size() {
        let bytes = b"fixture reverse material";
        let protected = protect(bytes, "camellia.nexus.intent-history", "record", 4096).unwrap();
        assert!(!protected.windows(bytes.len()).any(|window| window == bytes));
        assert_eq!(
            unprotect(&protected, "camellia.nexus.intent-history", "record", 4096).unwrap(),
            bytes
        );
        assert!(unprotect(&protected, "camellia.nexus.other", "record", 4096).is_err());
        assert!(unprotect(&protected, "camellia.nexus.intent-history", "other", 4096).is_err());
        assert!(protect(bytes, "camellia.nexus.intent-history", "record", 1).is_err());
    }
}
