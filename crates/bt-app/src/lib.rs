//! 装配根：把契约、存储、引擎与平台传输拼装成后端实例。
//!
//! GUI 与 CLI 都通过本 crate 的 build_backend 拿到 Arc 化后端，
//! 不直接依赖 bt-engine 与 bt-store，保证上层实现可替换。

pub mod paths;
pub mod stub;

pub use paths::AppPaths;
pub use stub::StubBackend;

use bt_api::{Backend, SharedBackend};
use bt_core::BtError;

use std::sync::Arc;

/// 构建后端。当前返回装配骨架：配置、记录与事件已接通，蓝牙能力等待 T3。
pub fn build_backend(paths: AppPaths) -> Result<Box<dyn Backend>, BtError> {
    Ok(Box::new(StubBackend::open(paths)?))
}

/// 构建可跨线程共享的后端句柄。
pub fn build_backend_shared(paths: AppPaths) -> Result<SharedBackend, BtError> {
    Ok(Arc::new(StubBackend::open(paths)?))
}

/// 装配层持有异步运行时，供后续慢操作的调度线程复用。
pub fn build_runtime() -> Result<tokio::runtime::Runtime, BtError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| BtError::internal(format!("运行时创建失败：{error}")))
}
