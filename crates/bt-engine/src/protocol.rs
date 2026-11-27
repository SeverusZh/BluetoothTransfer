use bt_core::BtError;

pub const MAGIC: u8 = 0x41;
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 7;
pub const MAX_FRAME_SIZE: usize = 1024 * 1024;
pub const MAX_PAYLOAD: usize = MAX_FRAME_SIZE - HEADER_LEN;
/// DATA 载荷内的偏移与长度字段开销。
pub const DATA_OVERHEAD: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Hello = 1,
    Offer = 2,
    Data = 3,
    Ack = 4,
    Done = 5,
    Cancel = 6,
    Error = 7,
}

impl FrameKind {
    pub const fn as_byte(self) -> u8 {
        self as u8
    }

    pub fn from_byte(byte: u8) -> Result<Self, BtError> {
        match byte {
            1 => Ok(Self::Hello),
            2 => Ok(Self::Offer),
            3 => Ok(Self::Data),
            4 => Ok(Self::Ack),
            5 => Ok(Self::Done),
            6 => Ok(Self::Cancel),
            7 => Ok(Self::Error),
            other => Err(BtError::protocol(format!("未知帧类型 {other}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    kind: FrameKind,
    payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: FrameKind, payload: Vec<u8>) -> Result<Self, BtError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(BtError::protocol(format!(
                "载荷超出上限 {} > {MAX_PAYLOAD}",
                payload.len()
            )));
        }
        Ok(Self { kind, payload })
    }

    pub const fn kind(&self) -> FrameKind {
        self.kind
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn total_len(&self) -> usize {
        HEADER_LEN + self.payload.len()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.total_len());
        out.push(MAGIC);
        out.push(VERSION);
        out.push(self.kind.as_byte());
        out.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BtError> {
        if bytes.len() < HEADER_LEN {
            return Err(BtError::protocol(format!("帧头不足 {} 字节", bytes.len())));
        }
        if bytes[0] != MAGIC {
            return Err(BtError::protocol(format!("帧魔数不匹配 {}", bytes[0])));
        }
        if bytes[1] != VERSION {
            return Err(BtError::protocol(format!("协议版本不支持 {}", bytes[1])));
        }
        let kind = FrameKind::from_byte(bytes[2])?;
        let declared = u32::from_be_bytes([bytes[3], bytes[4], bytes[5], bytes[6]]) as usize;
        if declared > MAX_PAYLOAD {
            return Err(BtError::protocol(format!("声明长度超出上限 {declared}")));
        }
        let total = HEADER_LEN + declared;
        if bytes.len() < total {
            return Err(BtError::protocol(format!("载荷不完整 {} < {total}", bytes.len())));
        }
        Self::new(kind, bytes[HEADER_LEN..total].to_vec())
    }
}

/// DATA 帧载荷视图：64 位偏移加长度加数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataSlice<'a> {
    pub offset: u64,
    pub bytes: &'a [u8],
}

pub fn encode_data(offset: u64, bytes: &[u8]) -> Result<Vec<u8>, BtError> {
    if bytes.len() + DATA_OVERHEAD > MAX_PAYLOAD {
        return Err(BtError::protocol("DATA 分片超出单帧上限"));
    }
    let mut payload = Vec::with_capacity(DATA_OVERHEAD + bytes.len());
    payload.extend_from_slice(&offset.to_be_bytes());
    payload.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    payload.extend_from_slice(bytes);
    Ok(payload)
}

pub fn decode_data(payload: &[u8]) -> Result<DataSlice<'_>, BtError> {
    if payload.len() < DATA_OVERHEAD {
        return Err(BtError::protocol("DATA 载荷不足"));
    }
    let offset = u64::from_be_bytes(payload[0..8].try_into().expect("长度已校验"));
    let declared = u32::from_be_bytes(payload[8..12].try_into().expect("长度已校验")) as usize;
    let available = payload.len() - DATA_OVERHEAD;
    if declared != available {
        return Err(BtError::protocol(format!(
            "DATA 长度字段与实际不符 {declared} != {available}"
        )));
    }
    Ok(DataSlice {
        offset,
        bytes: &payload[DATA_OVERHEAD..],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trips() {
        let frame = Frame::new(FrameKind::Offer, b"hello".to_vec()).unwrap();
        let encoded = frame.encode();
        assert_eq!(encoded.len(), HEADER_LEN + 5);
        assert_eq!(Frame::decode(&encoded).unwrap(), frame);
    }

    #[test]
    fn rejects_bad_header_fields() {
        let frame = Frame::new(FrameKind::Ack, Vec::new()).unwrap();
        let mut encoded = frame.encode();
        encoded[0] = 0x00;
        assert!(Frame::decode(&encoded).is_err());

        let mut encoded = frame.encode();
        encoded[2] = 0x7f;
        assert!(Frame::decode(&encoded).is_err());

        let truncated = &frame.encode()[..3];
        assert!(Frame::decode(truncated).is_err());
    }

    #[test]
    fn payload_over_limit_is_rejected() {
        let payload = vec![0u8; MAX_PAYLOAD + 1];
        assert!(Frame::new(FrameKind::Data, payload).is_err());
    }

    #[test]
    fn data_slice_round_trips() {
        let payload = encode_data(1024, b"chunk").unwrap();
        let decoded = decode_data(&payload).unwrap();
        assert_eq!(decoded.offset, 1024);
        assert_eq!(decoded.bytes, b"chunk");
    }

    #[test]
    fn data_slice_rejects_mismatched_length() {
        let mut payload = encode_data(0, b"chunk").unwrap();
        payload[11] = 9;
        assert!(decode_data(&payload).is_err());
    }
}
