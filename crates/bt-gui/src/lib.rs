//! eframe/egui 桌面界面骨架：一个主窗口加空状态，带无窗口自检入口。
//!
//! 界面只依赖 bt-api 契约与 bt-app 装配根，不直接依赖 bt-engine。

pub mod app;
pub mod state;

use bt_core::BtError;

pub use app::BtApp;
pub use state::UiState;

/// 命令行入口：识别 --selftest 后退出，其余情况打开窗口。
pub fn main_entry(args: Vec<String>) -> Result<u8, BtError> {
    if args.iter().any(|arg| arg == "--selftest") {
        return run_selftest();
    }
    run_window()
}

/// 打开主窗口，阻塞到窗口关闭。
pub fn run_window() -> Result<u8, BtError> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("蓝牙传输"),
        ..Default::default()
    };
    eframe::run_native("蓝牙传输", options, Box::new(|cc| Ok(Box::new(BtApp::new(cc)))))
        .map_err(|error| BtError::internal(format!("窗口创建失败：{error}")))?;
    Ok(0)
}

/// 构建界面状态后立即退出，用于无显示环境下的依赖与装配自检。
pub fn run_selftest() -> Result<u8, BtError> {
    let report = self_check();
    for check in &report.checks {
        let mark = if check.ok { "ok" } else { "fail" };
        println!("[{mark}] {}: {}", check.name, check.detail);
    }
    Ok(if report.passed() { 0 } else { 1 })
}

/// 用内存后端驱动一次界面状态构建，验证契约、事件与渲染数据可用。
pub fn self_check() -> bt_api::SelftestReport {
    use bt_api::{Device, MockBackend};
    use bt_core::DeviceId;
    use std::sync::Arc;

    let mut report = bt_api::SelftestReport::default();
    let first = DeviceId::from_bytes([0xAA, 0xBB, 0xCC, 0x00, 0x11, 0x22]);
    let second = DeviceId::from_bytes([0xAA, 0xBB, 0xCC, 0x00, 0x11, 0x33]);
    let backend: Arc<dyn bt_api::Backend> = Arc::new(MockBackend::new(vec![
        Device::new(first, "Pixel 9"),
        Device::new(second, "ThinkPad"),
    ]));

    let mut state = UiState::new(backend);
    state.start_scan();
    state.poll();
    state.refresh();

    report.push(
        "devices",
        state.devices.len() == 2,
        format!("设备 {}", state.devices.len()),
    );
    report.push("events", state.events_seen > 0, format!("事件 {}", state.events_seen));
    report.push("config", state.config.validate().is_ok(), "配置合法");
    report.push(
        "contract",
        bt_api::CONTRACT_VERSION >= 1,
        format!("契约版本 {}", bt_api::CONTRACT_VERSION),
    );
    report
}
