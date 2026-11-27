use std::path::PathBuf;

use bt_core::{AssistantState, Config, DeviceId};

use crate::dto::{AskId, Device, JobId, JobSnapshot, LogLevel, PinPromptKind, PinRequest};

/// 后端事件。GUI 每帧轮询，CLI 阻塞带超时，二者共用同一路类型。
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Log {
        level: LogLevel,
        message: String,
        at: String,
    },
    ScanStarted,
    ScanFinished {
        count: usize,
    },
    DeviceList(Vec<Device>),
    DeviceUpdated(Device),
    PairPrompt {
        request: PinRequest,
        device: Device,
        kind: PinPromptKind,
    },
    PairResult {
        device_id: DeviceId,
        ok: bool,
        error: Option<String>,
    },
    AssistantStateChanged {
        device_id: DeviceId,
        state: AssistantState,
    },
    JobAdded {
        job: JobSnapshot,
    },
    JobProgress {
        job_id: JobId,
        bytes: u64,
        total: u64,
        bps: u64,
        eta_seconds: Option<u64>,
    },
    JobStateChanged {
        job: JobSnapshot,
    },
    QueueIdle,
    ReceiveListeningChanged {
        listening: bool,
    },
    ReceiveProgress {
        bytes: u64,
        total: Option<u64>,
        name: String,
    },
    ReceiveAsk {
        id: AskId,
        name: String,
        size: u64,
    },
    ReceiveDone {
        ok: bool,
        path: Option<PathBuf>,
        error: Option<String>,
    },
    ConfigChanged {
        config: Config,
    },
    RecordsChanged,
}

impl Event {
    pub fn log(level: LogLevel, message: impl Into<String>) -> Self {
        Self::Log {
            level,
            message: message.into(),
            at: bt_core::now_rfc3339(),
        }
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self::log(LogLevel::Info, message)
    }

    pub fn warn(message: impl Into<String>) -> Self {
        Self::log(LogLevel::Warn, message)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::log(LogLevel::Error, message)
    }

    /// 是否为高频进度事件，界面可据此合并重绘。
    pub fn is_progress(&self) -> bool {
        matches!(
            self,
            Self::JobProgress { .. } | Self::ReceiveProgress { .. } | Self::ScanStarted
        )
    }
}
