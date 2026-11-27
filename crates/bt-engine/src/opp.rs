use std::fs::File;
use std::path::Path;
use std::time::Duration;

use bt_core::{BtError, BtErrorKind, DeviceId, ProtectionLevel};
use bt_transport::{RfcommConnector, ServiceOptions, OPP_SERVICE_UUID};

use crate::cancel::CancelFlag;
use crate::client::POLL_INTERVAL;
use crate::obex::{ObexClient, ObexOptions, ObexPushResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OppOptions {
    pub protection: ProtectionLevel,
    pub chunk_size: u32,
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
    pub name_use_bom: bool,
    pub auth_password: String,
}

impl Default for OppOptions {
    fn default() -> Self {
        Self {
            protection: ProtectionLevel::Auto,
            chunk_size: 32_768,
            connect_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(300),
            name_use_bom: false,
            auth_password: String::new(),
        }
    }
}

/// OPP 推送失败重试策略：临时失败按指数退避，永久失败直接放弃。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OppRetryPolicy {
    max_attempts: u32,
    base_delay: Duration,
}

impl OppRetryPolicy {
    pub fn new(retry_count: u32, base_delay: Duration) -> Self {
        Self {
            max_attempts: retry_count.saturating_add(1).max(1),
            base_delay: base_delay.max(Duration::from_millis(1)),
        }
    }

    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// attempt 从 1 开始。
    pub fn should_retry(&self, attempt: u32, error: &BtError) -> bool {
        attempt < self.max_attempts && is_retryable(error)
    }

    /// 第 attempt 次失败后的等待：base * 2^(attempt-1)，指数上限 30。
    pub fn next_delay(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1).min(30);
        self.base_delay.saturating_mul(1u32 << exponent)
    }
}

pub fn is_retryable(error: &BtError) -> bool {
    match error.kind() {
        BtErrorKind::Canceled | BtErrorKind::InvalidArgument | BtErrorKind::Unsupported | BtErrorKind::NotFound => {
            return false
        }
        BtErrorKind::Timeout | BtErrorKind::Busy => return true,
        _ => {}
    }
    let message = error.message();
    if message.is_empty() {
        return true;
    }
    if message.contains("0xC3") || message.contains("0xc3") {
        return true;
    }
    if message.contains("提前关闭") || message.contains("连接") || message.contains("超时") {
        return true;
    }
    if message.contains("设备未就绪") || message.contains("中止") {
        return true;
    }
    if message.contains("取消")
        || message.contains("未配对")
        || message.contains("不支持")
        || message.contains("未声明 OPP")
        || message.contains("不存在")
        || message.contains("超过 4GB")
    {
        return false;
    }
    false
}

pub fn mime_for_name(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let extension = lower.rsplit('.').next().unwrap_or("");
    match extension {
        "txt" => "text/plain",
        "zip" => "application/zip",
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        _ => "application/octet-stream",
    }
}

/// 单次 OPP 推送：连接 → CONNECT → 两阶段 PUT → DISCONNECT。
#[allow(clippy::too_many_arguments)]
pub fn push_file(
    connector: &dyn RfcommConnector,
    device: &DeviceId,
    path: &Path,
    name: &str,
    mime: &str,
    options: &OppOptions,
    cancel: &CancelFlag,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<ObexPushResult, BtError> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > u32::MAX as u64 {
        return Err(BtError::invalid("文件超过 OBEX 单对象 4GB 上限"));
    }
    let mut stream = connector.connect_service_with_options(
        device,
        OPP_SERVICE_UUID,
        ServiceOptions {
            protection: options.protection,
            timeout: Some(options.connect_timeout),
        },
    )?;
    stream.set_timeouts(Some(POLL_INTERVAL), Some(options.idle_timeout))?;
    let mut client = ObexClient::new(
        stream.as_mut(),
        ObexOptions {
            name_use_bom: options.name_use_bom,
        },
        &options.auth_password,
    );
    client.connect(cancel, options.connect_timeout)?;
    let mut source = file;
    let result = client.push(
        name,
        mime,
        size,
        options.chunk_size,
        &mut source,
        cancel,
        options.idle_timeout,
        progress,
    )?;
    let _ = client.disconnect(cancel, options.idle_timeout);
    Ok(result)
}

/// 带指数退避的 OPP 推送；重试前重新建立连接。
#[allow(clippy::too_many_arguments)]
pub fn push_file_with_retry(
    connector: &dyn RfcommConnector,
    device: &DeviceId,
    path: &Path,
    name: &str,
    mime: &str,
    options: &OppOptions,
    policy: &OppRetryPolicy,
    cancel: &CancelFlag,
    progress: &mut dyn FnMut(u64, u64),
    on_retry: &mut dyn FnMut(u32, &BtError, Duration),
) -> Result<ObexPushResult, BtError> {
    let mut attempt = 1u32;
    loop {
        if cancel.is_canceled() {
            return Err(BtError::canceled("用户取消"));
        }
        match push_file(connector, device, path, name, mime, options, cancel, progress) {
            Ok(result) => return Ok(result),
            Err(error) => {
                if cancel.is_canceled() {
                    return Err(BtError::canceled("用户取消"));
                }
                if !policy.should_retry(attempt, &error) {
                    return Err(error);
                }
                let delay = policy.next_delay(attempt);
                on_retry(attempt, &error, delay);
                sleep_cancelable(delay, cancel)?;
                attempt += 1;
            }
        }
    }
}

pub fn sleep_cancelable(delay: Duration, cancel: &CancelFlag) -> Result<(), BtError> {
    let deadline = std::time::Instant::now() + delay;
    while std::time::Instant::now() < deadline {
        if cancel.is_canceled() {
            return Err(BtError::canceled("用户取消"));
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obex::{header_id, opcode, response, ObexPacket};
    use bt_transport::mock::MockTransport;

    fn device() -> DeviceId {
        DeviceId::parse("AA:BB:CC:DD:EE:07").unwrap()
    }

    fn connect_ok() -> Vec<u8> {
        let mut packet = ObexPacket::new(response::OK);
        packet.include_connect_fields = true;
        packet.max_packet_length = u16::MAX;
        packet.encode().unwrap()
    }

    fn simple(code: u8) -> Vec<u8> {
        ObexPacket::new(code).encode().unwrap()
    }

    fn temp_file(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bt-opp-{tag}-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn retry_policy_matches_reference_rules() {
        let policy = OppRetryPolicy::new(3, Duration::from_secs(3));
        assert_eq!(policy.max_attempts(), 4);
        assert_eq!(OppRetryPolicy::new(0, Duration::ZERO).max_attempts(), 1);
        assert!(policy.should_retry(1, &BtError::timeout("连接或发送超时")));
        assert!(policy.should_retry(1, &BtError::protocol("OBEX PUT 失败：Forbidden (0xC3)")));
        assert!(!policy.should_retry(4, &BtError::timeout("超时")));
        assert!(!policy.should_retry(1, &BtError::invalid("设备未配对")));
        assert!(!policy.should_retry(1, &BtError::canceled("用户取消")));
        assert!(policy.should_retry(1, &BtError::new(BtErrorKind::Io, "")));
        assert_eq!(policy.next_delay(1), Duration::from_secs(3));
        assert_eq!(policy.next_delay(2), Duration::from_secs(6));
        assert_eq!(policy.next_delay(3), Duration::from_secs(12));
    }

    #[test]
    fn mime_lookup_covers_common_types() {
        assert_eq!(mime_for_name("a.TXT"), "text/plain");
        assert_eq!(mime_for_name("a.zip"), "application/zip");
        assert_eq!(mime_for_name("a.unknown"), "application/octet-stream");
        assert_eq!(mime_for_name("noext"), "application/octet-stream");
    }

    #[test]
    fn push_file_writes_two_phase_put_to_opp_service() {
        let path = temp_file("push", &[7u8; 128]);
        let mut inbound = connect_ok();
        inbound.extend_from_slice(&simple(response::CONTINUE));
        inbound.extend_from_slice(&simple(response::OK));
        let transport = MockTransport::new(Vec::new());
        transport.script_connect(inbound);
        let result = push_file(
            &transport,
            &device(),
            &path,
            "a.bin",
            "application/octet-stream",
            &OppOptions::default(),
            &CancelFlag::new(),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(result.bytes_sent, 128);
        let requests = transport.service_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].1, OPP_SERVICE_UUID);
        let written = transport.last_written();
        let mut packets = Vec::new();
        let mut pos = 0usize;
        while pos + 3 <= written.len() {
            let length = u16::from_be_bytes([written[pos + 1], written[pos + 2]]) as usize;
            packets.push(ObexPacket::decode(&written[pos..pos + length], false).unwrap());
            pos += length;
        }
        assert_eq!(packets.len(), 4);
        assert_eq!(packets[0].opcode, opcode::CONNECT);
        assert_eq!(packets[1].opcode, opcode::PUT);
        assert!(packets[1].headers.iter().all(|header| header.id != header_id::BODY));
        assert_eq!(packets[2].opcode, opcode::PUT_FINAL);
        assert_eq!(packets[3].opcode, opcode::DISCONNECT);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn retry_reconnects_after_scripted_failure() {
        let path = temp_file("retry", &[1u8; 32]);
        let transport = MockTransport::new(Vec::new());
        transport.script_failure();
        let mut inbound = connect_ok();
        inbound.extend_from_slice(&simple(response::CONTINUE));
        inbound.extend_from_slice(&simple(response::OK));
        transport.script_connect(inbound);
        let policy = OppRetryPolicy::new(1, Duration::from_millis(1));
        let mut retries = 0;
        let result = push_file_with_retry(
            &transport,
            &device(),
            &path,
            "a.bin",
            "application/octet-stream",
            &OppOptions::default(),
            &policy,
            &CancelFlag::new(),
            &mut |_, _| {},
            &mut |_, _, _| retries += 1,
        )
        .unwrap();
        assert_eq!(result.bytes_sent, 32);
        assert_eq!(retries, 1);
        assert_eq!(transport.service_requests().len(), 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn forbidden_0xc3_is_reported_and_retryable() {
        let path = temp_file("busy", &[1u8; 8]);
        let mut inbound = connect_ok();
        inbound.extend_from_slice(&simple(response::HTTP_FORBIDDEN));
        let transport = MockTransport::new(Vec::new());
        transport.script_connect(inbound);
        let error = push_file(
            &transport,
            &device(),
            &path,
            "a.bin",
            "",
            &OppOptions::default(),
            &CancelFlag::new(),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("0xC3"));
        assert!(is_retryable(&error));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cancel_before_push_returns_canceled() {
        let path = temp_file("cancel", &[1u8; 8]);
        let transport = MockTransport::new(Vec::new());
        let cancel = CancelFlag::new();
        cancel.cancel();
        let error = push_file(
            &transport,
            &device(),
            &path,
            "a.bin",
            "",
            &OppOptions::default(),
            &cancel,
            &mut |_, _| {},
        )
        .unwrap_err();
        assert_eq!(error.kind(), BtErrorKind::Canceled);
        let _ = std::fs::remove_file(path);
    }
}
