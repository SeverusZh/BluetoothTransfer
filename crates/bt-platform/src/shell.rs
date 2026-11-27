//! Shell 集成与外部发送请求解析。
//!
//! 命令行契约（资源管理器右键菜单、SendTo 与二次启动转发共用）：
//! `bt-gui --send <路径>...`、`bt-gui --send-text <文本文件>`；
//! 路径含空格或中文时由调用方整体加双引号，系统已经拆分好的参数不再二次处理引号。

use std::path::{Path, PathBuf};

use bt_core::{BtError, SendSource};

use crate::inbox::{InboxItem, InboxItemKind};

/// 单次请求最多处理的项数。
pub const MAX_SEND_ITEMS: usize = 100;
/// `--send-text` 读取文本文件的上限。
pub const MAX_TEXT_BYTES: u64 = 256 * 1024;

/// 命令行里的发送项类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendKind {
    File,
    Folder,
    Text,
}

/// 解析后的单项发送请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequestItem {
    pub kind: SendKind,
    pub path: PathBuf,
    pub text: Option<String>,
}

/// 一组待发送项与可展示的警告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendRequest {
    pub items: Vec<SendRequestItem>,
    pub warnings: Vec<String>,
}

impl SendRequest {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 解析命令行参数；未知选项与非发送位置参数会被忽略并给出提示。
    pub fn parse_cli(args: &[String]) -> Self {
        let mut request = SendRequest::default();
        let mut index = 0usize;
        let mut limit_warned = false;
        while index < args.len() {
            let token = args[index].as_str();
            if token == "--send" {
                index += 1;
                let mut consumed = 0usize;
                while index < args.len() && !is_option(args[index].as_str()) {
                    if request.items.len() >= MAX_SEND_ITEMS {
                        if !limit_warned {
                            limit_warned = true;
                            request
                                .warnings
                                .push(format!("一次最多处理 {MAX_SEND_ITEMS} 项，其余项已忽略"));
                        }
                    } else {
                        consumed += 1;
                        add_path_item(&mut request, &args[index]);
                    }
                    index += 1;
                }
                if consumed == 0 && request.items.len() < MAX_SEND_ITEMS {
                    request.warnings.push("--send 后缺少路径参数，已忽略".to_string());
                }
                continue;
            }
            if token == "--send-text" {
                index += 1;
                if index >= args.len() || is_option(args[index].as_str()) {
                    request
                        .warnings
                        .push("--send-text 后缺少文本文件参数，已忽略".to_string());
                    continue;
                }
                if request.items.len() >= MAX_SEND_ITEMS {
                    if !limit_warned {
                        limit_warned = true;
                        request
                            .warnings
                            .push(format!("一次最多处理 {MAX_SEND_ITEMS} 项，其余项已忽略"));
                    }
                } else {
                    add_text_item(&mut request, &args[index]);
                }
                index += 1;
                continue;
            }
            if is_option(token) {
                request.warnings.push(format!("已忽略未知参数：{token}"));
            }
            index += 1;
        }
        request
    }

    /// 合并多组请求：按路径去重、总数截断并汇总警告。
    pub fn merge(requests: impl IntoIterator<Item = SendRequest>) -> Self {
        let mut merged = SendRequest::default();
        let mut seen = std::collections::HashSet::new();
        let mut limit_warned = false;
        for request in requests {
            merged.warnings.extend(request.warnings);
            for item in request.items {
                let key = item_key(&item);
                if !seen.insert(key) {
                    continue;
                }
                if merged.items.len() >= MAX_SEND_ITEMS {
                    if !limit_warned {
                        limit_warned = true;
                        merged
                            .warnings
                            .push(format!("一次最多处理 {MAX_SEND_ITEMS} 项，其余项已忽略"));
                    }
                    continue;
                }
                merged.items.push(item);
            }
        }
        merged
    }

    /// 转成协议层发送来源。
    pub fn to_sources(&self) -> Vec<SendSource> {
        self.items
            .iter()
            .map(|item| match item.kind {
                SendKind::File => SendSource::File(item.path.clone()),
                SendKind::Folder => SendSource::Folder(item.path.clone()),
                SendKind::Text => SendSource::Text(item.text.clone().unwrap_or_default()),
            })
            .collect()
    }

    pub fn to_inbox_items(&self) -> Vec<InboxItem> {
        self.items
            .iter()
            .map(|item| match item.kind {
                SendKind::File => InboxItem::file(item.path.clone()),
                SendKind::Folder => InboxItem::folder(item.path.clone()),
                SendKind::Text => InboxItem::text(item.text.clone().unwrap_or_default()),
            })
            .collect()
    }

    pub fn from_inbox(items: Vec<InboxItem>, warnings: Vec<String>) -> Self {
        Self {
            items: items
                .into_iter()
                .map(|item| SendRequestItem {
                    kind: match item.kind {
                        InboxItemKind::File => SendKind::File,
                        InboxItemKind::Folder => SendKind::Folder,
                        InboxItemKind::Text => SendKind::Text,
                    },
                    path: item.path,
                    text: item.text,
                })
                .collect(),
            warnings,
        }
    }
}

fn is_option(token: &str) -> bool {
    token.starts_with("--")
}

fn add_path_item(request: &mut SendRequest, raw: &str) {
    let trimmed = raw.trim().trim_matches('"').trim();
    if trimmed.is_empty() {
        request.warnings.push("已忽略空路径".to_string());
        return;
    }
    let path = match std::path::absolute(trimmed) {
        Ok(path) => path,
        Err(error) => {
            request.warnings.push(format!("已忽略无效路径「{raw}」：{error}"));
            return;
        }
    };
    if path.is_dir() {
        request.items.push(SendRequestItem {
            kind: SendKind::Folder,
            path,
            text: None,
        });
    } else if path.is_file() {
        request.items.push(SendRequestItem {
            kind: SendKind::File,
            path,
            text: None,
        });
    } else {
        request
            .warnings
            .push(format!("路径不存在或不可访问，已忽略：{}", path.display()));
    }
}

fn add_text_item(request: &mut SendRequest, raw: &str) {
    let trimmed = raw.trim().trim_matches('"').trim();
    let path = match std::path::absolute(trimmed) {
        Ok(path) => path,
        Err(error) => {
            request.warnings.push(format!("已忽略无效文本路径「{raw}」：{error}"));
            return;
        }
    };
    if !path.is_file() {
        request
            .warnings
            .push(format!("文本文件不存在或不可访问，已忽略：{}", path.display()));
        return;
    }
    match read_text_file(&path) {
        Ok(text) => request.items.push(SendRequestItem {
            kind: SendKind::Text,
            path,
            text: Some(text),
        }),
        Err(error) => {
            request
                .warnings
                .push(format!("读取文本失败（{error}），已改为按文件发送：{}", path.display()));
            request.items.push(SendRequestItem {
                kind: SendKind::File,
                path,
                text: None,
            });
        }
    }
}

fn item_key(item: &SendRequestItem) -> String {
    match item.kind {
        SendKind::Text => format!("text:{}", item.text.as_deref().unwrap_or_default()),
        _ => format!("path:{}", item.path.display()),
    }
}

/// 读取文本文件：支持 UTF-8/UTF-16 BOM，拒绝二进制与超限文件。
pub fn read_text_file(path: &Path) -> Result<String, BtError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_TEXT_BYTES {
        return Err(BtError::invalid(format!(
            "文本文件为 {} 字节，超过 {MAX_TEXT_BYTES} 字节上限",
            metadata.len()
        )));
    }
    let bytes = std::fs::read(path)?;
    let text = decode_text(&bytes)?;
    if text.contains('\0') {
        return Err(BtError::invalid("文件内容包含 NUL 字符，可能是二进制文件"));
    }
    if text.is_empty() {
        return Err(BtError::invalid("文本文件为空"));
    }
    Ok(text)
}

fn decode_text(bytes: &[u8]) -> Result<String, BtError> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec()).map_err(|_| BtError::invalid("文件不是有效的 UTF-8 文本"));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, true);
    }
    String::from_utf8(bytes.to_vec())
        .map_err(|_| BtError::invalid("文件不是有效的 UTF-8/UTF-16 文本（可能为二进制或其它编码）"))
}

fn decode_utf16(bytes: &[u8], big_endian: bool) -> Result<String, BtError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(BtError::invalid("UTF-16 文本字节数为奇数"));
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| {
            if big_endian {
                u16::from_be_bytes([pair[0], pair[1]])
            } else {
                u16::from_le_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16(&units).map_err(|_| BtError::invalid("UTF-16 文本包含非法字符"))
}

/// Shell 集成结果，摘要与警告都可直接展示。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellReport {
    pub summary: String,
    pub warnings: Vec<String>,
}

impl ShellReport {
    pub fn ok(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            warnings: Vec::new(),
        }
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}

/// 文件管理器右键菜单集成。
pub trait ShellIntegration: Send + Sync {
    fn install(&self) -> Result<ShellReport, BtError>;

    fn uninstall(&self) -> Result<ShellReport, BtError>;

    fn is_installed(&self) -> bool;

    /// 当前平台入口名称，用于设置页展示。
    fn label(&self) -> &'static str;
}

/// 按当前平台返回集成实现。
pub fn platform_shell() -> Box<dyn ShellIntegration> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxShell)
    }
    #[cfg(windows)]
    {
        Box::new(windows::WindowsShell)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Box::new(UnsupportedShell)
    }
}

/// 解析命令行中的发送参数；供 GUI 启动时调用。
pub fn send_request_from_args(args: &[String]) -> SendRequest {
    SendRequest::parse_cli(args)
}

/// 资源管理器入口当前可执行文件路径。
pub fn current_exe() -> Result<PathBuf, BtError> {
    std::env::current_exe().map_err(|error| BtError::internal(format!("无法确定程序路径：{error}")))
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

/// 其他平台：显式报错。
#[cfg(not(any(target_os = "linux", windows)))]
#[derive(Debug, Default)]
pub struct UnsupportedShell;

#[cfg(not(any(target_os = "linux", windows)))]
impl ShellIntegration for UnsupportedShell {
    fn install(&self) -> Result<ShellReport, BtError> {
        Err(crate::unsupported("文件管理器右键菜单集成"))
    }

    fn uninstall(&self) -> Result<ShellReport, BtError> {
        Err(crate::unsupported("文件管理器右键菜单集成"))
    }

    fn is_installed(&self) -> bool {
        false
    }

    fn label(&self) -> &'static str {
        "当前平台不支持文件管理器集成"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bt-platform-shell-{tag}-{}-{}",
            std::process::id(),
            bt_core::unix_now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_files_folders_and_text() {
        let dir = temp_dir("parse");
        let file = dir.join("带空格 的文件.txt");
        let folder = dir.join("子目录");
        std::fs::write(&file, "正文").unwrap();
        std::fs::create_dir(&folder).unwrap();
        let args = vec![
            "--send".to_string(),
            folder.display().to_string(),
            file.display().to_string(),
            "--send-text".to_string(),
            file.display().to_string(),
        ];
        let request = SendRequest::parse_cli(&args);
        assert_eq!(request.items.len(), 3);
        assert_eq!(request.items[0].kind, SendKind::Folder);
        assert_eq!(request.items[1].kind, SendKind::File);
        assert_eq!(request.items[2].kind, SendKind::Text);
        assert_eq!(request.items[2].text.as_deref(), Some("正文"));
        assert!(request.warnings.is_empty());
        let sources = request.to_sources();
        assert_eq!(sources.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_paths_become_warnings_not_items() {
        let request =
            SendRequest::parse_cli(&["--send".to_string(), "/nonexistent/blue-tooth-transfer-xyz".to_string()]);
        assert!(request.items.is_empty());
        assert_eq!(request.warnings.len(), 1);
    }

    #[test]
    fn merge_deduplicates_and_limits_items() {
        let dir = temp_dir("merge");
        let file = dir.join("a.bin");
        std::fs::write(&file, b"x").unwrap();
        let item = SendRequestItem {
            kind: SendKind::File,
            path: file.clone(),
            text: None,
        };
        let first = SendRequest {
            items: vec![item.clone()],
            warnings: vec!["w1".into()],
        };
        let second = SendRequest {
            items: vec![item],
            warnings: vec!["w2".into()],
        };
        let merged = SendRequest::merge([first, second]);
        assert_eq!(merged.items.len(), 1);
        assert_eq!(merged.warnings, vec!["w1", "w2"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_file_limits_and_binary_rejection() {
        let dir = temp_dir("text");
        let binary = dir.join("bin.dat");
        std::fs::write(&binary, [0u8, 1, 2, 3]).unwrap();
        let error = read_text_file(&binary).unwrap_err();
        assert!(error.message().contains("二进制") || error.message().contains("文本"));
        let utf16 = dir.join("utf16.txt");
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "中文".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&utf16, bytes).unwrap();
        assert_eq!(read_text_file(&utf16).unwrap(), "中文");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
