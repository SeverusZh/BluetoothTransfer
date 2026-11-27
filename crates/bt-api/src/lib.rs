//! 冻结契约：GUI 与 CLI 只通过本 crate 获取后端能力。
//!
//! 线程模型：后端一律 `Send + Sync`，由装配层以 `Arc<dyn Backend>` 共享；
//! 慢操作只排队并立即返回，进度通过 [`Event`] 回传；快查询同步返回缓存。

pub mod backend;
pub mod bus;
pub mod dto;
pub mod event;
pub mod mock;

pub use backend::{Backend, SharedBackend};
pub use bus::EventBus;
pub use dto::{
    AskId, ConfigPatch, CopyItem, CopyKind, CopyPlan, DayStat, Device, ExportFormat, ExportSummary, JobId, JobSnapshot,
    JobState, LogLevel, MigrateOptions, MigrateReport, PinPromptKind, PinRequest, PinRequestId, ReceiveAsk,
    ReceiveOptions, ReceiveSession, RecordQuery, ScanOptions, SelftestCheck, SelftestReport, SendItem, Stats,
};
pub use event::Event;
pub use mock::{MockBackend, MockCall};

/// 契约版本；`bt-api` 的不兼容改动必须同时递增该值并在架构文档记录。
pub const CONTRACT_VERSION: u32 = 1;
