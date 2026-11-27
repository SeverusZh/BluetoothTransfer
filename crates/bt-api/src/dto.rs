use std::path::PathBuf;

use bt_core::{
    AssistantState, Channel, Config, DeviceId, ProtectionLevel, Record, RecordKind, RecordStatus, SendSource,
    TransferMode,
};
use serde::{Deserialize, Serialize};

/// 发送任务标识，进程内单调递增。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JobId(pub u64);

impl JobId {
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// 接收询问标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AskId(pub u64);

impl AskId {
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// 配对交互标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PinRequestId(pub u64);

impl PinRequestId {
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// 任务生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Connecting,
    Sending,
    Retrying,
    Paused,
    Completed,
    Failed,
    Canceled,
}

impl JobState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Connecting => "connecting",
            Self::Sending => "sending",
            Self::Retrying => "retrying",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
    }

    pub const fn is_running(self) -> bool {
        matches!(self, Self::Connecting | Self::Sending | Self::Retrying)
    }
}

/// 发送条目的来源快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendItem {
    pub source: SendSource,
    pub display_name: String,
    pub total_bytes: Option<u64>,
    pub kind: RecordKind,
}

impl SendItem {
    /// 由来源推导展示名与形态；字节数需要访问文件系统，由引擎入队时补齐。
    pub fn from_source(source: SendSource) -> Self {
        let kind = match &source {
            SendSource::File(_) => RecordKind::File,
            SendSource::Files(_) => RecordKind::MultiFile,
            SendSource::Folder(_) => RecordKind::Folder,
            SendSource::Text(_) => RecordKind::Text,
        };
        Self {
            display_name: source.display_name(),
            source,
            total_bytes: None,
            kind,
        }
    }
}

/// 任务快照，界面与 CLI 都按该结构展示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub id: JobId,
    pub device_id: DeviceId,
    pub device_name: String,
    pub item: SendItem,
    pub channel: Channel,
    pub state: JobState,
    pub bytes_sent: u64,
    pub total_bytes: u64,
    pub bps: u64,
    pub eta_seconds: Option<u64>,
    pub attempts: u32,
    pub error: Option<String>,
    pub resumable: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl JobSnapshot {
    pub fn percent(&self) -> u8 {
        if self.total_bytes == 0 {
            return if self.state.is_terminal() { 100 } else { 0 };
        }
        let ratio = self.bytes_sent.min(self.total_bytes) as f64 / self.total_bytes as f64;
        (ratio * 100.0).round().clamp(0.0, 100.0) as u8
    }
}

/// 扫描到的设备条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub alias: Option<String>,
    pub paired: bool,
    pub connected: bool,
    pub favorite: bool,
    pub rssi: Option<i16>,
    pub last_seen: Option<String>,
    pub assistant_state: AssistantState,
    pub channels: Vec<Channel>,
}

impl Device {
    pub fn new(id: DeviceId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            alias: None,
            paired: false,
            connected: false,
            favorite: false,
            rssi: None,
            last_seen: None,
            assistant_state: AssistantState::Unknown,
            channels: vec![Channel::Opp],
        }
    }

    /// 别名优先的展示名。
    pub fn label(&self) -> String {
        match &self.alias {
            Some(alias) if !alias.trim().is_empty() => alias.clone(),
            _ if !self.name.trim().is_empty() => self.name.clone(),
            _ => self.id.to_string(),
        }
    }

    pub fn is_assistant_ready(&self) -> bool {
        matches!(self.assistant_state, AssistantState::Available)
    }
}

/// 扫描选项。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanOptions {
    /// 扫描时长上限，None 表示由后端决定默认值。
    pub duration_s: Option<u32>,
    /// 是否同时刷新已配对设备列表。
    pub refresh_paired: bool,
}

/// 接收服务启动选项。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveOptions {
    pub dir: Option<PathBuf>,
    pub ask: Option<bool>,
}

/// 待决的接收询问。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveAsk {
    pub id: AskId,
    pub name: String,
    pub size: u64,
    pub device: Option<DeviceId>,
}

/// 接收服务快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveSession {
    pub listening: bool,
    pub dir: PathBuf,
    pub ask: bool,
    pub pending_ask: Option<ReceiveAsk>,
    pub active_name: Option<String>,
    pub received_bytes: u64,
}

impl ReceiveSession {
    pub fn idle(dir: PathBuf, ask: bool) -> Self {
        Self {
            listening: false,
            dir,
            ask,
            pending_ask: None,
            active_name: None,
            received_bytes: 0,
        }
    }
}

/// 记录查询条件。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordQuery {
    pub direction: Option<bt_core::Direction>,
    pub channel: Option<Channel>,
    pub kind: Option<RecordKind>,
    pub status: Option<RecordStatus>,
    pub device_id: Option<DeviceId>,
    pub search: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl RecordQuery {
    pub fn latest(limit: u32) -> Self {
        Self {
            limit: Some(limit),
            ..Self::default()
        }
    }

    pub fn matches(&self, record: &Record) -> bool {
        if self.direction.is_some_and(|value| value != record.direction) {
            return false;
        }
        if self.channel.is_some_and(|value| value != record.channel) {
            return false;
        }
        if self.kind.is_some_and(|value| value != record.kind) {
            return false;
        }
        if self.status.is_some_and(|value| value != record.status) {
            return false;
        }
        if let Some(device) = self.device_id {
            if record.device_id != Some(device) {
                return false;
            }
        }
        if let Some(search) = self.search.as_deref() {
            if !search.is_empty() {
                let needle = search.to_lowercase();
                let name = record.display_name.to_lowercase();
                let device = record.device_name.to_lowercase();
                if !name.contains(&needle) && !device.contains(&needle) {
                    return false;
                }
            }
        }
        if let Some(from) = self.from.as_deref() {
            if record.started_at.as_str() < from {
                return false;
            }
        }
        if let Some(to) = self.to.as_deref() {
            if record.started_at.as_str() > to {
                return false;
            }
        }
        true
    }
}

/// 单日统计。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayStat {
    pub date: String,
    pub sent: u64,
    pub received: u64,
    pub bytes: u64,
}

/// 汇总统计。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub total: u64,
    pub sent: u64,
    pub received: u64,
    pub failed: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub last_activity: Option<String>,
    pub days: Vec<DayStat>,
}

/// 导出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Jsonl,
    Csv,
}

impl ExportFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Csv => "csv",
        }
    }
}

/// 导出结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportSummary {
    pub path: PathBuf,
    pub rows: u64,
    pub bytes: u64,
    pub format: ExportFormat,
}

/// 复制计划中的单项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CopyItem {
    Path(PathBuf),
    Text(String),
    Missing(PathBuf),
}

/// 复制计划形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyKind {
    Files,
    Text,
    Mixed,
}

/// 记录复制到剪贴板或文件管理器的计划，由平台层执行。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyPlan {
    pub items: Vec<CopyItem>,
    pub kind: Option<CopyKind>,
}

impl CopyPlan {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn missing_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| matches!(item, CopyItem::Missing(_)))
            .count()
    }
}

/// 配对交互类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinPromptKind {
    /// 需要输入 PIN 并回填。
    Pin,
    /// 只需要确认是或否。
    Confirm,
    /// 对端要求显示匹配口令。
    Passkey(u32),
}

/// 配对交互请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinRequest {
    pub id: PinRequestId,
    pub device: DeviceId,
    pub kind: PinPromptKind,
}

/// 自检单项结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelftestCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// 自检汇总。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelftestReport {
    pub checks: Vec<SelftestCheck>,
}

impl SelftestReport {
    pub fn passed(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|check| check.ok)
    }

    pub fn push(&mut self, name: impl Into<String>, ok: bool, detail: impl Into<String>) {
        self.checks.push(SelftestCheck {
            name: name.into(),
            ok,
            detail: detail.into(),
        });
    }

    pub fn failed_names(&self) -> Vec<&str> {
        self.checks
            .iter()
            .filter(|check| !check.ok)
            .map(|check| check.name.as_str())
            .collect()
    }
}

/// 旧版本数据迁移选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrateOptions {
    /// 旧应用数据根目录，内含 config.json 与历史记录。
    pub legacy_root: PathBuf,
    /// 目标数据目录，None 表示使用当前应用目录。
    pub target_dir: Option<PathBuf>,
    /// 只探测不写入。
    pub dry_run: bool,
}

impl MigrateOptions {
    pub fn new(legacy_root: PathBuf) -> Self {
        Self {
            legacy_root,
            target_dir: None,
            dry_run: false,
        }
    }
}

/// 迁移结果。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrateReport {
    pub imported_records: u64,
    pub imported_config: bool,
    pub copied_files: u64,
    pub skipped: u64,
    pub warnings: Vec<String>,
    pub dry_run: bool,
}

/// 配置增量补丁，未出现的字段保持原值。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigPatch {
    pub opp_chunk_size: Option<u32>,
    pub opp_connect_timeout_s: Option<u32>,
    pub opp_send_idle_timeout_s: Option<u32>,
    pub opp_retry_count: Option<u32>,
    pub opp_retry_delay_s: Option<u32>,
    pub opp_protection: Option<ProtectionLevel>,
    pub opp_name_bom: Option<bool>,
    pub assistant_chunk_size: Option<u32>,
    pub push_text_file_name: Option<String>,
    pub receive_dir: Option<PathBuf>,
    pub receive_ask: Option<bool>,
    pub mode: Option<TransferMode>,
}

impl ConfigPatch {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// 就地应用补丁，随后由调用方归一化。
    pub fn apply(&self, config: &mut Config) {
        if let Some(value) = self.opp_chunk_size {
            config.opp_chunk_size = value;
        }
        if let Some(value) = self.opp_connect_timeout_s {
            config.opp_connect_timeout_s = value;
        }
        if let Some(value) = self.opp_send_idle_timeout_s {
            config.opp_send_idle_timeout_s = value;
        }
        if let Some(value) = self.opp_retry_count {
            config.opp_retry_count = value;
        }
        if let Some(value) = self.opp_retry_delay_s {
            config.opp_retry_delay_s = value;
        }
        if let Some(value) = self.opp_protection {
            config.opp_protection = value;
        }
        if let Some(value) = self.opp_name_bom {
            config.opp_name_bom = value;
        }
        if let Some(value) = self.assistant_chunk_size {
            config.assistant_chunk_size = value;
        }
        if let Some(value) = self.push_text_file_name.clone() {
            config.push_text_file_name = value;
        }
        if let Some(value) = self.receive_dir.clone() {
            config.receive_dir = value;
        }
        if let Some(value) = self.receive_ask {
            config.receive_ask = value;
        }
        if let Some(value) = self.mode {
            config.mode = value;
        }
    }
}

/// 日志级别，事件与日志文件共用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::{Direction, RecordKind};

    #[test]
    fn config_patch_applies_only_present_fields() {
        let mut config = Config::default();
        let patch = ConfigPatch {
            opp_retry_count: Some(7),
            mode: Some(TransferMode::Opp),
            ..Default::default()
        };
        patch.apply(&mut config);
        assert_eq!(config.opp_retry_count, 7);
        assert_eq!(config.mode, TransferMode::Opp);
        assert_eq!(config.opp_chunk_size, bt_core::DEFAULT_OPP_CHUNK_SIZE);
        assert!(!patch.is_empty());
        assert!(ConfigPatch::default().is_empty());
    }

    #[test]
    fn record_query_filters_by_search_and_direction() {
        let mut record = Record::running(
            Direction::Send,
            Channel::Opp,
            RecordKind::File,
            "photo.jpg",
            "2026-01-01T00:00:00Z",
        );
        record.device_name = "Pixel".to_string();
        assert!(RecordQuery::default().matches(&record));
        assert!(RecordQuery {
            search: Some("photo".into()),
            ..Default::default()
        }
        .matches(&record));
        assert!(RecordQuery {
            search: Some("pixel".into()),
            ..Default::default()
        }
        .matches(&record));
        assert!(!RecordQuery {
            search: Some("absent".into()),
            ..Default::default()
        }
        .matches(&record));
        assert!(!RecordQuery {
            direction: Some(Direction::Receive),
            ..Default::default()
        }
        .matches(&record));
    }

    #[test]
    fn device_label_prefers_alias() {
        let id = DeviceId::parse("AA:BB:CC:DD:EE:FF").unwrap();
        let mut device = Device::new(id, "Pixel 9");
        assert_eq!(device.label(), "Pixel 9");
        device.alias = Some("工作手机".to_string());
        assert_eq!(device.label(), "工作手机");
    }

    #[test]
    fn job_snapshot_percent_is_bounded() {
        let id = DeviceId::from_bytes([1, 2, 3, 4, 5, 6]);
        let item = SendItem::from_source(SendSource::Text("hello".into()));
        let job = JobSnapshot {
            id: JobId(1),
            device_id: id,
            device_name: "t".into(),
            item,
            channel: Channel::Opp,
            state: JobState::Sending,
            bytes_sent: 50,
            total_bytes: 100,
            bps: 0,
            eta_seconds: None,
            attempts: 1,
            error: None,
            resumable: false,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        assert_eq!(job.percent(), 50);
        assert!(!job.state.is_terminal());
    }
}
