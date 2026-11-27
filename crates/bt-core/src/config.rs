use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::BtError;
use crate::transfer::Channel;

/// OPP 分片默认值取协议单包上限，实际生效值不会超过上限。
pub const DEFAULT_OPP_CHUNK_SIZE: u32 = 65_023;
pub const MIN_OPP_CHUNK_SIZE: u32 = 4_096;
pub const MAX_OPP_CHUNK_SIZE: u32 = 65_023;

pub const DEFAULT_OPP_CONNECT_TIMEOUT_S: u32 = 30;
pub const DEFAULT_OPP_SEND_IDLE_TIMEOUT_S: u32 = 300;
pub const DEFAULT_OPP_RETRY_COUNT: u32 = 3;
pub const DEFAULT_OPP_RETRY_DELAY_S: u32 = 3;

pub const DEFAULT_ASSISTANT_CHUNK_SIZE: u32 = 262_144;
pub const MIN_ASSISTANT_CHUNK_SIZE: u32 = 4_096;

/// 分片上限由单帧上限扣除帧头与 DATA 载荷内的偏移、长度字段得出。
pub const MAX_ASSISTANT_CHUNK_SIZE: u32 = crate::ASSISTANT_MAX_FRAME_SIZE - crate::ASSISTANT_FRAME_HEADER_SIZE - 12;

pub const DEFAULT_PUSH_TEXT_FILE_NAME: &str = "bt-note.txt";

/// OPP 连接保护级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProtectionLevel {
    #[default]
    Auto,
    Plain,
    Encrypt,
}

impl ProtectionLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Plain => "plain",
            Self::Encrypt => "encrypt",
        }
    }
}

impl fmt::Display for ProtectionLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProtectionLevel {
    type Err = BtError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Ok(Self::Auto),
            "plain" => Ok(Self::Plain),
            "encrypt" => Ok(Self::Encrypt),
            other => Err(BtError::invalid(format!("未知保护级别 {other}"))),
        }
    }
}

/// 发送通道模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransferMode {
    #[default]
    Auto,
    Assistant,
    Opp,
}

impl TransferMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Assistant => "assistant",
            Self::Opp => "opp",
        }
    }
}

impl fmt::Display for TransferMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TransferMode {
    type Err = BtError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Ok(Self::Auto),
            "assistant" => Ok(Self::Assistant),
            "opp" => Ok(Self::Opp),
            other => Err(BtError::invalid(format!("未知通道模式 {other}"))),
        }
    }
}

/// 持久化配置。
///
/// 共 13 项设置语义：此结构承载其中 12 项；第 13 项 OPP 认证密码不进配置与配置文件，
/// 由 `bt_api::Backend::set_auth_password` 写入操作系统私密存储或权限受限的独立文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub opp_chunk_size: u32,
    pub opp_connect_timeout_s: u32,
    pub opp_send_idle_timeout_s: u32,
    pub opp_retry_count: u32,
    pub opp_retry_delay_s: u32,
    pub opp_protection: ProtectionLevel,
    pub opp_name_bom: bool,
    pub assistant_chunk_size: u32,
    pub push_text_file_name: String,
    pub receive_dir: PathBuf,
    pub receive_ask: bool,
    pub mode: TransferMode,
}

impl Config {
    /// 以给定接收目录构造默认配置；平台相关的目录由装配层注入。
    pub fn default_for(receive_dir: PathBuf) -> Self {
        Self {
            opp_chunk_size: DEFAULT_OPP_CHUNK_SIZE,
            opp_connect_timeout_s: DEFAULT_OPP_CONNECT_TIMEOUT_S,
            opp_send_idle_timeout_s: DEFAULT_OPP_SEND_IDLE_TIMEOUT_S,
            opp_retry_count: DEFAULT_OPP_RETRY_COUNT,
            opp_retry_delay_s: DEFAULT_OPP_RETRY_DELAY_S,
            opp_protection: ProtectionLevel::Auto,
            opp_name_bom: false,
            assistant_chunk_size: DEFAULT_ASSISTANT_CHUNK_SIZE,
            push_text_file_name: DEFAULT_PUSH_TEXT_FILE_NAME.to_string(),
            receive_dir,
            receive_ask: true,
            mode: TransferMode::Auto,
        }
    }

    pub fn chunk_size_for(&self, channel: Channel) -> u32 {
        match channel {
            Channel::Opp => self.opp_chunk_size,
            Channel::Assistant => self.assistant_chunk_size,
        }
    }

    /// 把手工改动或旧配置中的越界值裁剪到合法区间。
    pub fn normalize(&mut self) {
        self.opp_chunk_size = self.opp_chunk_size.clamp(MIN_OPP_CHUNK_SIZE, MAX_OPP_CHUNK_SIZE);
        self.opp_connect_timeout_s = self.opp_connect_timeout_s.clamp(1, 3_600);
        self.opp_send_idle_timeout_s = self.opp_send_idle_timeout_s.clamp(1, 86_400);
        self.opp_retry_count = self.opp_retry_count.min(10);
        self.opp_retry_delay_s = self.opp_retry_delay_s.min(300);
        self.assistant_chunk_size = self
            .assistant_chunk_size
            .clamp(MIN_ASSISTANT_CHUNK_SIZE, MAX_ASSISTANT_CHUNK_SIZE);
        let name = self.push_text_file_name.trim();
        self.push_text_file_name = if name.is_empty() {
            DEFAULT_PUSH_TEXT_FILE_NAME.to_string()
        } else {
            crate::paths::sanitize_file_name(name)
        };
        if self.receive_dir.as_os_str().is_empty() {
            self.receive_dir = PathBuf::from("BluetoothReceive");
        }
    }

    pub fn normalized(mut self) -> Self {
        self.normalize();
        self
    }

    /// 校验配置是否可以落盘使用。
    pub fn validate(&self) -> Result<(), BtError> {
        if !(MIN_OPP_CHUNK_SIZE..=MAX_OPP_CHUNK_SIZE).contains(&self.opp_chunk_size) {
            return Err(BtError::invalid("OPP 分片大小越界"));
        }
        if !(MIN_ASSISTANT_CHUNK_SIZE..=MAX_ASSISTANT_CHUNK_SIZE).contains(&self.assistant_chunk_size) {
            return Err(BtError::invalid("助手通道分片大小越界"));
        }
        if self.opp_connect_timeout_s == 0 || self.opp_send_idle_timeout_s == 0 {
            return Err(BtError::invalid("OPP 超时不能为 0"));
        }
        if self.push_text_file_name.trim().is_empty() {
            return Err(BtError::invalid("文本推送文件名不能为空"));
        }
        if self.receive_dir.as_os_str().is_empty() {
            return Err(BtError::invalid("接收目录不能为空"));
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::default_for(PathBuf::from("BluetoothReceive"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_within_documented_bounds() {
        let config = Config::default();
        assert_eq!(config.opp_chunk_size, DEFAULT_OPP_CHUNK_SIZE);
        assert_eq!(config.assistant_chunk_size, DEFAULT_ASSISTANT_CHUNK_SIZE);
        assert_eq!(config.opp_connect_timeout_s, 30);
        assert_eq!(config.opp_send_idle_timeout_s, 300);
        assert_eq!(config.opp_retry_count, 3);
        assert_eq!(config.opp_retry_delay_s, 3);
        assert_eq!(config.opp_protection, ProtectionLevel::Auto);
        assert!(!config.opp_name_bom);
        assert_eq!(config.mode, TransferMode::Auto);
        assert!(config.receive_ask);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn normalize_clamps_out_of_range_values() {
        let mut config = Config {
            opp_chunk_size: 4,
            assistant_chunk_size: 10 * 1024 * 1024,
            opp_retry_count: 99,
            push_text_file_name: "  ".to_string(),
            ..Config::default()
        };
        config.normalize();
        assert_eq!(config.opp_chunk_size, MIN_OPP_CHUNK_SIZE);
        assert_eq!(config.assistant_chunk_size, MAX_ASSISTANT_CHUNK_SIZE);
        assert_eq!(config.opp_retry_count, 10);
        assert_eq!(config.push_text_file_name, DEFAULT_PUSH_TEXT_FILE_NAME);
    }

    #[test]
    fn channel_chunk_size_follows_config() {
        let config = Config::default();
        assert_eq!(config.chunk_size_for(Channel::Opp), config.opp_chunk_size);
        assert_eq!(config.chunk_size_for(Channel::Assistant), config.assistant_chunk_size);
    }

    #[test]
    fn parses_transfer_mode_and_protection_level() {
        assert_eq!("auto".parse::<TransferMode>().unwrap(), TransferMode::Auto);
        assert_eq!("OPP".parse::<TransferMode>().unwrap(), TransferMode::Opp);
        assert!("nope".parse::<TransferMode>().is_err());
        assert_eq!("encrypt".parse::<ProtectionLevel>().unwrap(), ProtectionLevel::Encrypt);
        assert!("maybe".parse::<ProtectionLevel>().is_err());
    }
}
