use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

static CANCELED: AtomicBool = AtomicBool::new(false);
static INSTALL: Once = Once::new();

/// 安装 Ctrl+C 处理器；重复调用无副作用。
pub fn install() {
    INSTALL.call_once(|| {
        let _ = ctrlc::set_handler(|| {
            CANCELED.store(true, Ordering::SeqCst);
        });
    });
}

pub fn canceled() -> bool {
    CANCELED.load(Ordering::SeqCst)
}

pub fn reset() {
    CANCELED.store(false, Ordering::SeqCst);
}
