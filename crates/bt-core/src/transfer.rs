use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 传输方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Send,
    Receive,
}

impl Direction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Receive => "receive",
        }
    }
}

/// 实际使用的传输通道：标准 OPP 或私有助手通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Opp,
    Assistant,
}

impl Channel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opp => "opp",
            Self::Assistant => "assistant",
        }
    }
}

/// 对端助手通道的探测状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssistantState {
    #[default]
    Unknown,
    Probing,
    Available,
    Unavailable,
}

/// 记录内容形态，决定复制与导出的行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    File,
    MultiFile,
    Folder,
    Text,
    Other,
}

impl RecordKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::MultiFile => "multi_file",
            Self::Folder => "folder",
            Self::Text => "text",
            Self::Other => "other",
        }
    }
}

/// 记录状态，与界面筛选一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    Running,
    Completed,
    Failed,
    Canceled,
    Interrupted,
}

impl RecordStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Interrupted => "interrupted",
        }
    }

    /// 是否属于终态，终态记录才参与统计与导出。
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// 发送来源，覆盖单文件、多文件、文件夹与文本四类入口。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum SendSource {
    File(PathBuf),
    Files(Vec<PathBuf>),
    Folder(PathBuf),
    Text(String),
}

impl SendSource {
    /// 用于界面与记录展示的名称。
    pub fn display_name(&self) -> String {
        match self {
            Self::File(path) | Self::Folder(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            Self::Files(paths) => match paths.first() {
                Some(path) => {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.display().to_string());
                    format!("{name} 等 {} 个文件", paths.len())
                }
                None => "空文件列表".to_string(),
            },
            Self::Text(text) => {
                let chars: String = text.chars().take(16).collect();
                chars
            }
        }
    }
}
