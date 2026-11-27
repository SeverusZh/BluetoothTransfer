use std::time::Duration;

use bt_core::{Channel, TransferMode};

/// 通道探测策略：先查 SDP 是否有私有助手服务，再决定实际通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeStrategy {
    pub sdp_timeout: Duration,
    pub handshake_timeout: Duration,
    pub retries: u32,
}

impl Default for ProbeStrategy {
    fn default() -> Self {
        Self {
            sdp_timeout: Duration::from_secs(5),
            handshake_timeout: Duration::from_secs(8),
            retries: 1,
        }
    }
}

impl ProbeStrategy {
    /// 显式指定通道模式时不必探测。
    pub fn for_mode(mode: TransferMode) -> Self {
        match mode {
            TransferMode::Assistant => Self {
                sdp_timeout: Duration::from_secs(3),
                handshake_timeout: Duration::from_secs(8),
                retries: 0,
            },
            TransferMode::Opp => Self {
                retries: 0,
                ..Self::default()
            },
            TransferMode::Auto => Self::default(),
        }
    }

    /// 依据探测结果与实际模式选择通道。
    pub fn decide(&self, mode: TransferMode, assistant_available: bool) -> Channel {
        match mode {
            TransferMode::Assistant => Channel::Assistant,
            TransferMode::Opp => Channel::Opp,
            TransferMode::Auto if assistant_available => Channel::Assistant,
            TransferMode::Auto => Channel::Opp,
        }
    }

    /// 自动模式下探测失败可回退 OPP，强制模式不允许回退。
    pub fn allows_fallback(&self, mode: TransferMode) -> bool {
        mode == TransferMode::Auto
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_prefers_assistant_when_available() {
        let strategy = ProbeStrategy::default();
        assert_eq!(strategy.decide(TransferMode::Auto, true), Channel::Assistant);
        assert_eq!(strategy.decide(TransferMode::Auto, false), Channel::Opp);
    }

    #[test]
    fn forced_modes_ignore_detection() {
        let strategy = ProbeStrategy::default();
        assert_eq!(strategy.decide(TransferMode::Opp, true), Channel::Opp);
        assert_eq!(strategy.decide(TransferMode::Assistant, false), Channel::Assistant);
    }

    #[test]
    fn only_auto_mode_allows_fallback() {
        let strategy = ProbeStrategy::default();
        assert!(strategy.allows_fallback(TransferMode::Auto));
        assert!(!strategy.allows_fallback(TransferMode::Assistant));
        assert!(!strategy.allows_fallback(TransferMode::Opp));
    }
}
