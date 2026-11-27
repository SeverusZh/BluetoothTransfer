use std::path::PathBuf;
use std::time::Duration;

use bt_core::{BtError, BtErrorKind};
use bt_transport::TransportStream;

use crate::client::POLL_INTERVAL;
use crate::protocol::{
    encode_ack, encode_done, encode_error, encode_offer, read_frame_poll, Cancel, Done, ErrorCode, ErrorMessage, Frame,
    FrameKind, Hello, Offer,
};
use crate::sink::FileSink;

/// 助手通道接收端默认空闲超时。
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct ServerEvent {
    pub level: &'static str,
    pub message: String,
}

#[allow(clippy::type_complexity)]
pub struct ServerHooks<'a> {
    pub approve: Option<&'a mut dyn FnMut(&Hello) -> bool>,
    pub progress: Option<&'a mut dyn FnMut(&Hello, u64, u64)>,
    pub log: Option<&'a mut dyn FnMut(ServerEvent)>,
    pub idle_timeout: Duration,
}

impl<'a> Default for ServerHooks<'a> {
    fn default() -> Self {
        Self {
            approve: None,
            progress: None,
            log: None,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ServerOutcome {
    Completed {
        hello: Hello,
        path: PathBuf,
        sha256: String,
    },
    Rejected {
        hello: Hello,
    },
    Busy,
    Canceled {
        hello: Hello,
    },
    Failed {
        hello: Option<Hello>,
        message: String,
        keep_partial: bool,
    },
}

impl ServerOutcome {
    pub fn hello(&self) -> Option<&Hello> {
        match self {
            Self::Completed { hello, .. } | Self::Rejected { hello } | Self::Canceled { hello } => Some(hello),
            Self::Failed { hello, .. } => hello.as_ref(),
            Self::Busy => None,
        }
    }

    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }
}

/// 向新连接回复 BUSY 并关闭，用于单活跃接收端的并发拒绝。
pub fn send_busy(stream: &mut dyn TransportStream) -> Result<(), BtError> {
    let _ = stream.set_timeouts(Some(POLL_INTERVAL), Some(Duration::from_secs(10)));
    encode_offer(&Offer::busy())?.write_to(stream)?;
    let _ = stream.flush();
    let _ = stream.close_stream();
    Ok(())
}

/// 处理单个已接受的助手连接；调用方负责单活跃与 BUSY 语义。
pub fn serve_connection(
    stream: &mut dyn TransportStream,
    sink: &FileSink,
    hooks: &mut ServerHooks<'_>,
) -> ServerOutcome {
    let _ = stream.set_timeouts(Some(POLL_INTERVAL), Some(hooks.idle_timeout));
    let first = match read_frame_poll(stream, || false, hooks.idle_timeout) {
        Ok(frame) => frame,
        Err(error) => {
            let _ = send_error(stream, ErrorCode::IoError, &error.to_string());
            return failed(None, &error.to_string(), false);
        }
    };
    if first.kind() != FrameKind::Hello {
        let message = "首个消息必须是 HELLO".to_string();
        let _ = send_error(stream, ErrorCode::Mismatch, &message);
        return failed(None, &message, false);
    }
    let hello = match Hello::decode(first.payload()).and_then(|hello| {
        hello.validate()?;
        Ok(hello)
    }) {
        Ok(hello) => hello,
        Err(error) => {
            let message = error.to_string();
            let _ = send_error(stream, ErrorCode::Mismatch, &message);
            return failed(None, &message, false);
        }
    };
    log(
        hooks,
        "INFO",
        format!("收到传输请求：{}（{} 字节）", hello.file_name, hello.file_size),
    );

    let approved = match hooks.approve.as_mut() {
        Some(approve) => approve(&hello),
        None => true,
    };
    if !approved {
        let _ = encode_offer(&Offer::reject("用户拒绝")).and_then(|frame| frame.write_to(stream));
        return ServerOutcome::Rejected { hello };
    }

    let offset = match sink.open(&hello) {
        Ok(offset) => offset,
        Err(error) => {
            let message = error.to_string();
            let _ = send_error(stream, ErrorCode::IoError, &message);
            return failed(Some(hello), &message, true);
        }
    };
    if let Err(error) = encode_offer(&Offer::accept(offset as i64)).and_then(|frame| frame.write_to(stream)) {
        let _ = sink.abort(&hello, false);
        return failed(Some(hello), &error.to_string(), true);
    }

    let total = hello.file_size as u64;
    let mut current = offset.min(total);
    if let Some(progress) = hooks.progress.as_mut() {
        progress(&hello, current, total);
    }
    if current >= total {
        return finish(stream, sink, hooks, hello);
    }

    loop {
        let frame = match read_frame_poll(stream, || false, hooks.idle_timeout) {
            Ok(frame) => frame,
            Err(error) => {
                let message = error.to_string();
                let _ = sink.abort(&hello, false);
                let _ = send_error(stream, ErrorCode::IoError, &message);
                return failed(Some(hello), &message, true);
            }
        };
        match frame.kind() {
            FrameKind::Data => {
                let data = match crate::protocol::decode_data(frame.payload()) {
                    Ok(data) => data,
                    Err(error) => {
                        let message = error.to_string();
                        let _ = sink.abort(&hello, false);
                        let _ = send_error(stream, ErrorCode::Mismatch, &message);
                        return failed(Some(hello), &message, true);
                    }
                };
                if data.offset != current as i64 {
                    let message = format!("数据偏移不一致：期望 {current}，收到 {}", data.offset);
                    let _ = sink.abort(&hello, false);
                    let _ = send_error(stream, ErrorCode::Mismatch, &message);
                    return failed(Some(hello), &message, true);
                }
                if data.bytes.len() as u64 > total - current {
                    let message = format!(
                        "数据超出文件大小：偏移 {current}+长度 {} 超过 {total}",
                        data.bytes.len()
                    );
                    let _ = sink.abort(&hello, false);
                    let _ = send_error(stream, ErrorCode::Mismatch, &message);
                    return failed(Some(hello), &message, true);
                }
                if let Err(error) = sink.write(&hello, current, data.bytes) {
                    let message = error.to_string();
                    let _ = sink.abort(&hello, false);
                    let _ = send_error(stream, ErrorCode::IoError, &message);
                    return failed(Some(hello), &message, true);
                }
                current += data.bytes.len() as u64;
                if let Err(error) = encode_ack(current as i64).and_then(|frame| frame.write_to(stream)) {
                    let _ = sink.abort(&hello, false);
                    return failed(Some(hello), &error.to_string(), true);
                }
                if let Some(progress) = hooks.progress.as_mut() {
                    progress(&hello, current, total);
                }
                if current >= total {
                    return finish(stream, sink, hooks, hello);
                }
            }
            FrameKind::Cancel => {
                let reason = Cancel::decode(frame.payload()).map(|c| c.reason).unwrap_or_default();
                let _ = sink.abort(&hello, false);
                log(
                    hooks,
                    "INFO",
                    format!("传输已取消（保留半成品）：{} {}", hello.file_name, reason),
                );
                return ServerOutcome::Canceled { hello };
            }
            FrameKind::Error => {
                let message = ErrorMessage::decode(frame.payload())
                    .map(|e| e.message)
                    .unwrap_or_else(|_| "对端报错".to_string());
                let _ = sink.abort(&hello, false);
                log(
                    hooks,
                    "INFO",
                    format!("对端报错，已保留半成品：{} {message}", hello.file_name),
                );
                return failed(Some(hello), &message, true);
            }
            other => {
                let message = format!("协议错误：意外消息 {}", other.as_byte());
                let _ = sink.abort(&hello, false);
                let _ = send_error(stream, ErrorCode::Mismatch, &message);
                return failed(Some(hello), &message, true);
            }
        }
    }
}

fn finish(
    stream: &mut dyn TransportStream,
    sink: &FileSink,
    hooks: &mut ServerHooks<'_>,
    hello: Hello,
) -> ServerOutcome {
    let (done, final_path) = match sink.complete_with_path(&hello) {
        Ok(result) => result,
        Err(error) => {
            let message = error.to_string();
            let _ = send_error(stream, ErrorCode::IoError, &message);
            return failed(Some(hello), &message, true);
        }
    };
    if let Err(error) = encode_done(&done).and_then(|frame| frame.write_to(stream)) {
        return failed(Some(hello), &error.to_string(), true);
    }
    if done.ok {
        let path = final_path.unwrap_or_else(|| sink.dir().to_path_buf());
        log(
            hooks,
            "INFO",
            format!(
                "接收完成：{}（SHA-256 {}）",
                hello.file_name,
                done.hash.chars().take(8).collect::<String>()
            ),
        );
        ServerOutcome::Completed {
            hello,
            path,
            sha256: done.hash,
        }
    } else {
        failed(Some(hello), "SHA-256 校验失败", false)
    }
}

fn failed(hello: Option<Hello>, message: &str, keep_partial: bool) -> ServerOutcome {
    ServerOutcome::Failed {
        hello,
        message: message.to_string(),
        keep_partial,
    }
}

fn send_error(stream: &mut dyn TransportStream, code: ErrorCode, message: &str) -> Result<(), BtError> {
    encode_error(code as i32, message)?.write_to(stream)?;
    let _ = stream.flush();
    Ok(())
}

fn log(hooks: &mut ServerHooks<'_>, level: &'static str, message: String) {
    if let Some(log) = hooks.log.as_mut() {
        log(ServerEvent { level, message });
    }
}

/// 帧类型断言辅助，供接收服务在握手前区分协议错误。
pub fn expect_kind(frame: &Frame, kind: FrameKind) -> Result<(), BtError> {
    if frame.kind() == kind {
        Ok(())
    } else {
        Err(BtError::new(
            BtErrorKind::Protocol,
            format!("期望 {:?}，收到 {:?}", kind, frame.kind()),
        ))
    }
}

/// OFFER 编码辅助，接收服务在 BUSY 路径上复用。
pub fn offer_frame(offer: &Offer) -> Result<Frame, BtError> {
    encode_offer(offer)
}

/// DONE 编码辅助。
pub fn done_frame(done: &Done) -> Result<Frame, BtError> {
    encode_done(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_exposes_hello() {
        let hello = Hello::new("a.bin", 1, "ab".repeat(32), 4096);
        assert!(ServerOutcome::Completed {
            hello: hello.clone(),
            path: PathBuf::from("/tmp/a"),
            sha256: "x".into()
        }
        .is_completed());
        assert_eq!(ServerOutcome::Busy.hello(), None);
    }

    #[test]
    fn expect_kind_rejects_mismatch() {
        let frame = encode_offer(&Offer::busy()).unwrap();
        assert!(expect_kind(&frame, FrameKind::Offer).is_ok());
        assert!(expect_kind(&frame, FrameKind::Ack).is_err());
    }
}
