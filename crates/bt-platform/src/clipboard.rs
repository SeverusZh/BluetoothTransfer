use std::path::PathBuf;

use bt_core::BtError;

/// 剪贴板内容：文件列表或纯文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardContent {
    Paths(Vec<PathBuf>),
    Text(String),
}

/// 剪贴板读写；文件复制需要平台支持 CF_HDROP 或 text/uri-list。
pub trait Clipboard: Send + Sync {
    fn set(&self, content: ClipboardContent) -> Result<(), BtError>;

    fn get_text(&self) -> Result<Option<String>, BtError>;
}

/// 未接入平台时的占位实现。
#[derive(Debug, Default)]
pub struct NoopClipboard;

impl Clipboard for NoopClipboard {
    fn set(&self, _content: ClipboardContent) -> Result<(), BtError> {
        Err(crate::unsupported("剪贴板写入"))
    }

    fn get_text(&self) -> Result<Option<String>, BtError> {
        Err(crate::unsupported("剪贴板读取"))
    }
}

pub fn platform_clipboard() -> Box<dyn Clipboard> {
    Box::new(NoopClipboard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_clipboard_reports_unsupported() {
        let clipboard = platform_clipboard();
        assert!(clipboard.get_text().is_err());
        let error = clipboard.set(ClipboardContent::Text("x".into())).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::Unsupported);
    }
}
