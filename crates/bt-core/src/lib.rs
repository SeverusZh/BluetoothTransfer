//! 传输层无关的核心模型：设备标识、通道、配置、记录、错误、路径与哈希工具。

pub mod config;
pub mod device;
pub mod error;
pub mod hash;
pub mod paths;
pub mod record;
pub mod time;
pub mod transfer;

pub use config::{
    Config, ProtectionLevel, TransferMode, DEFAULT_ASSISTANT_CHUNK_SIZE, DEFAULT_OPP_CHUNK_SIZE,
    DEFAULT_OPP_CONNECT_TIMEOUT_S, DEFAULT_OPP_RETRY_COUNT, DEFAULT_OPP_RETRY_DELAY_S, DEFAULT_OPP_SEND_IDLE_TIMEOUT_S,
    DEFAULT_PUSH_TEXT_FILE_NAME, MAX_ASSISTANT_CHUNK_SIZE, MAX_OPP_CHUNK_SIZE, MIN_ASSISTANT_CHUNK_SIZE,
    MIN_OPP_CHUNK_SIZE,
};
pub use device::DeviceId;
pub use error::{BtError, BtErrorKind};
pub use hash::{hex_lower, parse_transfer_id, sha256, sha256_file, short_hex, TransferId};
pub use record::{Record, MAX_RECORD_CONTENT_BYTES};
pub use time::{format_bytes, format_duration, now_rfc3339, parse_rfc3339, rfc3339_from_unix, unix_now};
pub use transfer::{AssistantState, Channel, Direction, RecordKind, RecordStatus, SendSource};

/// 助手通道私有服务的 SDP UUID。
pub const ASSISTANT_SERVICE_UUID: &str = "9F3C0E2A-1B4D-4E6F-8A9B-0C1D2E3F4A5B";

/// 助手协议单帧上限，含帧头。
pub const ASSISTANT_MAX_FRAME_SIZE: u32 = 1024 * 1024;

/// 助手协议帧头长度。
pub const ASSISTANT_FRAME_HEADER_SIZE: u32 = 7;

/// OPP 通道单包上限，超过后 OBEX 对端可能直接断开。
pub const OPP_MAX_PACKET_SIZE: u32 = 65_023;
