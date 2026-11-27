use bt_core::{BtError, BtErrorKind};

use crate::output::OutputMode;

pub const EXIT_OK: u8 = 0;
pub const EXIT_FAILED: u8 = 1;
pub const EXIT_USAGE: u8 = 2;
pub const EXIT_RUNNING: u8 = 3;
pub const EXIT_PERMISSION: u8 = 4;
pub const EXIT_NOT_FOUND: u8 = 5;

/// 命令级错误：带分类、可读说明与可操作的下一步提示。
#[derive(Debug, Clone)]
pub struct CliError {
    kind: BtErrorKind,
    message: String,
    hint: Option<String>,
}

impl CliError {
    pub fn new(kind: BtErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: None,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::InvalidArgument, message)
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Internal, message)
    }

    pub fn canceled(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Canceled, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Unsupported, message)
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn kind(&self) -> BtErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn exit_code(&self) -> u8 {
        exit_code_for(self.kind)
    }

    fn hint_text(&self) -> Option<&str> {
        self.hint.as_deref().or_else(|| default_hint(self.kind))
    }

    /// 输出错误并按分类返回退出码；JSON 模式写入 stdout 的单个对象，人类模式写 stderr。
    pub fn report(&self, mode: OutputMode, command: &str) -> u8 {
        let hint = self.hint_text();
        match mode {
            OutputMode::Human => {
                eprintln!("错误：{}", self.message);
                if let Some(hint) = hint {
                    eprintln!("提示：{hint}");
                }
            }
            OutputMode::Json => {
                let mut body = serde_json::json!({
                    "ok": false,
                    "command": command,
                    "error": {
                        "kind": self.kind.as_str(),
                        "message": self.message,
                        "hint": hint,
                    }
                });
                if let Some(object) = body.as_object_mut() {
                    object.insert("type".to_string(), serde_json::json!("error"));
                }
                println!("{body}");
            }
            OutputMode::Events => {
                let body = serde_json::json!({
                    "type": "error",
                    "ok": false,
                    "command": command,
                    "error": {
                        "kind": self.kind.as_str(),
                        "message": self.message,
                        "hint": hint,
                    }
                });
                println!("{body}");
            }
        }
        self.exit_code()
    }
}

impl From<BtError> for CliError {
    fn from(error: BtError) -> Self {
        Self {
            kind: error.kind(),
            message: error.message().to_string(),
            hint: None,
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        let kind = match error.kind() {
            std::io::ErrorKind::PermissionDenied => BtErrorKind::Permission,
            _ => BtErrorKind::Io,
        };
        Self::new(kind, format!("读写失败：{error}"))
    }
}

/// 错误分类到退出码的唯一映射点。
pub fn exit_code_for(kind: BtErrorKind) -> u8 {
    match kind {
        BtErrorKind::InvalidArgument => EXIT_USAGE,
        BtErrorKind::NotFound => EXIT_NOT_FOUND,
        BtErrorKind::Permission | BtErrorKind::Locked => EXIT_PERMISSION,
        BtErrorKind::Busy => EXIT_RUNNING,
        BtErrorKind::Unsupported
        | BtErrorKind::NotImplemented
        | BtErrorKind::Io
        | BtErrorKind::Protocol
        | BtErrorKind::Timeout
        | BtErrorKind::Canceled
        | BtErrorKind::Storage
        | BtErrorKind::Internal => EXIT_FAILED,
    }
}

fn default_hint(kind: BtErrorKind) -> Option<&'static str> {
    match kind {
        BtErrorKind::InvalidArgument => Some("运行 bt --help 或对应子命令的 --help 查看用法"),
        BtErrorKind::NotFound => {
            Some("先用 bt devices scan 或 bt devices list 确认设备；记录编号用 bt records list 查看")
        }
        BtErrorKind::Permission => Some("检查配置、数据、接收目录权限；不要用 sudo 运行图形界面"),
        BtErrorKind::Locked => Some("另一进程正持有配置、journal 或单实例锁；稍后重试"),
        BtErrorKind::Busy => Some("已有实例或任务占用；等待任务结束或停止旧实例后重试"),
        BtErrorKind::Unsupported | BtErrorKind::NotImplemented => {
            Some("当前平台或后端不支持该能力；运行 bt selftest 查看后端状态")
        }
        BtErrorKind::Timeout => Some("确认设备在线且已配对，或调大 opp_connect_timeout_s 与 opp_send_idle_timeout_s"),
        BtErrorKind::Canceled => Some("发送取消后 journal 会保留，可用 bt resume list 查看"),
        BtErrorKind::Io | BtErrorKind::Storage => Some("检查磁盘空间与数据目录权限"),
        BtErrorKind::Protocol => Some("确认对端运行本应用接收端；OPP 与助手通道不互通"),
        BtErrorKind::Internal => Some("运行 bt selftest 收集诊断信息"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_contract() {
        assert_eq!(CliError::usage("x").exit_code(), EXIT_USAGE);
        assert_eq!(CliError::new(BtErrorKind::NotFound, "x").exit_code(), EXIT_NOT_FOUND);
        assert_eq!(CliError::new(BtErrorKind::Permission, "x").exit_code(), EXIT_PERMISSION);
        assert_eq!(CliError::new(BtErrorKind::Locked, "x").exit_code(), EXIT_PERMISSION);
        assert_eq!(CliError::new(BtErrorKind::Busy, "x").exit_code(), EXIT_RUNNING);
        assert_eq!(CliError::failed("x").exit_code(), EXIT_FAILED);
        assert_eq!(CliError::canceled("x").exit_code(), EXIT_FAILED);
        assert_eq!(CliError::unsupported("x").exit_code(), EXIT_FAILED);
    }

    #[test]
    fn every_error_kind_maps_to_a_single_exit_code() {
        let expected = [
            (BtErrorKind::InvalidArgument, EXIT_USAGE),
            (BtErrorKind::NotFound, EXIT_NOT_FOUND),
            (BtErrorKind::Permission, EXIT_PERMISSION),
            (BtErrorKind::Locked, EXIT_PERMISSION),
            (BtErrorKind::Busy, EXIT_RUNNING),
            (BtErrorKind::Unsupported, EXIT_FAILED),
            (BtErrorKind::NotImplemented, EXIT_FAILED),
            (BtErrorKind::Io, EXIT_FAILED),
            (BtErrorKind::Protocol, EXIT_FAILED),
            (BtErrorKind::Timeout, EXIT_FAILED),
            (BtErrorKind::Canceled, EXIT_FAILED),
            (BtErrorKind::Storage, EXIT_FAILED),
            (BtErrorKind::Internal, EXIT_FAILED),
        ];
        for (kind, code) in expected {
            assert_eq!(exit_code_for(kind), code, "{kind}");
            assert_eq!(CliError::new(kind, "x").exit_code(), code);
        }
    }

    #[test]
    fn io_permission_denied_maps_to_permission() {
        let error = CliError::from(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "拒绝"));
        assert_eq!(error.kind(), BtErrorKind::Permission);
        assert_eq!(error.exit_code(), EXIT_PERMISSION);
    }
}
