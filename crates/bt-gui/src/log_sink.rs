//! 日志：内存环形缓冲加热路径文件，支持级别过滤与诊断导出。

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use bt_api::LogLevel;
use bt_core::{now_rfc3339, BtError};

const MAX_ENTRIES: usize = 3000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub at: String,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Default)]
pub struct LogStore {
    entries: VecDeque<LogEntry>,
    writer: Option<BufWriter<std::fs::File>>,
    pub path: Option<PathBuf>,
    pub write_error: Option<String>,
    pending_writes: usize,
}

impl LogStore {
    /// 打开当日日志文件；失败时只在界面提示，不影响应用运行。
    pub fn open(directory: &Path) -> Self {
        let mut store = Self::default();
        if std::fs::create_dir_all(directory).is_err() {
            store.write_error = Some(format!("无法创建日志目录：{}", directory.display()));
            return store;
        }
        let date: String = now_rfc3339().chars().take(10).collect();
        let path = directory.join(format!("bt-gui-{date}.log"));
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                store.writer = Some(BufWriter::new(file));
                store.path = Some(path);
            }
            Err(error) => {
                store.write_error = Some(format!("无法打开日志文件 {}：{error}", path.display()));
            }
        }
        store
    }

    pub fn push(&mut self, level: LogLevel, message: impl Into<String>, at: Option<String>) {
        let entry = LogEntry {
            at: at.unwrap_or_else(now_rfc3339),
            level,
            message: message.into(),
        };
        if let Some(writer) = self.writer.as_mut() {
            let line = format!("{} {} {}\n", entry.at, level.as_str(), entry.message);
            if writer.write_all(line.as_bytes()).is_err() {
                self.write_error = Some("写入日志文件失败，后续日志仅保留在内存".to_string());
                self.writer = None;
            } else {
                // 日志是故障诊断的唯一落盘路径，逐条刷新避免进程异常终止时丢失。
                let _ = writer.flush();
                self.pending_writes = 0;
            }
        }
        self.entries.push_back(entry);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.push(LogLevel::Info, message, None);
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.push(LogLevel::Warn, message, None);
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.push(LogLevel::Error, message, None);
    }

    pub fn entries(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    pub fn filtered(&self, min_level: LogLevel) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter().filter(move |entry| entry.level >= min_level)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn flush(&mut self) {
        if let Some(writer) = self.writer.as_mut() {
            let _ = writer.flush();
            self.pending_writes = 0;
        }
    }

    /// 诊断文本：版本、平台、目录、日志与内存中的全部记录。
    pub fn diagnostics(&self, extra: &[(&str, String)]) -> String {
        let mut text = String::new();
        text.push_str("蓝牙传输诊断信息\n");
        text.push_str(&format!("版本 {}\n", env!("CARGO_PKG_VERSION")));
        text.push_str(&format!("系统 {} {}\n", std::env::consts::OS, std::env::consts::ARCH));
        for (key, value) in extra {
            text.push_str(&format!("{key} {value}\n"));
        }
        text.push_str(&format!(
            "日志文件 {}\n",
            self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
        ));
        text.push_str("\n最近日志\n");
        for entry in &self.entries {
            text.push_str(&format!("{} {} {}\n", entry.at, entry.level.as_str(), entry.message));
        }
        text
    }

    pub fn save_diagnostics(&self, path: &Path, extra: &[(&str, String)]) -> Result<u64, BtError> {
        let text = self.diagnostics(extra);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, &text)?;
        Ok(text.len() as u64)
    }
}

impl Drop for LogStore {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_by_level_and_keeps_order() {
        let mut store = LogStore::default();
        store.info("一");
        store.warn("二");
        store.error("三");
        let messages: Vec<_> = store
            .filtered(LogLevel::Warn)
            .map(|entry| entry.message.as_str())
            .collect();
        assert_eq!(messages, vec!["二", "三"]);
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn writes_log_file() {
        let dir = std::env::temp_dir().join(format!("bt-gui-log-{}", std::process::id()));
        let mut store = LogStore::open(&dir);
        store.info("hello");
        store.flush();
        let path = store.path.clone().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("hello"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
