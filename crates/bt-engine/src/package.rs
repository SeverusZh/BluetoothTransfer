use std::path::{Path, PathBuf};

use bt_core::{paths, BtError};

pub const PACKAGE_EXTENSION: &str = "zip";

/// 打包计划：文件夹整体压缩成一个 zip 再走任一条通道。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePlan {
    pub sources: Vec<PathBuf>,
    pub archive_name: String,
    pub estimated_bytes: u64,
}

/// 生成打包计划，不触碰文件内容。
pub fn plan_folder_package(folder: &Path) -> Result<PackagePlan, BtError> {
    if !folder.is_dir() {
        return Err(BtError::not_found(format!("目录不存在 {}", folder.display())));
    }
    let raw_name = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".to_string());
    let archive_name = format!("{}.{PACKAGE_EXTENSION}", paths::sanitize_file_name(&raw_name));
    Ok(PackagePlan {
        sources: vec![folder.to_path_buf()],
        archive_name,
        estimated_bytes: 0,
    })
}

/// 实际压缩；实现落在 T3，需要引入 zip 依赖。
pub fn package_folder(folder: &Path, target: &Path) -> Result<PathBuf, BtError> {
    let plan = plan_folder_package(folder)?;
    Err(BtError::not_implemented(format!(
        "打包待实现 {} -> {}",
        plan.archive_name,
        target.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_uses_sanitized_folder_name() {
        let dir = std::env::temp_dir().join("bt engine pack").join("my_folder");
        std::fs::create_dir_all(&dir).unwrap();
        let plan = plan_folder_package(&dir).unwrap();
        assert_eq!(plan.archive_name, "my_folder.zip");
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("bt engine pack"));
    }

    #[test]
    fn missing_folder_is_reported() {
        let error = plan_folder_package(Path::new("/nonexistent/bt-pack-folder")).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::NotFound);
    }
}
