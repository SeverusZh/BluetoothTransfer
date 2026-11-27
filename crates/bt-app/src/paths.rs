use std::path::{Path, PathBuf};

use bt_core::{BtError, Config};

/// 应用数据路径集合；装配层唯一关心目录结构的地方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub receive_dir: PathBuf,
    pub database_file: PathBuf,
    pub journal_dir: PathBuf,
}

impl AppPaths {
    /// 按当前平台约定解析目录，不创建任何文件。
    pub fn resolve() -> Result<Self, BtError> {
        let config_root = config_root().ok_or_else(|| BtError::internal("无法确定配置目录"))?;
        let data_root = data_root().ok_or_else(|| BtError::internal("无法确定数据目录"))?;
        let app_config = config_root.join("BluetoothTransfer");
        let app_data = data_root.join("BluetoothTransfer");
        let receive_dir = downloads_dir()
            .map(|dir| dir.join("BluetoothReceive"))
            .unwrap_or_else(|| app_data.join("receive"));
        Ok(Self {
            config_file: app_config.join("config.toml"),
            log_dir: app_data.join("logs"),
            database_file: app_data.join("records.db"),
            journal_dir: app_data.join("journal"),
            data_dir: app_data,
            receive_dir,
        })
    }

    /// 固定根目录下的路径布局，测试与便携模式共用。
    pub fn under(root: &Path) -> Self {
        let app = root.join("BluetoothTransfer");
        Self {
            config_file: app.join("config.toml"),
            log_dir: app.join("logs"),
            receive_dir: app.join("receive"),
            database_file: app.join("records.db"),
            journal_dir: app.join("journal"),
            data_dir: app,
        }
    }

    pub fn create_dirs(&self) -> Result<(), BtError> {
        for dir in [
            self.data_dir.as_path(),
            self.log_dir.as_path(),
            self.receive_dir.as_path(),
            self.journal_dir.as_path(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        if let Some(parent) = self.config_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    pub fn default_config(&self) -> Config {
        Config::default_for(self.receive_dir.clone())
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn config_root() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("APPDATA").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    home_dir().map(|home| home.join(".config"))
}

fn data_root() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    if let Some(value) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    home_dir().map(|home| home.join(".local").join("share"))
}

fn downloads_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("Downloads"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_builds_expected_layout() {
        let paths = AppPaths::under(Path::new("/tmp/bt-app-test"));
        assert_eq!(
            paths.config_file,
            Path::new("/tmp/bt-app-test/BluetoothTransfer/config.toml")
        );
        assert_eq!(
            paths.database_file,
            Path::new("/tmp/bt-app-test/BluetoothTransfer/records.db")
        );
        assert_eq!(
            paths.receive_dir,
            Path::new("/tmp/bt-app-test/BluetoothTransfer/receive")
        );
        assert_eq!(paths.default_config().receive_dir, paths.receive_dir);
    }

    #[test]
    fn create_dirs_materializes_directories() {
        let root = std::env::temp_dir().join("bt-app-paths-create");
        let paths = AppPaths::under(&root);
        paths.create_dirs().unwrap();
        assert!(paths.data_dir.is_dir());
        assert!(paths.log_dir.is_dir());
        assert!(paths.receive_dir.is_dir());
        assert!(paths.journal_dir.is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }
}
