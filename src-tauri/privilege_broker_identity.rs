use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use sha2::{Digest, Sha256};

pub const MAX_PRIVILEGE_BROKER_BYTES: u64 = 128 * 1024 * 1024;

pub fn digest_file_hex(path: &Path) -> io::Result<String> {
    digest_file_hex_with_limit(path, MAX_PRIVILEGE_BROKER_BYTES)
}

fn digest_file_hex_with_limit(path: &Path, maximum_bytes: u64) -> io::Result<String> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "privilege broker is empty, not a regular file, or exceeds the size limit",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(maximum_bytes) as usize);
    file.take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > maximum_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "privilege broker is empty or exceeds the size limit",
        ));
    }
    Ok(digest_hex(&bytes))
}

pub fn digest_hex(bytes: &[u8]) -> String {
    let normalized = normalized_executable(bytes);
    Sha256::digest(normalized)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn normalized_executable(bytes: &[u8]) -> Vec<u8> {
    let mut normalized = bytes.to_vec();
    let _ = normalize_pe_authenticode(&mut normalized)
        || normalize_macho_code_signature(&mut normalized);
    normalized
}

fn normalize_pe_authenticode(bytes: &mut Vec<u8>) -> bool {
    if bytes.get(..2) != Some(b"MZ") {
        return false;
    }
    let Some(pe_offset) = read_u32_le(bytes, 0x3c).map(|value| value as usize) else {
        return false;
    };
    if bytes.get(pe_offset..pe_offset.saturating_add(4)) != Some(b"PE\0\0") {
        return false;
    }
    let optional_header = pe_offset.saturating_add(24);
    let Some(magic) = read_u16_le(bytes, optional_header) else {
        return false;
    };
    let data_directory = match magic {
        0x10b => optional_header.saturating_add(96),
        0x20b => optional_header.saturating_add(112),
        _ => return false,
    };
    let checksum = optional_header.saturating_add(64);
    let certificate_entry = data_directory.saturating_add(4 * 8);
    if certificate_entry.saturating_add(8) > bytes.len() || checksum.saturating_add(4) > bytes.len()
    {
        return false;
    }
    let certificate_offset = read_u32_le(bytes, certificate_entry).unwrap_or_default() as usize;
    let certificate_size =
        read_u32_le(bytes, certificate_entry.saturating_add(4)).unwrap_or_default() as usize;
    bytes[checksum..checksum + 4].fill(0);
    bytes[certificate_entry..certificate_entry + 8].fill(0);
    remove_signature_blob(bytes, certificate_offset, certificate_size);
    true
}

fn normalize_macho_code_signature(bytes: &mut Vec<u8>) -> bool {
    let Some(magic) = bytes.get(..4) else {
        return false;
    };
    let header_size: usize = match magic {
        [0xcf, 0xfa, 0xed, 0xfe] => 32,
        [0xce, 0xfa, 0xed, 0xfe] => 28,
        _ => return false,
    };
    let Some(command_count) = read_u32_le(bytes, 16) else {
        return false;
    };
    let Some(commands_size) = read_u32_le(bytes, 20).map(|value| value as usize) else {
        return false;
    };
    let commands_end = header_size.saturating_add(commands_size);
    if commands_end > bytes.len() {
        return false;
    }
    let mut cursor = header_size;
    for _ in 0..command_count {
        let Some(_command) = read_u32_le(bytes, cursor) else {
            return false;
        };
        let Some(command_size) =
            read_u32_le(bytes, cursor.saturating_add(4)).map(|value| value as usize)
        else {
            return false;
        };
        if command_size < 8 || cursor.saturating_add(command_size) > commands_end {
            return false;
        }
        cursor = cursor.saturating_add(command_size);
    }

    let mut cursor = header_size;
    let mut signature = None;
    for _ in 0..command_count {
        let command = read_u32_le(bytes, cursor).unwrap_or_default();
        let command_size =
            read_u32_le(bytes, cursor.saturating_add(4)).unwrap_or_default() as usize;
        if command == 0x1d && command_size >= 16 {
            let offset = read_u32_le(bytes, cursor + 8).unwrap_or_default() as usize;
            let size = read_u32_le(bytes, cursor + 12).unwrap_or_default() as usize;
            bytes[cursor + 8..cursor + 16].fill(0);
            signature = Some((offset, size));
        }
        cursor = cursor.saturating_add(command_size);
    }
    if let Some((offset, size)) = signature {
        remove_signature_blob(bytes, offset, size);
    }
    true
}

fn remove_signature_blob(bytes: &mut Vec<u8>, offset: usize, size: usize) {
    if offset == 0 || size == 0 {
        return;
    }
    let end = offset.saturating_add(size);
    if offset <= bytes.len() && end <= bytes.len() {
        bytes.drain(offset..end);
    }
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let value: [u8; 2] = bytes
        .get(offset..offset.saturating_add(2))?
        .try_into()
        .ok()?;
    Some(u16::from_le_bytes(value))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let value: [u8; 4] = bytes
        .get(offset..offset.saturating_add(4))?
        .try_into()
        .ok()?;
    Some(u32::from_le_bytes(value))
}

#[cfg(test)]
mod tests {
    use super::{digest_file_hex_with_limit, digest_hex};

    #[test]
    fn file_identity_is_bounded_and_matches_memory_identity() {
        let directory = tempfile::tempdir().expect("tempdir");
        let broker = directory.path().join("broker");
        std::fs::write(&broker, b"MZ-short-broker").expect("write broker");
        assert_eq!(
            digest_file_hex_with_limit(&broker, 64).expect("bounded broker digest"),
            digest_hex(b"MZ-short-broker")
        );
        assert_eq!(
            digest_file_hex_with_limit(&broker, 8)
                .expect_err("oversized broker must fail")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn pe_authenticode_blob_does_not_change_identity() {
        let mut unsigned = vec![0u8; 256];
        unsigned[..2].copy_from_slice(b"MZ");
        unsigned[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        unsigned[64..68].copy_from_slice(b"PE\0\0");
        unsigned[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
        unsigned[200] = 42;

        let mut signed = unsigned.clone();
        signed[88 + 64..88 + 68].copy_from_slice(&123u32.to_le_bytes());
        let certificate_entry = 88 + 112 + 4 * 8;
        signed[certificate_entry..certificate_entry + 4]
            .copy_from_slice(&(unsigned.len() as u32).to_le_bytes());
        signed[certificate_entry + 4..certificate_entry + 8].copy_from_slice(&16u32.to_le_bytes());
        signed.extend_from_slice(&[7u8; 16]);

        assert_eq!(digest_hex(&unsigned), digest_hex(&signed));
        signed[200] ^= 1;
        assert_ne!(digest_hex(&unsigned), digest_hex(&signed));
    }

    #[test]
    fn malformed_executables_fall_back_to_complete_file_identity() {
        assert_ne!(digest_hex(b"MZ-short-a"), digest_hex(b"MZ-short-b"));

        let mut malformed_macho = vec![0_u8; 72];
        malformed_macho[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        malformed_macho[16..20].copy_from_slice(&2_u32.to_le_bytes());
        malformed_macho[20..24].copy_from_slice(&24_u32.to_le_bytes());
        malformed_macho[32..36].copy_from_slice(&0x1d_u32.to_le_bytes());
        malformed_macho[36..40].copy_from_slice(&16_u32.to_le_bytes());
        malformed_macho[40..44].copy_from_slice(&64_u32.to_le_bytes());
        malformed_macho[44..48].copy_from_slice(&8_u32.to_le_bytes());
        malformed_macho[48..52].copy_from_slice(&1_u32.to_le_bytes());
        malformed_macho[52..56].copy_from_slice(&4_u32.to_le_bytes());
        let mut altered = malformed_macho.clone();
        altered[40..48].fill(0);
        assert_ne!(digest_hex(&malformed_macho), digest_hex(&altered));
    }
}
