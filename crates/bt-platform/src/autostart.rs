use std::path::PathBuf;

use bt_core::BtError;

/// 自启项：可执行文件加启动参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartEntry {
    pub command: PathBuf,
    pub args: Vec<String>,
}

impl AutostartEntry {
    pub fn new(command: PathBuf) -> Self {
        Self {
            command,
            args: Vec::new(),
        }
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }
}

/// 开机自启管理。
pub trait Autostart: Send + Sync {
    fn enable(&self, entry: &AutostartEntry) -> Result<(), BtError>;

    fn disable(&self) -> Result<(), BtError>;

    fn is_enabled(&self) -> bool;
}

/// 未接入平台时的占位实现。
#[derive(Debug, Default)]
pub struct NoopAutostart;

impl Autostart for NoopAutostart {
    fn enable(&self, _entry: &AutostartEntry) -> Result<(), BtError> {
        Err(crate::unsupported("开机自启"))
    }

    fn disable(&self) -> Result<(), BtError> {
        Err(crate::unsupported("开机自启"))
    }

    fn is_enabled(&self) -> bool {
        false
    }
}

pub fn platform_autostart() -> Box<dyn Autostart> {
    Box::new(NoopAutostart)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_autostart_reports_unsupported() {
        let autostart = platform_autostart();
        assert!(!autostart.is_enabled());
        let entry = AutostartEntry::new(PathBuf::from("/usr/bin/bt"));
        assert!(autostart.enable(&entry).is_err());
    }
}
