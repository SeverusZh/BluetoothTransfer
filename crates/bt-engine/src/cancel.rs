use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 可跨线程设置的取消标记，传输循环在分片边界轮询。
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_is_shared_between_clones() {
        let flag = CancelFlag::new();
        let clone = flag.clone();
        assert!(!clone.is_canceled());
        flag.cancel();
        assert!(clone.is_canceled());
    }
}
