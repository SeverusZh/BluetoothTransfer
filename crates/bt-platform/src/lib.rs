//! 平台集成层：单实例、外部收件箱、Shell 集成、通知、剪贴板、对话框、自启与窗口状态。
//!
//! 调用方只处理 BtError；平台缺少某项桌面能力时返回 Unsupported，不静默失败。

pub mod autostart;
pub mod clipboard;
pub mod dialog;
pub mod inbox;
pub mod notify;
pub mod open;
pub mod paths;
pub mod process;
pub mod shell;
pub mod single_instance;
pub mod tray;
pub mod window_state;

pub use autostart::{platform_autostart, Autostart, AutostartEntry};
pub use clipboard::{platform_clipboard, Clipboard, ClipboardContent, ClipboardImage};
pub use dialog::{pick_files, pick_folder, save_file, PickedFiles};
pub use inbox::{ExternalInbox, InboxDrain, InboxItem, InboxItemKind};
pub use notify::{platform_notifier, Notification, Notifier};
pub use shell::platform_shell;
pub use shell::{send_request_from_args, SendKind, SendRequest, SendRequestItem, ShellIntegration, ShellReport};
pub use single_instance::{acquire_instance, signal_existing, InstanceGuard, InstanceMessage};
pub use tray::{platform_tray, TrayCommand, TrayHandle, TrayMenuState};
pub use window_state::WindowState;

/// 当前平台缺少某项桌面能力时的统一错误。
pub fn unsupported(feature: &str) -> bt_core::BtError {
    bt_core::BtError::unsupported(format!("{feature} 在当前平台不可用"))
}
