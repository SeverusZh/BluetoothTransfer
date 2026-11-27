use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bt_core::{Channel, DeviceId, TransferMode};

/// 助手探测三态：查询失败必须与确认不存在区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    Unknown,
    Available,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub outcome: ProbeOutcome,
    pub error: Option<String>,
}

impl ProbeResult {
    pub fn unknown() -> Self {
        Self {
            outcome: ProbeOutcome::Unknown,
            error: None,
        }
    }

    pub fn available() -> Self {
        Self {
            outcome: ProbeOutcome::Available,
            error: None,
        }
    }

    pub fn absent() -> Self {
        Self {
            outcome: ProbeOutcome::Absent,
            error: None,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            outcome: ProbeOutcome::Unknown,
            error: Some(error.into()),
        }
    }

    pub fn is_available(&self) -> bool {
        self.outcome == ProbeOutcome::Available
    }

    pub fn is_absent(&self) -> bool {
        self.outcome == ProbeOutcome::Absent
    }

    pub fn is_unknown(&self) -> bool {
        self.outcome == ProbeOutcome::Unknown
    }
}

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

    pub fn decide(&self, mode: TransferMode, assistant_available: bool) -> Channel {
        match mode {
            TransferMode::Assistant => Channel::Assistant,
            TransferMode::Opp => Channel::Opp,
            TransferMode::Auto if assistant_available => Channel::Assistant,
            TransferMode::Auto => Channel::Opp,
        }
    }

    pub fn allows_fallback(&self, mode: TransferMode) -> bool {
        mode == TransferMode::Auto
    }
}

#[derive(Debug, Clone)]
struct Observation {
    outcome: ProbeOutcome,
    observed_at: u64,
    error: Option<String>,
}

/// 助手在线状态跟踪：正结果带 TTL，负结果与失败均不缓存，失败不降级已知正结果。
pub struct PresenceTracker {
    ttl: Duration,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    observations: Mutex<HashMap<DeviceId, Observation>>,
}

impl PresenceTracker {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            clock: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_secs())
                    .unwrap_or(0)
            }),
            observations: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// 当前可用缓存结论：只有未过期的正结果返回 Available。
    pub fn current(&self, device: &DeviceId) -> ProbeResult {
        let now = (self.clock)();
        let Ok(observations) = self.observations.lock() else {
            return ProbeResult::unknown();
        };
        match observations.get(device) {
            Some(observation) => effective(observation, now, self.ttl),
            None => ProbeResult::unknown(),
        }
    }

    /// 记录一次观察，返回落库后的有效结论；旧观察不得覆盖新结论。
    pub fn record(&self, device: &DeviceId, result: ProbeResult) -> ProbeResult {
        self.record_at(device, result, (self.clock)())
    }

    pub fn record_at(&self, device: &DeviceId, result: ProbeResult, observed_at: u64) -> ProbeResult {
        let now = (self.clock)();
        let Ok(mut observations) = self.observations.lock() else {
            return result;
        };
        if let Some(current) = observations.get(device) {
            if observed_at < current.observed_at {
                return effective(current, now, self.ttl);
            }
            if result.is_unknown()
                && current.outcome == ProbeOutcome::Available
                && now.saturating_sub(current.observed_at) <= self.ttl.as_secs()
            {
                return ProbeResult {
                    outcome: ProbeOutcome::Available,
                    error: result.error,
                };
            }
        }
        observations.insert(
            *device,
            Observation {
                outcome: result.outcome,
                observed_at,
                error: result.error.clone(),
            },
        );
        result
    }

    pub fn invalidate(&self, device: &DeviceId) {
        if let Ok(mut observations) = self.observations.lock() {
            observations.remove(device);
        }
    }

    pub fn clear(&self) {
        if let Ok(mut observations) = self.observations.lock() {
            observations.clear();
        }
    }

    pub fn last_observed(&self, device: &DeviceId) -> ProbeResult {
        let Ok(observations) = self.observations.lock() else {
            return ProbeResult::unknown();
        };
        observations
            .get(device)
            .map(|observation| ProbeResult {
                outcome: observation.outcome,
                error: observation.error.clone(),
            })
            .unwrap_or_else(ProbeResult::unknown)
    }
}

fn effective(observation: &Observation, now: u64, ttl: Duration) -> ProbeResult {
    if observation.outcome != ProbeOutcome::Available {
        return ProbeResult::unknown();
    }
    if now.saturating_sub(observation.observed_at) <= ttl.as_secs() {
        ProbeResult::available()
    } else {
        ProbeResult::unknown()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceId {
        DeviceId::parse("AA:BB:CC:00:00:01").unwrap()
    }

    fn tracker(now: u64, ttl_s: u64) -> (PresenceTracker, Arc<std::sync::atomic::AtomicU64>) {
        let clock = Arc::new(std::sync::atomic::AtomicU64::new(now));
        let handle = clock.clone();
        let tracker = PresenceTracker::new(Duration::from_secs(ttl_s))
            .with_clock(Arc::new(move || clock.load(std::sync::atomic::Ordering::SeqCst)));
        (tracker, handle)
    }

    #[test]
    fn auto_mode_prefers_assistant_and_forced_modes_ignore_probe() {
        let strategy = ProbeStrategy::default();
        assert_eq!(strategy.decide(TransferMode::Auto, true), Channel::Assistant);
        assert_eq!(strategy.decide(TransferMode::Auto, false), Channel::Opp);
        assert_eq!(strategy.decide(TransferMode::Opp, true), Channel::Opp);
        assert_eq!(strategy.decide(TransferMode::Assistant, false), Channel::Assistant);
        assert!(strategy.allows_fallback(TransferMode::Auto));
        assert!(!strategy.allows_fallback(TransferMode::Assistant));
        assert!(!strategy.allows_fallback(TransferMode::Opp));
    }

    #[test]
    fn available_result_expires_to_unknown() {
        let (tracker, clock) = tracker(1_000, 10);
        tracker.record(&device(), ProbeResult::available());
        assert!(tracker.current(&device()).is_available());
        clock.store(1_011, std::sync::atomic::Ordering::SeqCst);
        assert!(tracker.current(&device()).is_unknown());
    }

    #[test]
    fn failure_does_not_downgrade_recent_available() {
        let (tracker, _clock) = tracker(1_000, 10);
        tracker.record(&device(), ProbeResult::available());
        let result = tracker.record(&device(), ProbeResult::failed("SDP 超时"));
        assert!(result.is_available());
        assert!(tracker.current(&device()).is_available());
    }

    #[test]
    fn absent_and_failure_are_not_cached_as_negative() {
        let (tracker, _clock) = tracker(1_000, 10);
        tracker.record(&device(), ProbeResult::absent());
        assert!(tracker.current(&device()).is_unknown());
        tracker.record(&device(), ProbeResult::failed("拒绝"));
        assert!(tracker.current(&device()).is_unknown());
    }

    #[test]
    fn stale_observation_cannot_override_newer_result() {
        let (tracker, _clock) = tracker(1_000, 10);
        tracker.record_at(&device(), ProbeResult::absent(), 2_000);
        let result = tracker.record_at(&device(), ProbeResult::available(), 1_000);
        assert!(!result.is_available());
        assert!(tracker.current(&device()).is_unknown());
    }
}
