//! 用系统默认程序打开路径或网址。

use std::path::Path;

use bt_core::BtError;

/// 用文件管理器打开目录，或用默认程序打开文件与网址。
pub fn open_path(path: &Path) -> Result<(), BtError> {
    if path.as_os_str().is_empty() {
        return Err(BtError::invalid("打开路径为空"));
    }
    platform_open(path)
}

#[cfg(target_os = "linux")]
fn platform_open(path: &Path) -> Result<(), BtError> {
    let attempts: [(&str, Vec<std::ffi::OsString>); 2] = [
        ("xdg-open", vec![path.as_os_str().to_os_string()]),
        ("gio", vec!["open".into(), path.as_os_str().to_os_string()]),
    ];
    let mut last_error = String::from("未找到 xdg-open 或 gio");
    for (program, args) in attempts {
        match std::process::Command::new(program).args(&args).spawn() {
            Ok(_) => return Ok(()),
            Err(error) => last_error = format!("{program} 启动失败：{error}"),
        }
    }
    Err(BtError::new(
        bt_core::BtErrorKind::Io,
        format!("无法打开 {}：{last_error}", path.display()),
    ))
}

#[cfg(windows)]
fn platform_open(path: &Path) -> Result<(), BtError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation: Vec<u16> = "open\0".encode_utf16().collect();
    let file: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        return Err(BtError::new(
            bt_core::BtErrorKind::Io,
            format!("无法打开 {}：ShellExecuteW 返回 {}", path.display(), result as isize),
        ));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", windows)))]
fn platform_open(path: &Path) -> Result<(), BtError> {
    let _ = path;
    Err(crate::unsupported("用系统程序打开路径"))
}
