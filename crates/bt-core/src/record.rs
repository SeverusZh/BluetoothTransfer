use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::device::DeviceId;
use crate::hash::TransferId;
use crate::transfer::{Channel, Direction, RecordKind, RecordStatus};

/// 文本记录正文入库上限；超过后正文不入库，避免记录体积失控。
pub const MAX_RECORD_CONTENT_BYTES: usize = 1024 * 1024;

/// 一条传输记录；字段与数据库表、导出格式一一对应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: i64,
    pub direction: Direction,
    pub channel: Channel,
    pub kind: RecordKind,
    pub status: RecordStatus,
    pub device_id: Option<DeviceId>,
    pub device_name: String,
    pub display_name: String,
    pub path: Option<PathBuf>,
    pub checksum: Option<TransferId>,
    pub content: Option<String>,
    pub byte_size: u64,
    pub transferred: u64,
    pub error: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub resumable: bool,
}

impl Record {
    /// 新建一条进行中的记录，时间戳由调用方注入便于测试。
    pub fn running(
        direction: Direction,
        channel: Channel,
        kind: RecordKind,
        display_name: impl Into<String>,
        started_at: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            direction,
            channel,
            kind,
            status: RecordStatus::Running,
            device_id: None,
            device_name: String::new(),
            display_name: display_name.into(),
            path: None,
            checksum: None,
            content: None,
            byte_size: 0,
            transferred: 0,
            error: None,
            started_at: started_at.into(),
            finished_at: None,
            resumable: false,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }

    /// 正文是否在入库上限内；超出时返回 false，由调用方决定是否落库。
    pub fn content_within_limit(&self) -> bool {
        self.content
            .as_ref()
            .is_none_or(|content| content.len() <= MAX_RECORD_CONTENT_BYTES)
    }

    /// 完成度百分比，用于界面进度展示。
    pub fn percent(&self) -> u8 {
        if self.byte_size == 0 {
            return if self.status.is_terminal() { 100 } else { 0 };
        }
        let ratio = self.transferred.min(self.byte_size) as f64 / self.byte_size as f64;
        (ratio * 100.0).round().clamp(0.0, 100.0) as u8
    }
}
