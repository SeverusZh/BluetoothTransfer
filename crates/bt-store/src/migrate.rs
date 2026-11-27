use std::path::Path;

use bt_api::{MigrateOptions, MigrateReport};
use bt_core::BtError;

/// 旧版本 .NET 应用在 %APPDATA% 下使用的配置文件名。
pub const LEGACY_CONFIG_FILE: &str = "config.json";

/// 旧版本记录文件候选名，按优先级探测。
pub const LEGACY_RECORD_FILES: [&str; 2] = ["records.json", "history.json"];

/// 判断目录是否是旧版应用数据目录。
pub fn looks_like_legacy_root(root: &Path) -> bool {
    root.join(LEGACY_CONFIG_FILE).is_file() || LEGACY_RECORD_FILES.iter().any(|name| root.join(name).is_file())
}

/// 一次性迁移入口；解析与写入在 T3 实现。
pub fn migrate_legacy(options: &MigrateOptions) -> Result<MigrateReport, BtError> {
    if !options.legacy_root.exists() {
        return Err(BtError::not_found(format!(
            "旧数据目录不存在 {}",
            options.legacy_root.display()
        )));
    }
    if !looks_like_legacy_root(&options.legacy_root) {
        return Err(BtError::invalid(format!(
            "目录中没有可识别的旧版数据 {}",
            options.legacy_root.display()
        )));
    }
    Err(BtError::not_implemented("旧版本数据迁移将在 T3 实现"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_root_reports_not_found() {
        let options = MigrateOptions::new(std::path::PathBuf::from("/nonexistent/bt-legacy-root"));
        let error = migrate_legacy(&options).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::NotFound);
    }

    #[test]
    fn empty_directory_is_rejected_as_not_legacy() {
        let dir = std::env::temp_dir().join("bt-store-legacy-empty");
        std::fs::create_dir_all(&dir).unwrap();
        let error = migrate_legacy(&MigrateOptions::new(dir.clone())).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::InvalidArgument);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
