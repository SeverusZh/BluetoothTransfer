//! 轻量进程探测：判断独立接收进程是否正在运行，用于接收页冲突提示。

/// 独立接收进程的可执行文件名。
pub const RECEIVER_PROCESS: &str = "btrecv";

/// 探测独立接收进程；探测失败按未运行处理，不阻塞界面。
pub fn standalone_receiver_running() -> bool {
    platform_probe()
}

#[cfg(target_os = "linux")]
fn platform_probe() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    let own = std::process::id().to_string();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == own || !name.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
            if comm.trim() == RECEIVER_PROCESS {
                return true;
            }
        }
        if let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) {
            if cmdline_mentions_receiver(&cmdline) {
                return true;
            }
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn cmdline_mentions_receiver(cmdline: &[u8]) -> bool {
    let text = String::from_utf8_lossy(cmdline);
    text.split('\0')
        .next()
        .and_then(|program| std::path::Path::new(program).file_name())
        .is_some_and(|program| program == RECEIVER_PROCESS)
}

#[cfg(windows)]
fn platform_probe() -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let filter = format!("IMAGENAME eq {RECEIVER_PROCESS}.exe");
    let Ok(output) = std::process::Command::new("tasklist")
        .args(["/FI", &filter, "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .to_ascii_lowercase()
        .contains(&format!("{RECEIVER_PROCESS}.exe"))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn platform_probe() -> bool {
    false
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn only_matches_receiver_program_name() {
        assert!(cmdline_mentions_receiver(b"btrecv\0--dir\0/tmp"));
        assert!(cmdline_mentions_receiver(b"/usr/local/bin/btrecv\0--dir\0/tmp"));
        assert!(!cmdline_mentions_receiver(b"bt-gui\0--send"));
        assert!(!cmdline_mentions_receiver(b"grep\0btrecv"));
    }
}
