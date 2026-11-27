use bt_core::{BtError, TransferId};

pub const SIDECAR_MAGIC: [u8; 4] = *b"BTS1";
/// 魔数 4 + 传输标识 32 + 已落盘偏移 8 + 总长 8 + 状态位 1。
pub const SIDECAR_LEN: usize = 53;
/// 半成品 sidecar 的体积上限。
pub const SIDECAR_LIMIT: usize = 64;
pub const FLAG_PAUSED: u8 = 0b0000_0001;
pub const FLAG_VERIFIED: u8 = 0b0000_0010;

/// 接收半成品的续传标记：传输标识、已落盘偏移、总长与状态位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumeSidecar {
    pub transfer: TransferId,
    pub offset: u64,
    pub total: u64,
    pub flags: u8,
}

impl ResumeSidecar {
    pub fn new(transfer: TransferId, offset: u64, total: u64) -> Self {
        Self {
            transfer,
            offset,
            total,
            flags: 0,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.flags & FLAG_PAUSED != 0
    }

    pub fn is_verified(&self) -> bool {
        self.flags & FLAG_VERIFIED != 0
    }

    /// 续传命中条件：内容标识与总长都一致，偏移不越过总长。
    pub fn matches(&self, transfer: &TransferId, total: u64) -> bool {
        self.transfer == *transfer && self.total == total && self.offset <= total
    }

    pub fn encode(&self) -> [u8; SIDECAR_LEN] {
        let mut out = [0u8; SIDECAR_LEN];
        out[0..4].copy_from_slice(&SIDECAR_MAGIC);
        out[4..36].copy_from_slice(&self.transfer);
        out[36..44].copy_from_slice(&self.offset.to_le_bytes());
        out[44..52].copy_from_slice(&self.total.to_le_bytes());
        out[52] = self.flags;
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BtError> {
        if bytes.len() != SIDECAR_LEN {
            return Err(BtError::protocol(format!(
                "续传标记长度不符 {} != {SIDECAR_LEN}",
                bytes.len()
            )));
        }
        if bytes[0..4] != SIDECAR_MAGIC {
            return Err(BtError::protocol("续传标记魔数不匹配"));
        }
        let mut transfer = [0u8; 32];
        transfer.copy_from_slice(&bytes[4..36]);
        let mut offset_bytes = [0u8; 8];
        offset_bytes.copy_from_slice(&bytes[36..44]);
        let mut total_bytes = [0u8; 8];
        total_bytes.copy_from_slice(&bytes[44..52]);
        Ok(Self {
            transfer,
            offset: u64::from_le_bytes(offset_bytes),
            total: u64::from_le_bytes(total_bytes),
            flags: bytes[52],
        })
    }

    pub fn write_to(&self, path: &std::path::Path) -> Result<(), BtError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, self.encode())?;
        Ok(())
    }

    pub fn read_from(path: &std::path::Path) -> Result<Self, BtError> {
        let bytes = std::fs::read(path)?;
        Self::decode(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_fits_budget_and_round_trips() {
        let mut sidecar = ResumeSidecar::new([7u8; 32], 4096, 1 << 20);
        sidecar.flags = FLAG_PAUSED | FLAG_VERIFIED;
        let encoded = sidecar.encode();
        assert_eq!(encoded.len(), SIDECAR_LEN);
        assert!(encoded.len() <= SIDECAR_LIMIT);
        assert_eq!(ResumeSidecar::decode(&encoded).unwrap(), sidecar);
        assert!(sidecar.is_paused());
        assert!(sidecar.is_verified());
    }

    #[test]
    fn rejects_wrong_length_and_magic() {
        let sidecar = ResumeSidecar::new([1u8; 32], 0, 10);
        assert!(ResumeSidecar::decode(&[0u8; 10]).is_err());
        let mut encoded = sidecar.encode();
        encoded[0] = b'X';
        assert!(ResumeSidecar::decode(&encoded).is_err());
    }

    #[test]
    fn match_requires_transfer_and_total() {
        let sidecar = ResumeSidecar::new([2u8; 32], 100, 1000);
        assert!(sidecar.matches(&[2u8; 32], 1000));
        assert!(!sidecar.matches(&[3u8; 32], 1000));
        assert!(!sidecar.matches(&[2u8; 32], 999));
        let mut over = sidecar;
        over.offset = 1001;
        assert!(!over.matches(&[2u8; 32], 1000));
    }
}
