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
}

/// 通知通道；托盘气泡与系统通知走同一接口。
pub trait Notifier: Send + Sync {
    fn notify(&self, notification: &Notification) -> Result<(), BtError>;
}

/// 未接入平台时的占位实现。
#[derive(Debug, Default)]
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn notify(&self, _notification: &Notification) -> Result<(), BtError> {
        Err(crate::unsupported("桌面通知"))
    }
}

pub fn platform_notifier() -> Box<dyn Notifier> {
    Box::new(NoopNotifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_notifier_reports_unsupported() {
        let notifier = platform_notifier();
        let error = notifier.notify(&Notification::new("t", "b")).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::Unsupported);
    }
}
