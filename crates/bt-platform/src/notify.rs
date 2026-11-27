//! 桌面通知：传输完成与失败、托盘交互结果都走同一接口。

use bt_core::BtError;

/// 一条桌面通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub urgent: bool,
}

impl Notification {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            urgent: false,
        }
    }

    pub fn urgent(mut self) -> Self {
        self.urgent = true;
        self
    }
}

/// 通知通道；托盘气泡与系统通知走同一接口。
pub trait Notifier: Send + Sync {
    fn notify(&self, notification: &Notification) -> Result<(), BtError>;
}

/// 使用系统通知服务；服务缺失时返回明确错误，由界面记日志而不是静默吞掉。
#[derive(Debug, Default)]
pub struct SystemNotifier;

impl Notifier for SystemNotifier {
    fn notify(&self, notification: &Notification) -> Result<(), BtError> {
        let mut desktop = notify_rust::Notification::new();
        desktop
            .summary(&notification.title)
            .body(&notification.body)
            .appname("蓝牙传输")
            .timeout(notify_rust::Timeout::Milliseconds(if notification.urgent {
                10_000
            } else {
                5_000
            }));
        if notification.urgent {
            desktop.urgency(notify_rust::Urgency::Critical);
        }
        desktop.show().map(|_| ()).map_err(|error| {
            BtError::new(
                bt_core::BtErrorKind::Unsupported,
                format!("桌面通知发送失败：{error}；请确认系统通知服务正在运行"),
            )
        })
    }
}

/// 按当前平台返回通知实现。
pub fn platform_notifier() -> Box<dyn Notifier> {
    Box::new(SystemNotifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urgent_flag_is_preserved() {
        let notification = Notification::new("发送完成", "photo.jpg").urgent();
        assert!(notification.urgent);
        assert_eq!(notification.title, "发送完成");
    }
}
