//! 外部收件箱：二次实例或 Shell 集成把待发送项写成 JSON，主实例去抖后统一排空。
//!
//! 安全边界：单个载荷不超过 1MB、条目不超过 100、路径必须是绝对路径且存在；
//! 删除只针对本目录下的 `.json` 与 `.tmp` 文件，从不触碰用户文件。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use bt_core::{BtError, BtErrorKind};
use serde::{Deserialize, Serialize};

/// 单个载荷文件的上限。
pub const MAX_PAYLOAD_BYTES: u64 = 1024 * 1024;
/// 一次排空允许合并的最大条目数。
pub const MAX_ITEMS: usize = 100;
const PAYLOAD_VERSION: u32 = 1;
const JSON_EXTENSION: &str = "json";
const TEMP_EXTENSION: &str = "tmp";
/// 正常消费的载荷超过该时长仍未处理，视为崩溃残留并清理。
pub const RESIDUE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const TEMP_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// 收件箱条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxItemKind {
    File,
    Folder,
    Text,
}

/// 外部应用投递的待发送项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItem {
    pub kind: InboxItemKind,
    pub path: PathBuf,
    pub text: Option<String>,
}

impl InboxItem {
    pub fn file(path: PathBuf) -> Self {
        Self {
            kind: InboxItemKind::File,
            path,
            text: None,
        }
    }

    pub fn folder(path: PathBuf) -> Self {
        Self {
            kind: InboxItemKind::Folder,
            path,
            text: None,
        }
    }

    pub fn text(text: String) -> Self {
        Self {
            kind: InboxItemKind::Text,
            path: PathBuf::new(),
            text: Some(text),
        }
    }
}

/// 排空结果：有效条目与可展示的警告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InboxDrain {
    pub items: Vec<InboxItem>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Payload {
    version: u32,
    items: Vec<WireItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireItem {
    kind: InboxItemKind,
    #[serde(default, skip_serializing_if = "path_is_empty")]
    path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
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

    /// 原子写入一份载荷；只写入本目录。
    pub fn write(&self, items: &[InboxItem], warnings: &[String]) -> Result<(), BtError> {
        if items.is_empty() && warnings.is_empty() {
            return Err(BtError::invalid("待转发的发送请求为空"));
        }
        if items.len() > MAX_ITEMS {
            return Err(BtError::invalid(format!(
                "待转发条目 {} 项，超过上限 {MAX_ITEMS} 项",
                items.len()
            )));
        }
        let payload = Payload {
            version: PAYLOAD_VERSION,
            items: items
                .iter()
                .map(|item| WireItem {
                    kind: item.kind,
                    path: absolute_path(&item.path).unwrap_or_else(|_| item.path.clone()),
                    text: item.text.clone(),
                })
                .collect(),
            warnings: warnings.to_vec(),
        };
        let bytes =
            serde_json::to_vec(&payload).map_err(|error| BtError::internal(format!("序列化发送请求失败：{error}")))?;
        if bytes.len() as u64 > MAX_PAYLOAD_BYTES {
            return Err(BtError::invalid(format!(
                "待转发数据 {} 字节，超过 {MAX_PAYLOAD_BYTES} 字节上限",
                bytes.len()
            )));
        }
        self.ensure()?;
        let final_path = self.root.join(format!("{}.{}", unique_stem(), JSON_EXTENSION));
        let temp_path = self.root.join(format!("{}.{}", unique_stem(), TEMP_EXTENSION));
        std::fs::write(&temp_path, &bytes)?;
        if let Err(error) = std::fs::rename(&temp_path, &final_path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(error.into());
        }
        Ok(())
    }

    /// 读出全部载荷并删除已消费文件；单个坏文件只产生警告，不中断其余条目。
    pub fn drain(&self) -> Result<InboxDrain, BtError> {
        let mut drain = InboxDrain::default();
        if !self.root.is_dir() {
            return Ok(drain);
        }
        let mut files = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case(JSON_EXTENSION))
                })
                .collect::<Vec<_>>(),
            Err(error) => {
                return Err(BtError::new(
                    BtErrorKind::Io,
                    format!("读取发送请求收件箱失败：{error}"),
                ))
            }
        };
        files.sort_by_key(|path| file_modified(path).unwrap_or(SystemTime::UNIX_EPOCH));
        let mut seen = std::collections::HashSet::new();
        let mut limit_warned = false;
        for file in files {
            let outcome = self.read_payload(&file);
            drain
                .warnings
                .extend(outcome.warnings.into_iter().filter(|warning| !warning.is_empty()));
            for item in outcome.items {
                if drain.items.len() >= MAX_ITEMS {
                    if !limit_warned {
                        limit_warned = true;
                        drain
                            .warnings
                            .push(format!("一次最多合并 {MAX_ITEMS} 项，其余项已忽略"));
                    }
                    break;
                }
                let key = item_key(&item);
                if seen.insert(key) {
                    drain.items.push(item);
                }
            }
            self.delete_owned(&file);
        }
        Ok(drain)
    }

    fn read_payload(&self, file: &Path) -> InboxDrain {
        let mut drain = InboxDrain::default();
        let name = file
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let metadata = match std::fs::metadata(file) {
            Ok(metadata) => metadata,
            Err(error) => {
                drain
                    .warnings
                    .push(format!("读取收件箱文件失败，已忽略 {name}：{error}"));
                return drain;
            }
        };
        if metadata.len() > MAX_PAYLOAD_BYTES {
            drain.warnings.push(format!(
                "收件箱请求 {name} 为 {} 字节，超过 {MAX_PAYLOAD_BYTES} 字节上限，已丢弃",
                metadata.len()
            ));
            return drain;
        }
        let bytes = match std::fs::read(file) {
            Ok(bytes) => bytes,
            Err(error) => {
                drain
                    .warnings
                    .push(format!("读取收件箱文件失败，已忽略 {name}：{error}"));
                return drain;
            }
        };
        let payload: Payload = match serde_json::from_slice(&bytes) {
            Ok(payload) => payload,
            Err(error) => {
                drain
                    .warnings
                    .push(format!("收件箱请求 {name} 解析失败，已丢弃：{error}"));
                return drain;
            }
        };
        if payload.version != PAYLOAD_VERSION {
            drain
                .warnings
                .push(format!("收件箱请求 {name} 版本为 {}，按兼容方式读取", payload.version));
        }
        drain.warnings.extend(payload.warnings);
        let mut count = 0usize;
        for (index, wire) in payload.items.into_iter().enumerate() {
            if count >= MAX_ITEMS {
                drain
                    .warnings
                    .push(format!("收件箱请求 {name} 条目超过 {MAX_ITEMS} 项，其余项已忽略"));
                break;
            }
            match validate_wire(wire) {
                Ok(item) => {
                    count += 1;
                    drain.items.push(item);
                }
                Err(warning) => drain
                    .warnings
                    .push(format!("收件箱请求 {name} 第 {} 项无效：{warning}", index + 1)),
            }
        }
        drain
    }

    /// 清理过期残留：JSON 默认 7 天，中断写入的临时文件 1 小时。
    pub fn clean_residue(&self, older_than: Duration) -> Result<usize, BtError> {
        if !self.root.is_dir() {
            return Ok(0);
        }
        let now = SystemTime::now();
        let mut removed = 0usize;
        for entry in std::fs::read_dir(&self.root)? {
            let Ok(entry) = entry else {
                continue;
            };
            let path = entry.path();
            let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
                continue;
            };
            let max_age = if extension.eq_ignore_ascii_case(JSON_EXTENSION) {
                older_than.max(Duration::from_secs(1))
            } else if extension.eq_ignore_ascii_case(TEMP_EXTENSION) {
                older_than.min(TEMP_MAX_AGE)
            } else {
                continue;
            };
            let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
                continue;
            };
            let age = now.duration_since(modified).unwrap_or(Duration::ZERO);
            if age < max_age {
                continue;
            }
            if self.delete_owned(&path) {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// 只删除本目录内的普通文件；目录外路径一律拒绝。
    fn delete_owned(&self, path: &Path) -> bool {
        let Ok(root) = std::fs::canonicalize(&self.root) else {
            return false;
        };
        let Ok(full) = std::fs::canonicalize(path) else {
            return false;
        };
        if !full.starts_with(&root) || full == root {
            return false;
        }
        let is_own_extension = full.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case(JSON_EXTENSION) || extension.eq_ignore_ascii_case(TEMP_EXTENSION)
        });
        if !is_own_extension {
            return false;
        }
        std::fs::remove_file(full).is_ok()
    }
}

fn validate_wire(wire: WireItem) -> Result<InboxItem, String> {
    match wire.kind {
        InboxItemKind::Text => {
            let text = wire.text.unwrap_or_default();
            if text.is_empty() {
                return Err("文本内容为空".to_string());
            }
            Ok(InboxItem::text(text))
        }
        kind => {
            if wire.path.as_os_str().is_empty() {
                return Err("路径为空".to_string());
            }
            let path = absolute_path(&wire.path).map_err(|error| error.to_string())?;
            let exists = match kind {
                InboxItemKind::File => path.is_file(),
                InboxItemKind::Folder => path.is_dir(),
                InboxItemKind::Text => unreachable!(),
            };
            if !exists {
                return Err(format!("路径不存在或类型不匹配：{}", path.display()));
            }
            Ok(InboxItem { kind, path, text: None })
        }
    }
}

fn item_key(item: &InboxItem) -> String {
    match item.kind {
        InboxItemKind::Text => format!("text:{}", item.text.as_deref().unwrap_or_default()),
        _ => format!("path:{}", item.path.display()),
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, BtError> {
    if path.as_os_str().is_empty() {
        return Err(BtError::invalid("路径为空"));
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    // 收件箱写入方总是传绝对路径；相对路径视为非法输入，避免依赖主实例的当前目录。
    Err(BtError::invalid(format!("路径必须是绝对路径：{}", path.display())))
}

#[allow(clippy::ptr_arg)] // serde 的 skip_serializing_if 要求与字段同类型。
fn path_is_empty(path: &PathBuf) -> bool {
    path.as_os_str().is_empty()
}

fn file_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn unique_stem() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}-{sequence}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bt-platform-inbox-{tag}-{}-{}",
            std::process::id(),
            unique_stem()
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn write_then_drain_round_trips_files_and_text() {
        let root = temp_root("roundtrip");
        let inbox = ExternalInbox::new(root.clone());
        let file = root.join("note.txt");
        inbox.ensure().unwrap();
        std::fs::write(&file, b"hello").unwrap();
        let items = vec![InboxItem::file(file.clone()), InboxItem::text("正文".into())];
        inbox.write(&items, &["提示".into()]).unwrap();
        let drain = inbox.drain().unwrap();
        assert_eq!(drain.items, items);
        assert_eq!(drain.warnings, vec!["提示".to_string()]);
        let remaining: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|extension| extension == JSON_EXTENSION))
            .collect();
        assert!(remaining.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_relative_and_missing_paths_but_keeps_other_items() {
        let root = temp_root("validation");
        let inbox = ExternalInbox::new(root.clone());
        let valid = root.join("valid.bin");
        inbox.ensure().unwrap();
        std::fs::write(&valid, b"x").unwrap();
        inbox
            .write(
                &[
                    InboxItem::file(valid.clone()),
                    InboxItem::file(PathBuf::from("relative.txt")),
                    InboxItem::file(root.join("missing.bin")),
                ],
                &[],
            )
            .unwrap();
        let drain = inbox.drain().unwrap();
        assert_eq!(drain.items.len(), 1);
        assert_eq!(drain.items[0].path, valid);
        assert_eq!(drain.warnings.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn oversized_payloads_are_discarded_with_warning() {
        let root = temp_root("oversize");
        let inbox = ExternalInbox::new(root.clone());
        inbox.ensure().unwrap();
        let path = root.join("big.json");
        std::fs::write(&path, vec![b'x'; (MAX_PAYLOAD_BYTES + 1) as usize]).unwrap();
        let drain = inbox.drain().unwrap();
        assert!(drain.items.is_empty());
        assert!(drain.warnings.iter().any(|warning| warning.contains("超过")));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn residue_cleanup_only_touches_own_directory() {
        let root = temp_root("residue");
        let inbox = ExternalInbox::new(root.clone());
        inbox.ensure().unwrap();
        let stale_temp = root.join("stale.tmp");
        std::fs::write(&stale_temp, b"x").unwrap();
        let outside = root.parent().unwrap().join(format!("outside-{}.json", unique_stem()));
        std::fs::write(&outside, b"{}").unwrap();
        let removed = inbox.clean_residue(Duration::ZERO).unwrap();
        assert_eq!(removed, 1);
        assert!(!stale_temp.exists());
        assert!(outside.exists());
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn malformed_json_is_deleted_with_warning() {
        let root = temp_root("malformed");
        let inbox = ExternalInbox::new(root.clone());
        inbox.ensure().unwrap();
        let path = root.join("bad.json");
        std::fs::write(&path, b"not json").unwrap();
        let drain = inbox.drain().unwrap();
        assert!(drain.items.is_empty());
        assert_eq!(drain.warnings.len(), 1);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
