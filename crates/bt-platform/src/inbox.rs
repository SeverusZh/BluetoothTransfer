use std::path::{Path, PathBuf};

use bt_core::BtError;

/// 外部应用投递的待发送项，通常来自资源管理器右键菜单或拖放落点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItem {
    pub source: PathBuf,
    pub text: Option<String>,
}

impl InboxItem {
    pub fn file(path: PathBuf) -> Self {
        Self {
            source: path,
            text: None,
        }
    }

    pub fn text(text: String) -> Self {
        Self {
            source: PathBuf::new(),
            text: Some(text),
        }
    }
}

/// 收件箱目录：外部进程写入条目，主实例轮询取走。
#[derive(Debug, Clone)]
pub struct ExternalInbox {
    root: PathBuf,
}

impl ExternalInbox {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ensure(&self) -> Result<(), BtError> {
        std::fs::create_dir_all(&self.root)?;
        Ok(())
    }

    /// 取走全部条目；实现落在 T4。
    pub fn drain(&self) -> Result<Vec<InboxItem>, BtError> {
        Err(BtError::not_implemented("外部收件箱轮询需要平台实现，T4 完成"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_creates_inbox_directory() {
        let root = std::env::temp_dir().join("bt-platform-inbox");
        let inbox = ExternalInbox::new(root.clone());
        inbox.ensure().unwrap();
        assert!(root.is_dir());
        assert!(inbox.drain().is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
