use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::BtError;

/// 传输标识即整文件 SHA-256，断点续传与去重共用同一标识。
pub type TransferId = [u8; 32];

pub fn sha256(bytes: &[u8]) -> TransferId {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    to_array(hasher.finalize())
}

/// 流式计算文件摘要，避免大文件整体载入内存。
pub fn sha256_file(path: &Path) -> Result<TransferId, BtError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(to_array(hasher.finalize()))
}

fn to_array(digest: impl AsRef<[u8]>) -> TransferId {
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

pub fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 解析 64 位十六进制摘要，长度不符直接拒绝。
pub fn parse_transfer_id(value: &str) -> Result<TransferId, BtError> {
    let trimmed = value.trim();
    if trimmed.len() != 64 {
        return Err(BtError::invalid("传输标识长度不是 64 位十六进制"));
    }
    let bytes = trimmed.as_bytes();
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        let high = hex_value(bytes[index * 2])?;
        let low = hex_value(bytes[index * 2 + 1])?;
        *slot = high << 4 | low;
    }
    Ok(out)
}

pub fn short_hex(id: &TransferId) -> String {
    hex_lower(&id[..4])
}

fn hex_value(byte: u8) -> Result<u8, BtError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(BtError::invalid("传输标识包含非十六进制字符")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        let digest = sha256(b"abc");
        assert_eq!(
            hex_lower(&digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn transfer_id_round_trips_through_hex() {
        let digest = sha256(b"round-trip");
        let parsed = parse_transfer_id(&hex_lower(&digest)).unwrap();
        assert_eq!(parsed, digest);
        assert!(parse_transfer_id("abcd").is_err());
    }

    #[test]
    fn short_hex_is_first_four_bytes() {
        let digest = [0u8; 32];
        assert_eq!(short_hex(&digest), "00000000");
    }
}
