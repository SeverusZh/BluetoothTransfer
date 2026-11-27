use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use bt_core::{BtError, BtErrorKind};
use bt_transport::TransportStream;

use crate::cancel::CancelFlag;
use crate::protocol::{
    encode_cancel, encode_data_frame, encode_hello, read_frame_poll, Ack, Done, Frame, FrameKind, Hello, Offer,
    OfferStatus, MAX_CHUNK_SIZE,
};

/// 传输阶段轮询粒度，决定取消与停止的响应延迟。
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome {
    pub ok: bool,
    pub error: Option<String>,
    pub bytes_sent: u64,
    pub total_bytes: u64,
    pub resume_offset: u64,
    pub canceled: bool,
    pub retryable: bool,
}

impl SendOutcome {
    fn busy() -> Self {
        Self {
            ok: false,
            error: Some("接收端忙，稍后自动重试".to_string()),
            bytes_sent: 0,
            total_bytes: 0,
            resume_offset: 0,
            canceled: false,
            retryable: true,
        }
    }
}

/// 读取源文件中指定偏移的分片。
pub trait ChunkSource {
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<usize, BtError>;
}

impl ChunkSource for File {
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<usize, BtError> {
        self.seek(SeekFrom::Start(offset))?;
        Ok(self.read(buffer)?)
    }
}

impl ChunkSource for &[u8] {
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<usize, BtError> {
        let start = offset as usize;
        if start >= self.len() {
            return Ok(0);
        }
        let count = buffer.len().min(self.len() - start);
        buffer[..count].copy_from_slice(&self[start..start + count]);
        Ok(count)
    }
}

/// 源文件分片读取器包装，便于在测试中注入字节切片。
pub struct ReaderSource<'a>(pub &'a mut (dyn ChunkSource + 'a));

/// 助手通道客户端：HELLO → OFFER（含续传偏移）→ DATA/ACK → DONE。
pub fn send_source(
    stream: &mut dyn TransportStream,
    hello: &Hello,
    source: &mut dyn ChunkSource,
    cancel: &CancelFlag,
    idle_timeout: Duration,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<SendOutcome, BtError> {
    hello.validate()?;
    if hello.chunk_size <= 0 || hello.chunk_size as u32 > MAX_CHUNK_SIZE {
        return Err(BtError::invalid(format!("非法分片大小 {}", hello.chunk_size)));
    }
    if hello.file_size < 0 {
        return Err(BtError::invalid("文件大小为负"));
    }
    let total = hello.file_size as u64;
    let chunk = hello.chunk_size as usize;

    stream.set_timeouts(Some(POLL_INTERVAL), Some(idle_timeout))?;
    encode_hello(hello)?.write_to(stream)?;

    let offer = match read_offer(stream, cancel, idle_timeout) {
        Ok(offer) => offer,
        Err(error) if error.kind() == BtErrorKind::Canceled => {
            send_cancel_best_effort(stream, "用户取消");
            return Ok(canceled_outcome(0, total, 0));
        }
        Err(error) => return Err(error),
    };
    match offer.status {
        OfferStatus::Busy => {
            let mut outcome = SendOutcome::busy();
            outcome.total_bytes = total;
            return Ok(outcome);
        }
        OfferStatus::Reject => {
            return Ok(SendOutcome {
                ok: false,
                error: Some(if offer.reason.is_empty() {
                    "接收端拒绝".to_string()
                } else {
                    offer.reason
                }),
                bytes_sent: 0,
                total_bytes: total,
                resume_offset: 0,
                canceled: false,
                retryable: false,
            });
        }
        OfferStatus::Accept => {}
    }
    if offer.resume_offset < 0 || offer.resume_offset as u64 > total {
        return Err(BtError::protocol(format!("非法续传偏移 {}", offer.resume_offset)));
    }

    let resume_offset = offer.resume_offset as u64;
    let mut current = resume_offset;
    progress(current, total);
    let mut buffer = vec![0u8; chunk];

    while current < total {
        if cancel.is_canceled() {
            send_cancel_best_effort(stream, "用户取消");
            return Ok(canceled_outcome(current, total, resume_offset));
        }
        let read = source.read_at(current, &mut buffer)?;
        if read == 0 {
            return Err(BtError::protocol("源文件读取提前结束"));
        }
        let end = current + read as u64;
        if end > total {
            return Err(BtError::protocol("读取越过文件末尾"));
        }
        encode_data_frame(current as i64, &buffer[..read])?.write_to(stream)?;
        let frame = match read_frame_poll(stream, || cancel.is_canceled(), idle_timeout) {
            Ok(frame) => frame,
            Err(error) if error.kind() == BtErrorKind::Canceled => {
                send_cancel_best_effort(stream, "用户取消");
                return Ok(canceled_outcome(current, total, resume_offset));
            }
            Err(error) => return Err(error),
        };
        if frame.kind() != FrameKind::Ack {
            return Err(BtError::protocol(format!(
                "期望 ACK，收到消息类型 {}",
                frame.kind().as_byte()
            )));
        }
        let ack = Ack::decode(frame.payload())?;
        if ack.offset < current as i64 || ack.offset > total as i64 {
            return Err(BtError::protocol(format!(
                "非法 ACK 偏移 {}（当前发送偏移 {current}，文件大小 {total}）",
                ack.offset
            )));
        }
        current = ack.offset as u64;
        progress(current, total);
    }

    let frame = match read_frame_poll(stream, || cancel.is_canceled(), idle_timeout) {
        Ok(frame) => frame,
        Err(error) if error.kind() == BtErrorKind::Canceled => {
            send_cancel_best_effort(stream, "用户取消");
            return Ok(canceled_outcome(current, total, resume_offset));
        }
        Err(error) => return Err(error),
    };
    if frame.kind() != FrameKind::Done {
        return Err(BtError::protocol(format!(
            "期望 DONE，收到消息类型 {}",
            frame.kind().as_byte()
        )));
    }
    let done = Done::decode(frame.payload())?;
    if done.ok {
        Ok(SendOutcome {
            ok: true,
            error: None,
            bytes_sent: current,
            total_bytes: total,
            resume_offset,
            canceled: false,
            retryable: false,
        })
    } else {
        Ok(SendOutcome {
            ok: false,
            error: Some("接收端 SHA-256 校验失败（已清空半成品，重试将从头传输）".to_string()),
            bytes_sent: current,
            total_bytes: total,
            resume_offset,
            canceled: false,
            retryable: true,
        })
    }
}

/// 从文件发送，文件大小必须与 HELLO 一致。
pub fn send_file(
    stream: &mut dyn TransportStream,
    hello: &Hello,
    path: &Path,
    cancel: &CancelFlag,
    idle_timeout: Duration,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<SendOutcome, BtError> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    if size != hello.file_size as u64 {
        return Err(BtError::invalid(format!(
            "源文件大小已变化 {} != {}",
            hello.file_size, size
        )));
    }
    send_source(stream, hello, &mut file, cancel, idle_timeout, progress)
}

fn read_offer(stream: &mut dyn TransportStream, cancel: &CancelFlag, idle: Duration) -> Result<Offer, BtError> {
    let frame: Frame = read_frame_poll(stream, || cancel.is_canceled(), idle)?;
    if frame.kind() != FrameKind::Offer {
        return Err(BtError::protocol(format!(
            "期望 OFFER，收到消息类型 {}",
            frame.kind().as_byte()
        )));
    }
    Offer::decode(frame.payload())
}

fn send_cancel_best_effort(stream: &mut dyn TransportStream, reason: &str) {
    if let Ok(frame) = encode_cancel(reason) {
        let _ = frame.write_to(stream);
    }
}

fn canceled_outcome(bytes_sent: u64, total: u64, resume_offset: u64) -> SendOutcome {
    SendOutcome {
        ok: false,
        error: Some("用户取消".to_string()),
        bytes_sent,
        total_bytes: total,
        resume_offset,
        canceled: true,
        retryable: true,
    }
}

/// 校验接收端返回的 ACK 是否落在允许区间。
pub fn validate_ack(current: u64, ack: &Ack, total: u64) -> Result<u64, BtError> {
    if ack.offset < current as i64 || ack.offset > total as i64 {
        return Err(BtError::protocol("非法 ACK 偏移"));
    }
    Ok(ack.offset as u64)
}

/// 便于测试的字节源发送入口。
pub fn send_bytes(
    stream: &mut dyn TransportStream,
    hello: &Hello,
    bytes: &[u8],
    cancel: &CancelFlag,
    idle_timeout: Duration,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<SendOutcome, BtError> {
    let mut source = bytes;
    send_source(stream, hello, &mut source, cancel, idle_timeout, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_source_slice_stays_in_bounds() {
        let data = [1u8, 2, 3];
        let mut source: &[u8] = &data;
        let mut buffer = [0u8; 2];
        assert_eq!(source.read_at(0, &mut buffer).unwrap(), 2);
        assert_eq!(buffer, [1, 2]);
        assert_eq!(source.read_at(1, &mut buffer).unwrap(), 2);
        assert_eq!(buffer, [2, 3]);
        assert_eq!(source.read_at(3, &mut buffer).unwrap(), 0);
    }

    #[test]
    fn ack_validation_rejects_regression_and_overflow() {
        let ack = Ack::new(5);
        assert_eq!(validate_ack(5, &ack, 10).unwrap(), 5);
        assert!(validate_ack(6, &ack, 10).is_err());
        assert!(validate_ack(0, &Ack::new(11), 10).is_err());
        assert!(validate_ack(0, &Ack::new(-1), 10).is_err());
    }
}
