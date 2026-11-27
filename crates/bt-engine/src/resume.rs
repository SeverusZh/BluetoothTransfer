use bt_core::{BtError, TransferId};

pub const SIDECAR_MAGIC: [u8; 4] = *b"BTS1";
pub const SIDECAR_LEN: usize = 45;
/// 半成品 sidecar 的体积上限，验收按此断言。
pub const SIDECAR_LIMIT: usize = 64;
pub const FLAG_PAUSED: u8 = 0b0000_0001;
pub const FLAG_VERIFIED: u8 = 0b0000_0010;

/// 半成品续传标记：传输标识、已落盘偏移、状态位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeSidecar {
    pub transfer: TransferId,
    pub offset: u64,
    pub flags: u8,
}

impl ResumeSidecar {
    pub fn new(transfer: TransferId, offset: u64) -> Self {
        Self {
            transfer,
            offset,
            flags: 0,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.flags & FLAG_PAUSED != 0
    }

    pub fn is_verified(&self) -> bool {
        self.flags & FLAG_VERIFIED != 0
    }

    pub fn encode(&self) -> [u8; SIDECAR_LEN] {
        let mut out = [0u8; SIDECAR_LEN];
        out[0..4].copy_from_slice(&SIDECAR_MAGIC);
        out[4..36].copy_from_slice(&self.transfer);
        out[36..44].copy_from_slice(&self.offset.to_be_bytes());
        out[44] = self.flags;
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
        let offset = u64::from_be_bytes(bytes[36..44].try_into().expect("长度已校验"));
        Ok(Self {
            transfer,
            offset,
            flags: bytes[44],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_fits_budget_and_round_trips() {
        let mut sidecar = ResumeSidecar::new([7u8; 32], 4096);
        sidecar.flags = FLAG_PAUSED;
        let encoded = sidecar.encode();
        assert_eq!(encoded.len(), SIDECAR_LEN);
        assert!(encoded.len() <= SIDECAR_LIMIT);
        assert_eq!(ResumeSidecar::decode(&encoded).unwrap(), sidecar);
    }

    #[test]
    fn rejects_wrong_length_and_magic() {
        let sidecar = ResumeSidecar::new([1u8; 32], 0);
        assert!(ResumeSidecar::decode(&[0u8; 10]).is_err());
        let mut encoded = sidecar.encode();
        encoded[0] = b'X';
        assert!(ResumeSidecar::decode(&encoded).is_err());
    }
}
