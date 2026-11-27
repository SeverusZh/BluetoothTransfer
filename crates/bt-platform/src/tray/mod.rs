//! 托盘图标与菜单：显示、隐藏、退出、安装或移除文件管理器集成。
//!
//! Linux 走 StatusNotifierItem + DBusMenu，Windows 走 Shell_NotifyIcon；
//! 两者都只发命令，窗口显示与退出由界面线程执行。

use bt_core::BtError;
use crossbeam_channel::Receiver;

/// 托盘菜单命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    Hide,
    Exit,
    InstallIntegration,
    UninstallIntegration,
}

/// 托盘菜单需要展示的状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrayMenuState {
    pub integration_installed: bool,
}

/// 已启动的托盘；Drop 时移除图标并释放平台资源。
pub trait TrayHandle: Send + Sync {
    /// 界面线程每帧轮询的菜单命令通道。
    fn commands(&self) -> Receiver<TrayCommand>;

    /// 托盘已启动但无法显示时的原因（例如缺少 StatusNotifierWatcher）。
    fn unavailable_reason(&self) -> Option<String> {
        None
    }
}

/// 启动托盘；平台缺能力时返回明确错误，界面继续运行但记录原因。
pub fn platform_tray(state: TrayMenuState) -> Result<Box<dyn TrayHandle>, BtError> {
    #[cfg(target_os = "linux")]
    {
        linux::start(state)
    }
    #[cfg(windows)]
    {
        windows::start(state)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = state;
        Err(crate::unsupported("系统托盘"))
    }
}

/// 生成 32x32 托盘图标 RGBA，避免内嵌图片资源。
pub fn icon_rgba() -> (Vec<u8>, u32, u32) {
    const SIZE: u32 = 32;
    let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
    let brand = [15u8, 108, 189, 255];
    let white = [255u8, 255, 255, 255];
    let radius = 7.0f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            if inside_rounded(fx, fy, SIZE as f32, radius) {
                let index = ((y * SIZE + x) * 4) as usize;
                pixels[index..index + 4].copy_from_slice(&brand);
            }
        }
    }
    let points: [(f32, f32); 7] = [
        (16.0, 6.0),
        (24.0, 12.5),
        (16.0, 18.0),
        (24.0, 23.5),
        (16.0, 28.0),
        (16.0, 6.0),
        (16.0, 28.0),
    ];
    for pair in points.windows(2) {
        draw_line(&mut pixels, SIZE, pair[0], pair[1], 2.1, white);
    }
    (pixels, SIZE, SIZE)
}

fn inside_rounded(x: f32, y: f32, size: f32, radius: f32) -> bool {
    let min = 1.0;
    let max = size - 1.0;
    if x < min || y < min || x > max || y > max {
        return false;
    }
    let cx = x.clamp(radius, size - radius);
    let cy = y.clamp(radius, size - radius);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

fn draw_line(rgba: &mut [u8], size: u32, from: (f32, f32), to: (f32, f32), thickness: f32, color: [u8; 4]) {
    let (x0, y0) = from;
    let (x1, y1) = to;
    let min_x = (x0.min(x1) - thickness).floor().max(0.0) as u32;
    let max_x = (x0.max(x1) + thickness).ceil().min(size as f32 - 1.0) as u32;
    let min_y = (y0.min(y1) - thickness).floor().max(0.0) as u32;
    let max_y = (y0.max(y1) + thickness).ceil().min(size as f32 - 1.0) as u32;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let length_squared = dx * dx + dy * dy;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let t = if length_squared == 0.0 {
                0.0
            } else {
                (((px - x0) * dx + (py - y0) * dy) / length_squared).clamp(0.0, 1.0)
            };
            let closest_x = x0 + t * dx;
            let closest_y = y0 + t * dy;
            let distance = ((px - closest_x).powi(2) + (py - closest_y).powi(2)).sqrt();
            if distance <= thickness / 2.0 {
                let index = ((y * size + x) * 4) as usize;
                rgba[index..index + 4].copy_from_slice(&color);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_opaque_brand_square_with_rune() {
        let (pixels, width, height) = icon_rgba();
        assert_eq!(width, 32);
        assert_eq!(height, 32);
        assert_eq!(pixels.len(), 32 * 32 * 4);
        let center = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(&pixels[center..center + 4], &[255, 255, 255, 255]);
        let corner = 0usize;
        assert_eq!(pixels[corner + 3], 0);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_runtime_tests {
    use super::*;

    #[test]
    #[ignore = "需要桌面会话的 D-Bus"]
    fn starts_and_owns_status_notifier_name() {
        let handle = platform_tray(TrayMenuState::default()).expect("托盘应能在会话总线上启动");
        std::thread::sleep(std::time::Duration::from_millis(500));
        let output = std::process::Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.freedesktop.DBus",
                "--object-path",
                "/org/freedesktop/DBus",
                "--method",
                "org.freedesktop.DBus.ListNames",
            ])
            .output()
            .expect("gdbus 应可用");
        let names = String::from_utf8_lossy(&output.stdout);
        assert!(names.contains("org.kde.StatusNotifierItem"), "{names}");
        drop(handle);
    }
}
