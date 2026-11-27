//! eframe/egui 桌面界面：页面、平台集成与无窗口自检入口。
//!
//! 界面只依赖 bt-api 契约与 bt-app 装配根，不直接依赖 bt-engine。

pub mod app;
pub mod copy;
pub mod format;
pub mod log_sink;
pub mod pages;
pub mod selftest;
pub mod state;
pub mod theme;

use std::path::PathBuf;

use bt_api::SharedBackend;
use bt_core::{BtError, Config};

pub use app::{BtApp, GuiRuntime};
pub use state::{Page, UiState};

/// 单实例名；同一个用户会话内只允许一个界面实例。
pub const INSTANCE_NAME: &str = "BluetoothTransfer.Gui";

/// 命令行入口：识别 `--selftest` 后退出，其余情况打开窗口。
pub fn main_entry(args: Vec<String>) -> Result<u8, BtError> {
    if args.iter().any(|arg| arg == "--selftest") {
        return selftest::run();
    }
    run_window(args)
}

/// 打开主窗口，阻塞到窗口关闭。
pub fn run_window(args: Vec<String>) -> Result<u8, BtError> {
    let minimized = args.iter().any(|arg| arg == "--minimized");
    let own_request = bt_platform::SendRequest::parse_cli(&args);
    let paths = bt_app::AppPaths::resolve().ok();
    let data_dir = paths
        .as_ref()
        .map(|paths| paths.data_dir.clone())
        .or_else(bt_platform::paths::data_dir);
    let default_config = paths.as_ref().map(|paths| paths.default_config()).unwrap_or_else(|| {
        let receive_dir = bt_platform::paths::downloads_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("BluetoothReceive");
        Config::default_for(receive_dir)
    });
    let window_file = data_dir.as_ref().map(|dir| dir.join("window.state"));
    let inbox_root = data_dir.as_ref().map(|dir| dir.join("inbox"));
    let window_state = window_file
        .as_deref()
        .map(bt_platform::WindowState::load)
        .unwrap_or_default();

    let guard = match bt_platform::acquire_instance(INSTANCE_NAME)? {
        Some(guard) => Some(guard),
        None => {
            if let Some(root) = &inbox_root {
                let inbox = bt_platform::ExternalInbox::new(root.clone());
                let items = own_request.to_inbox_items();
                if !items.is_empty() || !own_request.warnings.is_empty() {
                    if let Err(error) = inbox.write(&items, &own_request.warnings) {
                        eprintln!("写入外部发送收件箱失败：{}", error.message());
                    }
                }
            }
            let fallback_args = if own_request.is_empty() {
                Vec::new()
            } else {
                args.clone()
            };
            let forwarded =
                bt_platform::signal_existing(INSTANCE_NAME, &bt_platform::InstanceMessage::new(fallback_args))?;
            if forwarded {
                return Ok(0);
            }
            match bt_platform::acquire_instance(INSTANCE_NAME)? {
                Some(guard) => Some(guard),
                None => {
                    return Err(BtError::busy(
                        "已有实例在运行，但无法转交启动参数；请关闭已运行的实例后重试",
                    ))
                }
            }
        }
    };

    let mut backend_error = None;
    if let Some(paths) = paths.as_ref() {
        if let Err(error) = paths.create_dirs() {
            backend_error = Some(format!("创建应用目录失败：{}", error.message()));
        }
    }
    let (backend, build_error) = build_backend(paths.clone());
    let backend_error = match (backend_error, build_error) {
        (Some(first), Some(second)) => Some(format!("{first}；{second}")),
        (Some(first), None) => Some(first),
        (None, Some(second)) => Some(second),
        (None, None) => None,
    };
    let runtime = GuiRuntime {
        backend,
        backend_error,
        default_config,
        data_dir,
        app_paths: paths,
        window_file,
        inbox_root,
        own_request,
        minimized,
        guard,
        window_state,
    };
    run_native(runtime, window_state)
}

fn build_backend(paths: Option<bt_app::AppPaths>) -> (SharedBackend, Option<String>) {
    let Some(paths) = paths else {
        let fallback: SharedBackend = std::sync::Arc::new(bt_api::MockBackend::default());
        return (
            fallback,
            Some("无法确定应用数据目录，当前使用内存后端；记录与配置不会持久化".to_string()),
        );
    };
    match bt_app::build_backend_shared(paths.clone()) {
        Ok(backend) => (backend, None),
        Err(error) => {
            let fallback: SharedBackend = std::sync::Arc::new(bt_api::MockBackend::default());
            (
                fallback,
                Some(format!("后端初始化失败，当前使用内存后端：{}", error.message())),
            )
        }
    }
}

fn run_native(runtime: GuiRuntime, window_state: bt_platform::WindowState) -> Result<u8, BtError> {
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([window_state.width, window_state.height])
        .with_min_inner_size([900.0, 600.0])
        .with_title(format!("蓝牙传输 {}", env!("CARGO_PKG_VERSION")))
        .with_app_id("bluetooth-transfer");
    if window_state.maximized {
        viewport = viewport.with_maximized(true);
    } else {
        viewport = viewport.with_position([window_state.x, window_state.y]);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        concat!("蓝牙传输 ", env!("CARGO_PKG_VERSION")),
        options,
        Box::new(move |cc| Ok(Box::new(BtApp::new(cc, runtime)))),
    )
    .map_err(|error| BtError::internal(format!("窗口创建失败：{error}")))?;
    Ok(0)
}
