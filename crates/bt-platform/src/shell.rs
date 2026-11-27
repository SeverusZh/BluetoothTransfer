use bt_core::BtError;

/// Shell 集成覆盖的对象范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellScope {
    Files,
    Directories,
}

/// 资源管理器或文件管理器右键菜单集成。
pub trait ShellIntegration: Send + Sync {
    fn install(&self, scope: ShellScope) -> Result<(), BtError>;

    fn uninstall(&self) -> Result<(), BtError>;

    fn is_installed(&self) -> bool;
}

/// 未接入平台时的占位实现。
#[derive(Debug, Default)]
pub struct NoopShell;

impl ShellIntegration for NoopShell {
    fn install(&self, _scope: ShellScope) -> Result<(), BtError> {
        Err(crate::unsupported("右键菜单集成"))
    }

    fn uninstall(&self) -> Result<(), BtError> {
        Err(crate::unsupported("右键菜单集成"))
    }

    fn is_installed(&self) -> bool {
        false
    }
}

/// 按当前平台返回集成实现。
pub fn platform_shell() -> Box<dyn ShellIntegration> {
    Box::new(NoopShell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_shell_reports_unsupported() {
        let shell = platform_shell();
        assert!(!shell.is_installed());
        let error = shell.install(ShellScope::Files).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::Unsupported);
    }
}
