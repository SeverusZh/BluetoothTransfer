use std::path::PathBuf;

use bt_core::BtError;

/// 单实例守卫；存活期间持有平台锁，Drop 时释放。
#[derive(Debug)]
pub struct InstanceGuard {
    name: String,
    primary: bool,
}

impl InstanceGuard {
    pub fn new(name: impl Into<String>, primary: bool) -> Self {
        Self {
            name: name.into(),
            primary,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 本进程是否为首个实例。
    pub fn is_primary(&self) -> bool {
        self.primary
    }
}

/// 获取单实例锁；返回 None 表示已有实例在运行，参数应转交给它。
pub fn acquire_instance(name: &str) -> Result<Option<InstanceGuard>, BtError> {
    if name.trim().is_empty() {
        return Err(BtError::invalid("单实例名不能为空"));
    }
    Err(BtError::not_implemented("单实例锁需要平台实现，T4 完成"))
}

/// 首个实例收到的启动参数。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceMessage {
    pub args: Vec<String>,
}

impl InstanceMessage {
    pub fn new(args: Vec<String>) -> Self {
        Self { args }
    }

    pub fn encode(&self) -> String {
        self.args.join("\n")
    }

    pub fn decode(payload: &str) -> Self {
        if payload.is_empty() {
            return Self::default();
        }
        Self {
            args: payload.split('\n').map(|value| value.to_string()).collect(),
        }
    }

    pub fn to_paths(&self) -> Vec<PathBuf> {
        self.args.iter().map(PathBuf::from).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_round_trips() {
        let message = InstanceMessage::new(vec!["--send".into(), "/tmp/a.txt".into()]);
        assert_eq!(InstanceMessage::decode(&message.encode()), message);
        assert_eq!(InstanceMessage::decode(""), InstanceMessage::default());
    }

    #[test]
    fn empty_name_is_rejected() {
        assert!(acquire_instance(" ").is_err());
    }
}
