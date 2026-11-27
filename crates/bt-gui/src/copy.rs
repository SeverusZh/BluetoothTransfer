//! 记录复制：把后端复制计划落到剪贴板，按记录类型选择文本、图片或文件列表。

use std::path::{Path, PathBuf};

use bt_api::{CopyItem, CopyPlan};
use bt_core::{BtError, Record, RecordKind};

const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;

const IMAGE_EXTENSIONS: [&str; 9] = ["png", "jpg", "jpeg", "bmp", "gif", "webp", "ico", "tif", "tiff"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyOutcome {
    Success(String),
    Failure(String),
}

/// 执行复制计划；失败时返回可直接展示的原因，不静默。
pub fn apply_copy_plan(plan: &CopyPlan, record: &Record) -> CopyOutcome {
    if plan.is_empty() {
        return CopyOutcome::Failure("没有可复制的内容".to_string());
    }
    let paths: Vec<PathBuf> = plan
        .items
        .iter()
        .filter_map(|item| match item {
            CopyItem::Path(path) => Some(path.clone()),
            _ => None,
        })
        .collect();
    let texts: Vec<String> = plan
        .items
        .iter()
        .filter_map(|item| match item {
            CopyItem::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    let missing: Vec<PathBuf> = plan
        .items
        .iter()
        .filter_map(|item| match item {
            CopyItem::Missing(path) => Some(path.clone()),
            _ => None,
        })
        .collect();

    if record.kind == RecordKind::Text {
        if let Some(content) = record.content.as_deref() {
            if !content.is_empty() {
                return match set(bt_platform::ClipboardContent::Text(content.to_string())) {
                    Ok(()) => CopyOutcome::Success("已复制文本内容".to_string()),
                    Err(error) => CopyOutcome::Failure(error.message().to_string()),
                };
            }
        }
    }

    if record.kind == RecordKind::Text && !paths.is_empty() {
        return match read_text(&paths[0]) {
            Ok(text) => match set(bt_platform::ClipboardContent::Text(text)) {
                Ok(()) => CopyOutcome::Success("已复制文本内容".to_string()),
                Err(error) => CopyOutcome::Failure(error.message().to_string()),
            },
            Err(error) => CopyOutcome::Failure(error.message().to_string()),
        };
    }

    if paths.len() == 1 {
        let path = &paths[0];
        if is_image(path) {
            match bt_platform::clipboard::read_image_file(path) {
                Ok(image) => {
                    return match set(bt_platform::ClipboardContent::Image(image)) {
                        Ok(()) => CopyOutcome::Success("已复制图片".to_string()),
                        Err(error) => CopyOutcome::Failure(error.message().to_string()),
                    };
                }
                Err(error) => {
                    let fallback = set(bt_platform::ClipboardContent::Paths(vec![path.clone()]));
                    return match fallback {
                        Ok(()) => CopyOutcome::Success(format!("图片无法解码（{}），已改为复制文件", error.message())),
                        Err(error) => CopyOutcome::Failure(error.message().to_string()),
                    };
                }
            }
        }
    }

    if !paths.is_empty() {
        let existing: Vec<PathBuf> = paths.iter().filter(|path| path.exists()).cloned().collect();
        if existing.is_empty() {
            return CopyOutcome::Failure(format!(
                "本地文件已不存在：{}",
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        return match set(bt_platform::ClipboardContent::Paths(existing.clone())) {
            Ok(()) => {
                let mut message = format!("已复制 {} 个文件", existing.len());
                if !missing.is_empty() {
                    message.push_str(&format!("；{} 个文件已不存在，未加入剪贴板", missing.len()));
                }
                CopyOutcome::Success(message)
            }
            Err(error) => CopyOutcome::Failure(error.message().to_string()),
        };
    }

    if !texts.is_empty() {
        let text = texts.join("\n");
        return match set(bt_platform::ClipboardContent::Text(text)) {
            Ok(()) => CopyOutcome::Success("已复制文本".to_string()),
            Err(error) => CopyOutcome::Failure(error.message().to_string()),
        };
    }

    if !missing.is_empty() {
        let message = if record.kind == RecordKind::Text {
            format!("文本记录未保存正文且本地文件已不存在：{}", missing[0].display())
        } else {
            format!("本地文件已不存在：{}", missing[0].display())
        };
        return CopyOutcome::Failure(message);
    }
    CopyOutcome::Failure("该记录没有可复制的内容".to_string())
}

fn set(content: bt_platform::ClipboardContent) -> Result<(), BtError> {
    bt_platform::platform_clipboard().set(content)
}

fn read_text(path: &Path) -> Result<String, BtError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_TEXT_BYTES {
        return Err(BtError::invalid(format!(
            "文本文件为 {} 字节，超过 8MB 复制上限",
            metadata.len()
        )));
    }
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::{Channel, Direction, RecordKind};

    fn record(kind: RecordKind) -> Record {
        let mut record = Record::running(Direction::Send, Channel::Opp, kind, "x", "2026-01-01T00:00:00Z");
        record.status = bt_core::RecordStatus::Completed;
        record
    }

    #[test]
    fn missing_paths_produce_actionable_failure() {
        let plan = CopyPlan {
            items: vec![CopyItem::Missing(PathBuf::from("/nonexistent/bt-copy/x.txt"))],
            kind: Some(bt_api::CopyKind::Files),
        };
        let outcome = apply_copy_plan(&plan, &record(RecordKind::File));
        match outcome {
            CopyOutcome::Failure(message) => assert!(message.contains("不存在")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn image_extension_is_detected_case_insensitively() {
        assert!(is_image(Path::new("a.PNG")));
        assert!(!is_image(Path::new("a.txt")));
    }
}
