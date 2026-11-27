//! 构建任务入口；参数解析刻意保持零依赖。

use std::path::Path;
use std::process::{Command, ExitCode};

const HELP: &str = "用法：cargo xtask <任务>

任务：
  msrv        校验 workspace rust-version 不低于依赖声明的最低版本
  dist        发布打包入口，实际执行见发布工作流
  bundle      安装器入口，实际执行见发布工作流
  help        显示本帮助";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("help") | Some("-h") | Some("--help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("msrv") => check_msrv(),
        Some("dist") => {
            eprintln!("dist 由 .github/workflows/release.yml 或 packaging/linux 脚本执行");
            ExitCode::from(1)
        }
        Some("bundle") => {
            eprintln!("bundle 由 .github/workflows/release.yml 的 Windows runner 执行");
            ExitCode::from(1)
        }
        Some(other) => {
            eprintln!("未知任务 {other}");
            println!("{HELP}");
            ExitCode::from(2)
        }
    }
}

/// 依赖声明的 rust-version 是 workspace 声明的最低版本依据。
fn check_msrv() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let manifest = match std::fs::read_to_string(root.join("Cargo.toml")) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("读取 Cargo.toml 失败：{error}");
            return ExitCode::from(1);
        }
    };
    let Some(declared) = find_value(&manifest, "rust-version") else {
        eprintln!("根 Cargo.toml 缺少 workspace.package.rust-version");
        return ExitCode::from(1);
    };
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = match Command::new(cargo)
        .current_dir(&root)
        .args(["metadata", "--format-version", "1", "--locked"])
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!("执行 cargo metadata 失败：{error}");
            return ExitCode::from(1);
        }
    };
    if !output.status.success() {
        eprintln!("cargo metadata 失败：{}", String::from_utf8_lossy(&output.stderr));
        return ExitCode::from(1);
    }
    let metadata = String::from_utf8_lossy(&output.stdout);
    let required = find_all(&metadata, "rust_version")
        .into_iter()
        .max_by_key(|version| version_key(version))
        .unwrap_or_else(|| declared.clone());
    println!("MSRV workspace={declared} 依赖最高要求={required}");
    if version_key(&required) > version_key(&declared) {
        eprintln!("workspace rust-version {declared} 低于依赖要求 {required}；提升 Cargo.toml、clippy.toml 与文档声明");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

/// 从 JSON 或 TOML 文本中取键后的第一个字符串值。
fn find_value(text: &str, key: &str) -> Option<String> {
    for marker in [format!("\"{key}\""), key.to_string()] {
        let Some(start) = text.find(&marker) else {
            continue;
        };
        let rest = text[start + marker.len()..].trim_start();
        let Some(rest) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('=')) else {
            continue;
        };
        let rest = rest.trim_start();
        if let Some(value) = rest.strip_prefix('"') {
            if let Some(end) = value.find('"') {
                return Some(value[..end].to_string());
            }
        }
    }
    None
}

fn find_all(text: &str, key: &str) -> Vec<String> {
    let marker = format!("\"{key}\"");
    let mut values = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(&marker) {
        rest = &rest[index + marker.len()..];
        let trimmed = rest.trim_start();
        let Some(after) = trimmed.strip_prefix(':') else {
            continue;
        };
        let after = after.trim_start();
        if let Some(value) = after.strip_prefix('"') {
            if let Some(end) = value.find('"') {
                values.push(value[..end].to_string());
            }
        }
        rest = after;
    }
    values
}

fn version_key(version: &str) -> Vec<u64> {
    let mut key: Vec<u64> = version.split('.').map(|part| part.parse().unwrap_or(0)).collect();
    while key.len() > 1 && key.last() == Some(&0) {
        key.pop();
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_first_value_in_toml() {
        let text = "[workspace.package]\nedition = \"2021\"\nrust-version = \"1.95\"\n";
        assert_eq!(find_value(text, "rust-version").as_deref(), Some("1.95"));
    }

    #[test]
    fn collects_rust_versions_and_skips_null() {
        let json = r#"{"a":{"rust_version":"1.85"},"b":{"rust_version":null},"c":{"rust_version":"1.95"}}"#;
        assert_eq!(find_all(json, "rust_version"), vec!["1.85", "1.95"]);
    }

    #[test]
    fn version_key_orders_numerically() {
        assert!(version_key("1.95") > version_key("1.9"));
        assert_eq!(version_key("1.95.0"), version_key("1.95"));
    }
}
