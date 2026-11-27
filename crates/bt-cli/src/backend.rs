use std::sync::Arc;

use bt_api::{Backend, Device, Event, SharedBackend};
use bt_app::AppPaths;
use bt_core::DeviceId;
use crossbeam_channel::Receiver;

use crate::error::CliError;
use crate::output::OutputMode;

/// 数据根与后端句柄：一次命令执行内共享同一个订阅通道。
pub struct Context {
    pub backend: SharedBackend,
    pub events: Receiver<Event>,
    pub paths: AppPaths,
    pub mode: OutputMode,
    pub repl: bool,
}

impl Context {
    pub fn open(mode: OutputMode) -> Result<Self, CliError> {
        Self::open_with_auto_resume(mode, true)
    }

    /// 续传命令使用：后端打开时不自动恢复 journal，避免查询或取消前触发发送。
    pub fn open_without_resume(mode: OutputMode) -> Result<Self, CliError> {
        Self::open_with_auto_resume(mode, false)
    }

    fn open_with_auto_resume(mode: OutputMode, auto_resume: bool) -> Result<Self, CliError> {
        let paths = resolve_paths()?;
        let options = bt_app::BackendOptions { auto_resume };
        let backend = open_backend(&paths, options)?;
        let events = backend.subscribe();
        Ok(Self {
            backend,
            events,
            paths,
            mode,
            repl: false,
        })
    }

    pub fn shutdown(&self) {
        self.backend.shutdown();
    }

    /// 清空上一命令遗留的事件，避免长任务把旧事件当成本次进度。
    pub fn drain_events(&self) {
        while self.events.try_recv().is_ok() {}
    }
}

/// 解析数据根；BT_HOME 由 AppPaths::resolve 统一处理。
pub fn resolve_paths() -> Result<AppPaths, CliError> {
    AppPaths::resolve().map_err(CliError::from)
}

/// 后端装配交给 bt-app；BT_BACKEND=mock 的选择在装配层完成。
fn open_backend(paths: &AppPaths, options: bt_app::BackendOptions) -> Result<SharedBackend, CliError> {
    let boxed = bt_app::build_backend_with_options(paths.clone(), options).map_err(CliError::from)?;
    Ok(Arc::from(boxed))
}

/// 设备快照中查找指定地址。
pub fn find_device(backend: &dyn Backend, id: DeviceId) -> Option<Device> {
    backend.devices().into_iter().find(|device| device.id == id)
}

/// 读取认证密码：TTY 下不回显，管道下读全部 stdin；空输入表示清除。
pub fn read_auth_password() -> Result<Option<String>, CliError> {
    use std::io::{IsTerminal, Read};
    let stdin = std::io::stdin();
    let raw = if stdin.is_terminal() {
        rpassword::prompt_password("OPP 认证密码（输入不回显，空行清除）：")?
    } else {
        let mut buffer = String::new();
        stdin.lock().read_to_string(&mut buffer).map_err(CliError::from)?;
        buffer
    };
    let trimmed = raw.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}
