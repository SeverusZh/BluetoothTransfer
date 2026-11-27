use std::path::{Component, Path, PathBuf};

use crate::error::BtError;

const MAX_FILE_NAME_CHARS: usize = 180;

const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 把任意来源的文件名清洗成可落盘的单段名称，用于接收端防穿越。
pub fn sanitize_file_name(input: &str) -> String {
    let base = input.rsplit(['/', '\\']).next().unwrap_or(input);
    let mut cleaned = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_control() {
            continue;
        }
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => cleaned.push('_'),
            _ => cleaned.push(ch),
        }
    }
    let trimmed = cleaned.trim().trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        return "unnamed".to_string();
    }
    let mut name = trimmed.to_string();
    let stem = name.split('.').next().unwrap_or_default().to_ascii_uppercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        name.push('_');
    }
    if name.chars().count() > MAX_FILE_NAME_CHARS {
        name = name.chars().take(MAX_FILE_NAME_CHARS).collect();
    }
    name
}

/// 纯词法归一化：消去 `.`，折叠 `..`，不访问文件系统。
pub fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 确认候选路径仍位于根目录之内，防止 `..` 与绝对路径逃逸。
pub fn ensure_within(root: &Path, candidate: &Path) -> Result<PathBuf, BtError> {
    let normalized_root = normalize_lexically(root);
    let normalized_candidate = normalize_lexically(candidate);
    if !normalized_candidate.starts_with(&normalized_root) {
        return Err(BtError::permission(format!(
            "目标路径越过根目录：{}",
            candidate.display()
        )));
    }
    Ok(normalized_candidate)
}

/// 清洗后拼接到根目录下，是接收落盘的唯一入口。
pub fn safe_join(root: &Path, name: &str) -> Result<PathBuf, BtError> {
    let cleaned = sanitize_file_name(name);
    ensure_within(root, &root.join(cleaned))
}

/// 追加 ` (n)` 后缀避免覆盖已有文件。
pub fn unique_child(root: &Path, name: &str) -> PathBuf {
    let base = safe_join(root, name).unwrap_or_else(|_| root.join("unnamed"));
    if !base.exists() {
        return base;
    }
    let stem = base
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string());
    let extension = base
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    for index in 1..=9_999u32 {
        let candidate = root.join(format!("{stem} ({index}){extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_path_separators_and_control_chars() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("a\\b\\c.txt"), "c.txt");
        assert_eq!(sanitize_file_name("bad\u{7}name.txt"), "badname.txt");
        assert_eq!(sanitize_file_name("  .hidden  "), ".hidden");
        assert_eq!(sanitize_file_name(""), "unnamed");
    }

    #[test]
    fn handles_absolute_traversal_and_overlong_names() {
        assert_eq!(sanitize_file_name("/etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("..\\..\\evil.txt"), "evil.txt");
        assert_eq!(sanitize_file_name("C:\\Windows\\x.dll"), "x.dll");
        let long = "a".repeat(300);
        assert_eq!(sanitize_file_name(&long).chars().count(), 180);
        assert_eq!(sanitize_file_name(".."), "unnamed");
        assert_eq!(sanitize_file_name("..."), "unnamed");
    }

    #[test]
    fn neutralizes_windows_reserved_names() {
        assert_eq!(sanitize_file_name("CON"), "CON_");
        assert_eq!(sanitize_file_name("lpt9.txt"), "lpt9.txt_");
    }

    #[test]
    fn rejects_parent_directory_escape() {
        let root = Path::new("/tmp/receive");
        assert!(ensure_within(root, Path::new("/tmp/receive/a.txt")).is_ok());
        assert!(ensure_within(root, Path::new("/tmp/receive/../evil.txt")).is_err());
        assert!(ensure_within(root, Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn safe_join_keeps_child_inside_root() {
        let joined = safe_join(Path::new("/tmp/receive"), "../../evil.txt").unwrap();
        assert_eq!(joined, Path::new("/tmp/receive/evil.txt"));
    }

    #[test]
    fn lexical_normalization_folds_dot_segments() {
        let normalized = normalize_lexically(Path::new("/a/./b/../c"));
        assert_eq!(normalized, Path::new("/a/c"));
    }
}
