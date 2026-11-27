//! 开机自启：Linux 写用户级 autostart 桌面项，Windows 写 HKCU Run 值。

use std::path::PathBuf;

use bt_core::BtError;

/// 自启项：可执行文件加启动参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartEntry {
    pub command: PathBuf,
    pub args: Vec<String>,
}

impl AutostartEntry {
    pub fn new(command: PathBuf) -> Self {
        Self {
            command,
            args: Vec::new(),
        }
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    fn command_line(&self) -> String {
        let mut line = quote(&self.command);
        for argument in &self.args {
            line.push(' ');
            line.push_str(&quote_string(argument));
        }
        line
    }
}

/// 开机自启管理。
pub trait Autostart: Send + Sync {
    fn enable(&self, entry: &AutostartEntry) -> Result<(), BtError>;

    fn disable(&self) -> Result<(), BtError>;

    fn is_enabled(&self) -> bool;
}

/// 按当前平台返回自启实现。
pub fn platform_autostart() -> Box<dyn Autostart> {
    #[cfg(target_os = "linux")]
    {
        Box::new(LinuxAutostart)
    }
    #[cfg(windows)]
    {
        Box::new(WindowsAutostart)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Box::new(UnsupportedAutostart)
    }
}

fn quote(path: &std::path::Path) -> String {
    quote_string(&path.to_string_lossy())
}

fn quote_string(value: &str) -> String {
    if value.contains(' ') || value.contains('"') {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct LinuxAutostart;

#[cfg(target_os = "linux")]
impl Autostart for LinuxAutostart {
    fn enable(&self, entry: &AutostartEntry) -> Result<(), BtError> {
        let path = autostart_path().ok_or_else(|| BtError::internal("无法确定自启目录"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = format!(
            "[Desktop Entry]\nType=Application\nName=蓝牙传输\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
            entry.command_line()
        );
        std::fs::write(path, body)?;
        Ok(())
    }

    fn disable(&self) -> Result<(), BtError> {
        if let Some(path) = autostart_path().filter(|path| path.is_file()) {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        autostart_path().is_some_and(|path| path.is_file())
    }
}

#[cfg(target_os = "linux")]
fn autostart_path() -> Option<PathBuf> {
    crate::paths::autostart_dir().map(|dir| dir.join("bluetooth-transfer.desktop"))
}

#[cfg(windows)]
#[derive(Debug, Default)]
pub struct WindowsAutostart;

#[cfg(windows)]
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "BluetoothTransfer";

#[cfg(windows)]
impl Autostart for WindowsAutostart {
    fn enable(&self, entry: &AutostartEntry) -> Result<(), BtError> {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_WRITE, REG_SZ,
        };

        let key_path = wide(RUN_KEY);
        let mut key = std::ptr::null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key_path.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            return Err(BtError::new(
                bt_core::BtErrorKind::Permission,
                format!("写入开机自启失败（错误码 {status}）"),
            ));
        }
        let name = wide(RUN_VALUE);
        let data: Vec<u16> = entry.command_line().encode_utf16().chain(std::iter::once(0)).collect();
        let result = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        unsafe {
            RegCloseKey(key);
        }
        if result != 0 {
            return Err(BtError::new(
                bt_core::BtErrorKind::Permission,
                format!("写入开机自启值失败（错误码 {result}）"),
            ));
        }
        Ok(())
    }

    fn disable(&self) -> Result<(), BtError> {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_WRITE,
        };
        let key_path = wide(RUN_KEY);
        let mut key = std::ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key_path.as_ptr(), 0, KEY_WRITE, &mut key) };
        if status != 0 {
            return Ok(());
        }
        let name = wide(RUN_VALUE);
        let result = unsafe { RegDeleteValueW(key, name.as_ptr()) };
        unsafe {
            RegCloseKey(key);
        }
        if result != 0 && result != 2 {
            return Err(BtError::new(
                bt_core::BtErrorKind::Permission,
                format!("删除开机自启值失败（错误码 {result}）"),
            ));
        }
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_READ, REG_SZ,
        };
        let key_path = wide(RUN_KEY);
        let mut key = std::ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key_path.as_ptr(), 0, KEY_READ, &mut key) };
        if status != 0 {
            return false;
        }
        let name = wide(RUN_VALUE);
        let mut value_type = 0u32;
        let mut size = 0u32;
        let result = unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                &mut value_type,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        unsafe {
            RegCloseKey(key);
        }
        result == 0 && value_type == REG_SZ
    }
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(not(any(target_os = "linux", windows)))]
#[derive(Debug, Default)]
pub struct UnsupportedAutostart;

#[cfg(not(any(target_os = "linux", windows)))]
impl Autostart for UnsupportedAutostart {
    fn enable(&self, _entry: &AutostartEntry) -> Result<(), BtError> {
        Err(crate::unsupported("开机自启"))
    }

    fn disable(&self) -> Result<(), BtError> {
        Err(crate::unsupported("开机自启"))
    }

    fn is_enabled(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_quotes_paths_with_spaces() {
        let entry = AutostartEntry::new(PathBuf::from("/opt/My Apps/bt gui"));
        assert_eq!(entry.command_line(), "\"/opt/My Apps/bt gui\"");
        let entry = entry.with_args(vec!["--minimized".to_string()]);
        assert_eq!(entry.command_line(), "\"/opt/My Apps/bt gui\" --minimized");
    }
}
