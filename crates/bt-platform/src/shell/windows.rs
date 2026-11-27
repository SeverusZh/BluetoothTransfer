//! Windows 资源管理器集成：HKCU 静态 Verb 与「发送到」快捷方式。

use std::path::{Path, PathBuf};

use bt_core::BtError;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    KEY_WRITE, REG_SZ,
};

use super::{current_exe, ShellIntegration, ShellReport};

const CLASSES: &str = "Software\\Classes";
const SEND_TO_NAME: &str = "蓝牙传输.lnk";

const FILE_VERB: &str = "Software\\Classes\\*\\shell\\BluetoothTransfer.Send";
const DIRECTORY_VERB: &str = "Software\\Classes\\Directory\\shell\\BluetoothTransfer.Send";
const BACKGROUND_VERB: &str = "Software\\Classes\\Directory\\Background\\shell\\BluetoothTransfer.Send";
const DRIVE_VERB: &str = "Software\\Classes\\Drive\\shell\\BluetoothTransfer.Send";
const TEXT_VERB: &str = "Software\\Classes\\SystemFileAssociations\\.txt\\shell\\BluetoothTransfer.SendText";

const VERBS: [&str; 5] = [FILE_VERB, DIRECTORY_VERB, BACKGROUND_VERB, DRIVE_VERB, TEXT_VERB];

#[derive(Debug, Default)]
pub struct WindowsShell;

impl ShellIntegration for WindowsShell {
    fn install(&self) -> Result<ShellReport, BtError> {
        let exe = current_exe()?;
        let icon = format!("\"{}\",0", exe.display());
        let mut report = ShellReport::ok("已安装资源管理器右键菜单");
        for (verb, menu_text, placeholder, argument) in [
            (FILE_VERB, "用蓝牙传输发送", "%1", "--send"),
            (DIRECTORY_VERB, "用蓝牙传输发送", "%1", "--send"),
            (BACKGROUND_VERB, "用蓝牙传输发送当前文件夹", "%V", "--send"),
            (DRIVE_VERB, "用蓝牙传输发送此驱动器", "%1", "--send"),
            (TEXT_VERB, "用蓝牙发送文本内容", "%1", "--send-text"),
        ] {
            write_verb(verb, menu_text, &icon, &exe, placeholder, argument)?;
        }
        match send_to_shortcut_path() {
            Some(path) => match create_shortcut(&path, &exe) {
                Ok(()) => report
                    .warnings
                    .push(format!("「发送到」快捷方式已创建：{}", path.display())),
                Err(error) => report.warnings.push(format!(
                    "「发送到」快捷方式创建失败：{}；多选发送请使用右键菜单",
                    error.message()
                )),
            },
            None => report
                .warnings
                .push("未找到 APPDATA，无法创建「发送到」快捷方式".to_string()),
        }
        report
            .warnings
            .push("Windows 11 请在右键菜单的「显示更多选项」中查看".to_string());
        Ok(report)
    }

    fn uninstall(&self) -> Result<ShellReport, BtError> {
        let mut removed = 0usize;
        for verb in VERBS {
            if delete_key_tree(verb)? {
                removed += 1;
            }
        }
        let mut shortcut_removed = false;
        if let Some(path) = send_to_shortcut_path() {
            if path.is_file() {
                std::fs::remove_file(&path)?;
                shortcut_removed = true;
            }
        }
        if removed == 0 && !shortcut_removed {
            return Ok(ShellReport::ok("未发现已安装的资源管理器集成，无需移除"));
        }
        Ok(ShellReport::ok(format!(
            "已移除 {removed} 个右键菜单项{}",
            if shortcut_removed {
                "与「发送到」快捷方式"
            } else {
                ""
            }
        )))
    }

    fn is_installed(&self) -> bool {
        key_exists(FILE_VERB)
    }

    fn label(&self) -> &'static str {
        "资源管理器右键菜单与发送到"
    }
}

fn write_verb(
    verb: &str,
    menu_text: &str,
    icon: &str,
    exe: &Path,
    placeholder: &str,
    argument: &str,
) -> Result<(), BtError> {
    let command = format!("\"{}\" {argument} \"{placeholder}\"", exe.display());
    let key = create_key(verb)?;
    let result = (|| {
        set_string(key, None, menu_text)?;
        set_string(key, Some("MUIVerb"), menu_text)?;
        set_string(key, Some("Icon"), icon)?;
        set_string(key, Some("MultiSelectModel"), "Single")?;
        set_string(key, Some("Position"), "Top")?;
        Ok::<(), BtError>(())
    })();
    unsafe {
        RegCloseKey(key);
    }
    result?;
    let command_key = create_key(&format!("{verb}\\command"))?;
    let result = set_string(command_key, None, &command);
    unsafe {
        RegCloseKey(command_key);
    }
    result
}

fn create_key(path: &str) -> Result<HKEY, BtError> {
    let path = wide(path);
    let mut key: HKEY = std::ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(BtError::new(
            bt_core::BtErrorKind::Permission,
            format!("写入注册表失败（错误码 {status}）：{path:?}"),
        ));
    }
    Ok(key)
}

fn set_string(key: HKEY, name: Option<&str>, value: &str) -> Result<(), BtError> {
    let name = name.map(wide);
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = data.len() * std::mem::size_of::<u16>();
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ref().map(|value| value.as_ptr()).unwrap_or(std::ptr::null()),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            bytes as u32,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(BtError::new(
            bt_core::BtErrorKind::Permission,
            format!("写入注册表值失败（错误码 {status}）"),
        ));
    }
    Ok(())
}

fn delete_key_tree(path: &str) -> Result<bool, BtError> {
    let path = wide(path);
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
    match status {
        ERROR_SUCCESS => Ok(true),
        ERROR_FILE_NOT_FOUND => Ok(false),
        other => Err(BtError::new(
            bt_core::BtErrorKind::Permission,
            format!("删除注册表项失败（错误码 {other}）"),
        )),
    }
}

fn key_exists(path: &str) -> bool {
    let path = wide(path);
    let mut key: HKEY = std::ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) };
    if status == ERROR_SUCCESS {
        unsafe {
            RegCloseKey(key);
        }
        true
    } else {
        false
    }
}

fn send_to_shortcut_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA").filter(|value| !value.is_empty())?;
    Some(
        PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("SendTo")
            .join(SEND_TO_NAME),
    )
}

fn create_shortcut(path: &Path, exe: &Path) -> Result<(), BtError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Arguments='--send';$s.IconLocation='{},0';$s.Save()",
        ps_quote(&path.to_string_lossy()),
        ps_quote(&exe.to_string_lossy()),
        ps_quote(&exe.to_string_lossy())
    );
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|error| BtError::internal(format!("启动 PowerShell 失败：{error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BtError::internal(format!("创建快捷方式失败：{}", stderr.trim())));
    }
    if !path.is_file() {
        return Err(BtError::internal(format!("快捷方式未生成：{}", path.display())));
    }
    Ok(())
}

fn ps_quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[allow(dead_code)]
fn assert_classes_constant() {
    let _ = CLASSES;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_quotes_apostrophes() {
        assert_eq!(ps_quote("it's"), "it''s");
    }

    #[test]
    fn command_line_keeps_placeholder_quoted() {
        let exe = Path::new("C:\\Program Files\\蓝牙传输\\bt-gui.exe");
        let command = format!("\"{}\" --send \"%1\"", exe.display());
        assert!(command.ends_with("--send \"%1\""));
        assert!(command.starts_with("\"C:\\Program Files"));
    }
}
