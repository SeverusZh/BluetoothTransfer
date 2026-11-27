use std::io::{Read, Write};
use std::time::{Duration, Instant};

use bt_core::{parse_transfer_id, BtError, BtErrorKind, TransferId};

/// 帧魔数 'A'，与旧版 .NET 接收端一致。
pub const MAGIC: u8 = 0x41;
pub const VERSION: u8 = 1;
/// 长度字段 4 字节、魔数 1 字节、版本 1 字节、类型 1 字节。
pub const HEADER_LEN: usize = 7;
pub const MAX_FRAME_SIZE: usize = 1024 * 1024;
pub const MAX_PAYLOAD: usize = MAX_FRAME_SIZE - HEADER_LEN;
/// DATA 载荷内的偏移与长度字段。
pub const DATA_OVERHEAD: usize = 12;
/// 单帧上限扣除帧头与 DATA 字段后的最大分片。
pub const MAX_CHUNK_SIZE: u32 = (MAX_FRAME_SIZE - HEADER_LEN - DATA_OVERHEAD) as u32;

/// 助手协议消息类型。
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

/// 一帧：7 字节帧头加明文载荷，总长前缀为小端 u32。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    kind: FrameKind,
    payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: FrameKind, payload: Vec<u8>) -> Result<Self, BtError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(BtError::protocol(format!(
                "帧载荷超出上限 {} > {MAX_PAYLOAD}",
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
        let total = self.total_len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.push(MAGIC);
        out.push(VERSION);
        out.push(self.kind.as_byte());
        out.extend_from_slice(&self.payload);
        out
    }

    /// 解析完整帧；长度字段必须与实际长度一致。
    pub fn decode(bytes: &[u8]) -> Result<Self, BtError> {
        let header = parse_header(bytes)?;
        let (kind, total) = header;
        if bytes.len() != total {
            return Err(BtError::protocol(format!(
                "帧长度字段与实际不符 {total} != {}",
                bytes.len()
            )));
        }
        Self::new(kind, bytes[HEADER_LEN..total].to_vec())
    }

    /// 从流中读取一帧，先读帧头再读载荷。
    pub fn read_from<R: Read + ?Sized>(reader: &mut R) -> Result<Self, BtError> {
        let mut header = [0u8; HEADER_LEN];
        reader
            .read_exact(&mut header)
            .map_err(|error| io_error("读取帧头失败", error))?;
        let (kind, total) = parse_header(&header)?;
        let mut payload = vec![0u8; total - HEADER_LEN];
        reader
            .read_exact(&mut payload)
            .map_err(|error| io_error("读取帧载荷失败", error))?;
        Self::new(kind, payload)
    }

    pub fn write_to<W: Write + ?Sized>(&self, writer: &mut W) -> Result<(), BtError> {
        writer
            .write_all(&self.encode())
            .map_err(|error| io_error("写入帧失败", error))
    }
}

fn parse_header(bytes: &[u8]) -> Result<(FrameKind, usize), BtError> {
    if bytes.len() < HEADER_LEN {
        return Err(BtError::protocol(format!("帧头不足 {} 字节", bytes.len())));
    }
    let declared = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if !(HEADER_LEN..=MAX_FRAME_SIZE).contains(&declared) {
        return Err(BtError::protocol(format!("帧长度非法 {declared}")));
    }
    if bytes[4] != MAGIC {
        return Err(BtError::protocol(format!("帧魔数不匹配 0x{:02X}", bytes[4])));
    }
    if bytes[5] != VERSION {
        return Err(BtError::protocol(format!("协议版本不支持 {}", bytes[5])));
    }
    Ok((FrameKind::from_byte(bytes[6])?, declared))
}

fn io_error(context: &str, error: std::io::Error) -> BtError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            BtError::timeout(format!("{context}：{error}"))
        }
        _ => BtError::new(bt_core::BtErrorKind::Io, format!("{context}：{error}")),
    }
}

fn write_string(out: &mut Vec<u8>, value: &str) -> Result<(), BtError> {
    let bytes = value.as_bytes();
    if bytes.len() > u32::MAX as usize {
        return Err(BtError::protocol("字符串超出 4 字节长度上限"));
    }
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn read_string(data: &[u8], pos: &mut usize) -> Result<String, BtError> {
    if data.len().saturating_sub(*pos) < 4 {
        return Err(BtError::protocol("字符串长度字段截断"));
    }
    let length = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]) as usize;
    *pos += 4;
    let available = data.len() - *pos;
    if length > available {
        return Err(BtError::protocol(format!("字符串长度非法 {length} > 剩余 {available}")));
    }
    let value = std::str::from_utf8(&data[*pos..*pos + length])
        .map_err(|_| BtError::protocol("字符串不是合法 UTF-8"))?
        .to_string();
    *pos += length;
    Ok(value)
}

fn take_i64(data: &[u8], pos: &mut usize) -> Result<i64, BtError> {
    if data.len().saturating_sub(*pos) < 8 {
        return Err(BtError::protocol("64 位字段截断"));
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&data[*pos..*pos + 8]);
    *pos += 8;
    Ok(i64::from_le_bytes(bytes))
}

fn take_i32(data: &[u8], pos: &mut usize) -> Result<i32, BtError> {
    if data.len().saturating_sub(*pos) < 4 {
        return Err(BtError::protocol("32 位字段截断"));
    }
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&data[*pos..*pos + 4]);
    *pos += 4;
    Ok(i32::from_le_bytes(bytes))
}

/// 在短读超时流上轮询读满一帧：分片边界检查停止标记，超时空闲判定用整体期限。
pub fn read_frame_poll<R: Read + ?Sized>(
    reader: &mut R,
    mut should_stop: impl FnMut() -> bool,
    idle: Duration,
) -> Result<Frame, BtError> {
    let deadline = Instant::now() + idle;
    let mut header = [0u8; HEADER_LEN];
    read_exact_poll(reader, &mut header, &mut should_stop, deadline)?;
    let (kind, total) = parse_header(&header)?;
    let mut payload = vec![0u8; total - HEADER_LEN];
    read_exact_poll(reader, &mut payload, &mut should_stop, deadline)?;
    Frame::new(kind, payload)
}

fn read_exact_poll<R: Read + ?Sized>(
    reader: &mut R,
    buffer: &mut [u8],
    should_stop: &mut impl FnMut() -> bool,
    deadline: Instant,
) -> Result<(), BtError> {
    let mut position = 0usize;
    while position < buffer.len() {
        if should_stop() {
            return Err(BtError::canceled("传输已停止"));
        }
        match reader.read(&mut buffer[position..]) {
            Ok(0) => return Err(BtError::new(BtErrorKind::Io, "对端提前关闭连接")),
            Ok(read) => position += read,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                if Instant::now() >= deadline {
                    return Err(BtError::timeout("等待对端数据超时"));
                }
            }
            Err(error) => return Err(io_error("读取帧失败", error)),
        }
    }
    Ok(())
}

/// HELLO：传输标识、落盘文件名、总长、分片大小、期望 SHA-256。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub transfer_id: String,
    pub file_name: String,
    pub file_size: i64,
    pub chunk_size: i32,
    pub expected_sha256: String,
}

impl Hello {
    pub fn new(
        file_name: impl Into<String>,
        file_size: i64,
        expected_sha256: impl Into<String>,
        chunk_size: u32,
    ) -> Self {
        let file_name = file_name.into();
        let expected_sha256 = expected_sha256.into();
        Self {
            transfer_id: expected_sha256.clone(),
            file_name,
            file_size,
            chunk_size: chunk_size as i32,
            expected_sha256,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, BtError> {
        let mut out = Vec::new();
        write_string(&mut out, &self.transfer_id)?;
        write_string(&mut out, &self.file_name)?;
        out.extend_from_slice(&self.file_size.to_le_bytes());
        out.extend_from_slice(&self.chunk_size.to_le_bytes());
        write_string(&mut out, &self.expected_sha256)?;
        Ok(out)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, BtError> {
        let mut pos = 0usize;
        let transfer_id = read_string(payload, &mut pos)?;
        let file_name = read_string(payload, &mut pos)?;
        let file_size = take_i64(payload, &mut pos)?;
        let chunk_size = take_i32(payload, &mut pos)?;
        let expected_sha256 = read_string(payload, &mut pos)?;
        if file_size < 0 {
            return Err(BtError::protocol(format!("文件大小为负 {file_size}")));
        }
        if chunk_size <= 0 || chunk_size as u32 > MAX_CHUNK_SIZE {
            return Err(BtError::protocol(format!("分片大小非法 {chunk_size}")));
        }
        Ok(Self {
            transfer_id,
            file_name,
            file_size,
            chunk_size,
            expected_sha256,
        })
    }

    /// HELLO 合法性：大小非负、SHA-256 为 64 位十六进制。
    pub fn validate(&self) -> Result<(), BtError> {
        if self.file_size < 0 {
            return Err(BtError::protocol("文件大小为负"));
        }
        if parse_transfer_id(&self.expected_sha256).is_err() {
            return Err(BtError::protocol("期望 SHA-256 缺失或格式非法"));
        }
        Ok(())
    }

    pub fn expected_digest(&self) -> Result<TransferId, BtError> {
        parse_transfer_id(&self.expected_sha256)
    }
}

/// OFFER 状态：0 接受、1 忙、2 拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OfferStatus {
    Accept = 0,
    Busy = 1,
    Reject = 2,
}

impl OfferStatus {
    pub fn from_byte(byte: u8) -> Result<Self, BtError> {
        match byte {
            0 => Ok(Self::Accept),
            1 => Ok(Self::Busy),
            2 => Ok(Self::Reject),
            other => Err(BtError::protocol(format!("未知 OFFER 状态 {other}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub status: OfferStatus,
    pub resume_offset: i64,
    pub reason: String,
}

impl Offer {
    pub fn new(status: OfferStatus, resume_offset: i64, reason: impl Into<String>) -> Self {
        Self {
            status,
            resume_offset,
            reason: reason.into(),
        }
    }

    pub fn accept(resume_offset: i64) -> Self {
        Self::new(OfferStatus::Accept, resume_offset, "")
    }

    pub fn busy() -> Self {
        Self::new(OfferStatus::Busy, 0, "接收端忙")
    }

    pub fn reject(reason: impl Into<String>) -> Self {
        Self::new(OfferStatus::Reject, 0, reason)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(9 + self.reason.len() + 4);
        out.push(self.status as u8);
        out.extend_from_slice(&self.resume_offset.to_le_bytes());
        let _ = write_string(&mut out, &self.reason);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self, BtError> {
        if payload.len() < 9 {
            return Err(BtError::protocol("OFFER 载荷截断"));
        }
        let status = OfferStatus::from_byte(payload[0])?;
        let mut offset_bytes = [0u8; 8];
        offset_bytes.copy_from_slice(&payload[1..9]);
        let resume_offset = i64::from_le_bytes(offset_bytes);
        let mut pos = 9usize;
        let reason = read_string(payload, &mut pos)?;
        if resume_offset < 0 {
            return Err(BtError::protocol(format!("非法续传偏移 {resume_offset}")));
        }
        Ok(Self {
            status,
            resume_offset,
            reason,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataSlice<'a> {
    pub offset: i64,
    pub bytes: &'a [u8],
}

pub fn encode_data(offset: i64, bytes: &[u8]) -> Result<Vec<u8>, BtError> {
    if offset < 0 {
        return Err(BtError::protocol("数据偏移为负"));
    }
    if bytes.len() + DATA_OVERHEAD > MAX_PAYLOAD {
        return Err(BtError::protocol("DATA 分片超出单帧上限"));
    }
    let mut payload = Vec::with_capacity(DATA_OVERHEAD + bytes.len());
    payload.extend_from_slice(&offset.to_le_bytes());
    payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    payload.extend_from_slice(bytes);
    Ok(payload)
}

pub fn decode_data(payload: &[u8]) -> Result<DataSlice<'_>, BtError> {
    if payload.len() < DATA_OVERHEAD {
        return Err(BtError::protocol("DATA 载荷截断"));
    }
    let mut offset_bytes = [0u8; 8];
    offset_bytes.copy_from_slice(&payload[0..8]);
    let offset = i64::from_le_bytes(offset_bytes);
    let declared = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]) as usize;
    let available = payload.len() - DATA_OVERHEAD;
    if declared != available {
        return Err(BtError::protocol(format!(
            "DATA 长度字段与实际不符 {declared} != {available}"
        )));
    }
    if offset < 0 {
        return Err(BtError::protocol("数据偏移为负"));
    }
    Ok(DataSlice {
        offset,
        bytes: &payload[DATA_OVERHEAD..],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ack {
    pub offset: i64,
}

impl Ack {
    pub fn new(offset: i64) -> Self {
        Self { offset }
    }

    pub fn encode(&self) -> [u8; 8] {
        self.offset.to_le_bytes()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, BtError> {
        if payload.len() < 8 {
            return Err(BtError::protocol("ACK 载荷截断"));
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&payload[0..8]);
        Ok(Self {
            offset: i64::from_le_bytes(bytes),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub ok: bool,
    pub hash: String,
}

impl Done {
    pub fn new(ok: bool, hash: impl Into<String>) -> Self {
        Self { ok, hash: hash.into() }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 4 + self.hash.len());
        out.push(if self.ok { 1 } else { 0 });
        let _ = write_string(&mut out, &self.hash);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self, BtError> {
        if payload.is_empty() {
            return Err(BtError::protocol("DONE 载荷截断"));
        }
        let ok = payload[0] == 1;
        let mut pos = 1usize;
        let hash = read_string(payload, &mut pos)?;
        Ok(Self { ok, hash })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancel {
    pub reason: String,
}

impl Cancel {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let _ = write_string(&mut out, &self.reason);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self, BtError> {
        let mut pos = 0usize;
        Ok(Self {
            reason: read_string(payload, &mut pos)?,
        })
    }
}

/// 错误码与旧版一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ErrorCode {
    Busy = 1,
    DiskFull = 2,
    Mismatch = 3,
    HashMismatch = 4,
    IoError = 5,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorMessage {
    pub code: i32,
    pub message: String,
}

impl ErrorMessage {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 4 + self.message.len());
        out.extend_from_slice(&self.code.to_le_bytes());
        let _ = write_string(&mut out, &self.message);
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Self, BtError> {
        if payload.len() < 4 {
            return Err(BtError::protocol("ERROR 载荷截断"));
        }
        let code = i32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let mut pos = 4usize;
        let message = read_string(payload, &mut pos)?;
        Ok(Self { code, message })
    }
}

pub fn build(kind: FrameKind, payload: impl Into<Vec<u8>>) -> Result<Frame, BtError> {
    Frame::new(kind, payload.into())
}

pub fn encode_hello(hello: &Hello) -> Result<Frame, BtError> {
    Frame::new(FrameKind::Hello, hello.encode()?)
}

pub fn encode_offer(offer: &Offer) -> Result<Frame, BtError> {
    Frame::new(FrameKind::Offer, offer.encode())
}

pub fn encode_data_frame(offset: i64, bytes: &[u8]) -> Result<Frame, BtError> {
    Frame::new(FrameKind::Data, encode_data(offset, bytes)?)
}

pub fn encode_ack(offset: i64) -> Result<Frame, BtError> {
    Frame::new(FrameKind::Ack, Ack::new(offset).encode().to_vec())
}

pub fn encode_done(done: &Done) -> Result<Frame, BtError> {
    Frame::new(FrameKind::Done, done.encode())
}

pub fn encode_cancel(reason: &str) -> Result<Frame, BtError> {
    Frame::new(
        FrameKind::Cancel,
        Cancel {
            reason: reason.to_string(),
        }
        .encode(),
    )
}

pub fn encode_error(code: i32, message: &str) -> Result<Frame, BtError> {
    Frame::new(FrameKind::Error, ErrorMessage::new(code, message).encode())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::{hex_lower, sha256};

    #[test]
    fn frame_matches_reference_bytes() {
        let frame = Frame::new(FrameKind::Hello, vec![1, 2, 3]).unwrap();
        let encoded = frame.encode();
        assert_eq!(encoded.len(), 10);
        assert_eq!(&encoded[..4], &[10, 0, 0, 0]);
        assert_eq!(encoded[4], 0x41);
        assert_eq!(encoded[5], 1);
        assert_eq!(encoded[6], 1);
        assert_eq!(&encoded[7..], &[1, 2, 3]);
        assert_eq!(Frame::decode(&encoded).unwrap(), frame);
    }

    #[test]
    fn hello_matches_reference_byte_layout() {
        let hello = Hello {
            transfer_id: "t1".into(),
            file_name: "a.bin".into(),
            file_size: 5,
            chunk_size: 4096,
            expected_sha256: "ab".repeat(32),
        };
        let payload = hello.encode().unwrap();
        // transferId: 2 + "t1"; fileName: 2 + "a.bin"; size i64; chunk i32; sha 64 字节字符串
        assert_eq!(&payload[..4], &[2, 0, 0, 0]);
        assert_eq!(&payload[4..6], b"t1");
        assert_eq!(&payload[6..10], &[5, 0, 0, 0]);
        assert_eq!(&payload[10..15], b"a.bin");
        assert_eq!(&payload[15..23], &5i64.to_le_bytes());
        assert_eq!(&payload[23..27], &4096i32.to_le_bytes());
        assert_eq!(payload.len(), 27 + 4 + 64);
        assert_eq!(Hello::decode(&payload).unwrap(), hello);
    }

    #[test]
    fn hello_rejects_negative_size_and_bad_chunk() {
        let mut hello = Hello::new("a.bin", 5, "ab".repeat(32), 4096);
        hello.file_size = -1;
        assert!(Hello::decode(&hello.encode().unwrap()).is_err());
        let mut hello = Hello::new("a.bin", 5, "ab".repeat(32), 4096);
        hello.chunk_size = 0;
        assert!(Hello::decode(&hello.encode().unwrap()).is_err());
        let mut hello = Hello::new("a.bin", 5, "ab".repeat(32), 4096);
        hello.chunk_size = MAX_CHUNK_SIZE as i32 + 1;
        assert!(Hello::decode(&hello.encode().unwrap()).is_err());
    }

    #[test]
    fn hello_validate_requires_hex_sha() {
        assert!(Hello::new("a.bin", 1, "ab".repeat(32), 4096).validate().is_ok());
        assert!(Hello::new("a.bin", 1, "", 4096).validate().is_err());
        assert!(Hello::new("a.bin", 1, "zz".repeat(32), 4096).validate().is_err());
        assert!(Hello::new("a.bin", 1, "ab".repeat(31), 4096).validate().is_err());
    }

    #[test]
    fn offer_round_trip_all_statuses() {
        for offer in [
            Offer::accept(0),
            Offer::accept(4096),
            Offer::busy(),
            Offer::reject("用户拒绝"),
        ] {
            assert_eq!(Offer::decode(&offer.encode()).unwrap(), offer);
        }
        assert_eq!(Offer::decode(&Offer::accept(7).encode()).unwrap().resume_offset, 7);
        let mut bad = Offer::accept(1).encode();
        bad[0] = 9;
        assert!(Offer::decode(&bad).is_err());
    }

    #[test]
    fn data_slice_round_trips() {
        let payload = encode_data(1024, b"chunk").unwrap();
        let decoded = decode_data(&payload).unwrap();
        assert_eq!(decoded.offset, 1024);
        assert_eq!(decoded.bytes, b"chunk");
        let mut tampered = payload.clone();
        tampered[11] = 9;
        assert!(decode_data(&tampered).is_err());
    }

    #[test]
    fn rejected_inputs_do_not_panic() {
        assert!(Frame::decode(&[]).is_err());
        assert!(Frame::decode(&[7, 0, 0, 0, 0x41, 1, 1]).is_ok());
        let mut frame = Frame::new(FrameKind::Ack, Vec::new()).unwrap().encode();
        frame[0] = 0xff;
        assert!(Frame::decode(&frame).is_err());
        let mut frame = frame.clone();
        frame[0] = 7;
        frame[4] = 0;
        assert!(Frame::decode(&frame).is_err());
        let mut frame = Frame::new(FrameKind::Ack, Vec::new()).unwrap().encode();
        frame[2] = 2;
        assert!(Frame::decode(&frame).is_err());

        let payload = [0u8; 12];
        assert!(decode_data(&payload).is_ok());
        assert!(decode_data(&[0u8; 11]).is_err());
    }

    #[test]
    fn string_length_int32_max_is_rejected() {
        let mut payload = vec![0u8; 13];
        payload[1..9].copy_from_slice(&0i64.to_le_bytes());
        payload[9..13].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(Offer::decode(&payload).is_err());

        let mut payload = vec![0u8; 12];
        payload[8..12].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(decode_data(&payload).is_err());
    }

    #[test]
    fn done_cancel_error_round_trip() {
        assert_eq!(
            Done::decode(&Done::new(true, "HASH").encode()).unwrap(),
            Done::new(true, "HASH")
        );
        assert_eq!(
            Done::decode(&Done::new(false, "").encode()).unwrap(),
            Done::new(false, "")
        );
        let cancel = Cancel {
            reason: "暂停".into()
        };
        assert_eq!(Cancel::decode(&cancel.encode()).unwrap(), cancel);
        let error = ErrorMessage::new(ErrorCode::HashMismatch as i32, "校验失败");
        assert_eq!(ErrorMessage::decode(&error.encode()).unwrap(), error);
        assert!(Done::decode(&[]).is_err());
        assert!(ErrorMessage::decode(&[1, 0, 0]).is_err());
    }

    #[test]
    fn payload_over_limit_is_rejected() {
        assert!(Frame::new(FrameKind::Data, vec![0u8; MAX_PAYLOAD + 1]).is_err());
        assert!(Frame::new(FrameKind::Data, vec![0u8; MAX_PAYLOAD]).is_ok());
        assert!(encode_data(0, &vec![0u8; MAX_PAYLOAD]).is_err());
    }

    #[test]
    fn sha256_hex_matches_core_helper() {
        let digest = sha256(b"blue");
        let hello = Hello::new("x", 4, hex_lower(&digest), 4096);
        assert_eq!(hello.expected_digest().unwrap(), digest);
    }
}
