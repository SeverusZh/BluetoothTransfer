//! 平台目录解析：只读取环境变量，不创建文件。

use std::path::PathBuf;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn home_dir() -> Option<PathBuf> {
    env_path("HOME").or_else(|| env_path("USERPROFILE"))
}

fn root_from(env_name: &str, fallback: impl FnOnce(&PathBuf) -> PathBuf) -> Option<PathBuf> {
    if let Some(value) = env_path(env_name) {
        return Some(value);
    }
    home_dir().map(|home| fallback(&home))
}

/// 当前用户的应用数据根目录（含应用名子目录）。
pub fn data_dir() -> Option<PathBuf> {
    let root = if cfg!(windows) {
        root_from("LOCALAPPDATA", |home| home.join("AppData").join("Local"))
    } else {
        root_from("XDG_DATA_HOME", |home| home.join(".local").join("share"))
    }?;
    Some(root.join("BluetoothTransfer"))
}

/// 当前用户的配置根目录（含应用名子目录）。
pub fn config_dir() -> Option<PathBuf> {
    let root = if cfg!(windows) {
        root_from("APPDATA", |home| home.join("AppData").join("Roaming"))
    } else {
        root_from("XDG_CONFIG_HOME", |home| home.join(".config"))
    }?;
    Some(root.join("BluetoothTransfer"))
}

/// 用于锁文件等短命对象的目录。
pub fn runtime_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return data_dir().map(|dir| dir.join("runtime"));
    }
    env_path("XDG_RUNTIME_DIR").or_else(|| Some(std::env::temp_dir()))
}

pub fn downloads_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join("Downloads"))
}

/// 桌面文件默认安装目录（仅 Linux 有意义）。
pub fn applications_dir() -> Option<PathBuf> {
    root_from("XDG_DATA_HOME", |home| home.join(".local").join("share")).map(|root| root.join("applications"))
}

/// 自启项目录（仅 Linux 有意义），遵循 XDG 规范放在配置根下的 autostart。
pub fn autostart_dir() -> Option<PathBuf> {
    let root = if cfg!(windows) {
        root_from("APPDATA", |home| home.join("AppData").join("Roaming"))?
    } else {
        root_from("XDG_CONFIG_HOME", |home| home.join(".config"))?
    };
    Some(root.join("autostart"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_under_a_root() {
        if let Some(dir) = data_dir() {
            assert!(dir.is_absolute() || dir.starts_with("BluetoothTransfer"));
            assert!(dir.ends_with("BluetoothTransfer"));
        }
        if let Some(dir) = config_dir() {
            assert!(dir.ends_with("BluetoothTransfer"));
        }
    }
}
