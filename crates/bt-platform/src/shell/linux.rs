//! Linux 文件管理器集成：桌面文件与 Nautilus 脚本。

use std::path::PathBuf;

use bt_core::BtError;

use super::{current_exe, ShellIntegration, ShellReport};
use crate::paths;

const DESKTOP_FILE: &str = "bluetooth-transfer.desktop";
const NAUTILUS_SCRIPT: &str = "蓝牙传输发送";

#[derive(Debug, Default)]
pub struct LinuxShell;

impl ShellIntegration for LinuxShell {
    fn install(&self) -> Result<ShellReport, BtError> {
        let exe = current_exe()?;
        let applications = paths::applications_dir().ok_or_else(|| BtError::internal("无法确定应用菜单目录"))?;
        std::fs::create_dir_all(&applications)?;
        let desktop_path = applications.join(DESKTOP_FILE);
        std::fs::write(&desktop_path, desktop_entry(&exe))?;

        let mut report = ShellReport::ok(format!("已安装应用菜单项：{}", desktop_path.display()));
        match nautilus_script_path() {
            Some(script_path) => {
                if let Some(parent) = script_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&script_path, nautilus_script(&exe))?;
                set_executable(&script_path)?;
                report
                    .warnings
                    .push(format!("Nautilus 脚本已安装：{}", script_path.display()));
            }
            None => report
                .warnings
                .push("未找到用户数据目录，Nautilus 脚本未安装；应用菜单项仍可用。".to_string()),
        }
        Ok(report)
    }

    fn uninstall(&self) -> Result<ShellReport, BtError> {
        let mut removed = Vec::new();
        let mut warnings = Vec::new();
        if let Some(applications) = paths::applications_dir() {
            let desktop_path = applications.join(DESKTOP_FILE);
            if desktop_path.is_file() {
                std::fs::remove_file(&desktop_path)?;
                removed.push(desktop_path.display().to_string());
            }
        }
        if let Some(script_path) = nautilus_script_path() {
            if script_path.is_file() {
                std::fs::remove_file(&script_path)?;
                removed.push(script_path.display().to_string());
            }
        }
        if removed.is_empty() {
            return Ok(ShellReport::ok("未发现已安装的文件管理器集成，无需移除"));
        }
        if let Some(parent) = nautilus_script_path().and_then(|path| path.parent().map(|value| value.to_path_buf())) {
            if removed.iter().any(|path| path.ends_with(NAUTILUS_SCRIPT)) && parent.is_dir() {
                std::fs::remove_dir(&parent).ok();
            }
        }
        warnings.push("若文件管理器仍在显示旧菜单，请注销并重新登录".to_string());
        Ok(ShellReport {
            summary: format!("已移除 {}", removed.join("、")),
            warnings,
        })
    }

    fn is_installed(&self) -> bool {
        paths::applications_dir()
            .map(|dir| dir.join(DESKTOP_FILE).is_file())
            .unwrap_or(false)
    }

    fn label(&self) -> &'static str {
        "应用菜单与 Nautilus 右键脚本"
    }
}

fn nautilus_script_path() -> Option<PathBuf> {
    let data = if let Some(value) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        PathBuf::from(value)
    } else {
        paths::home_dir()?.join(".local").join("share")
    };
    Some(data.join("nautilus").join("scripts").join(NAUTILUS_SCRIPT))
}

fn desktop_entry(exe: &std::path::Path) -> String {
    let exec = quote_exec(exe);
    format!(
        "[Desktop Entry]\nType=Application\nName=蓝牙传输\nComment=通过蓝牙发送与接收文件\nExec={exec} %F\nTerminal=false\nCategories=Utility;Network;\nActions=SendWithBluetooth;\n\n[Desktop Action SendWithBluetooth]\nName=用蓝牙传输发送\nExec={exec} --send %F\n"
    )
}

fn nautilus_script(exe: &std::path::Path) -> String {
    format!(
        "#!/bin/sh\n# 由蓝牙传输安装：把选中的文件/文件夹交给已运行实例\nexec {} --send \"$@\"\n",
        quote_exec(exe)
    )
}

/// 桌面文件 Exec 规则：整体加双引号，内部的双引号与反斜杠转义。
fn quote_exec(exe: &std::path::Path) -> String {
    let raw = exe.to_string_lossy();
    let escaped = raw.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(unix)]
fn set_executable(path: &std::path::Path) -> Result<(), BtError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &std::path::Path) -> Result<(), BtError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_quotes_executable_for_paths_with_spaces() {
        let text = desktop_entry(std::path::Path::new("/opt/My Apps/bt gui"));
        assert!(text.contains("Exec=\"/opt/My Apps/bt gui\" %F"));
        assert!(text.contains("[Desktop Action SendWithBluetooth]"));
    }

    #[test]
    fn nautilus_script_passes_all_selected_items() {
        let text = nautilus_script(std::path::Path::new("/usr/bin/bt-gui"));
        assert!(text.starts_with("#!/bin/sh"));
        assert!(text.contains("--send \"$@\""));
    }
}
