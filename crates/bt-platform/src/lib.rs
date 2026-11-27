//! 平台集成层：单实例、外部收件箱、Shell 集成、通知、剪贴板、自启与窗口状态。
//!
//! 本轮只固定 trait 与数据结构，具体实现按目标平台在 T4 补齐；
//! 调用方只需要处理 BtError，不必关心平台差异。

pub mod autostart;
pub mod clipboard;
pub mod inbox;
pub mod notify;
pub mod shell;
pub mod single_instance;
pub mod window_state;

pub use autostart::{Autostart, AutostartEntry};
pub use clipboard::{Clipboard, ClipboardContent};
pub use inbox::{ExternalInbox, InboxItem};
pub use notify::{Notification, Notifier};
pub use shell::{ShellIntegration, ShellScope};
pub use single_instance::{acquire_instance, InstanceGuard, InstanceMessage};
pub use window_state::WindowState;

/// 当前平台缺少某项桌面能力时的统一错误。
pub fn unsupported(feature: &str) -> bt_core::BtError {
    bt_core::BtError::unsupported(format!("{feature} 在当前平台不可用"))
}
