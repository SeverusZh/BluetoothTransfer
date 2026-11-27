use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use bt_api::{JobId, JobSnapshot, JobState, SendItem};
use bt_core::{now_rfc3339, BtError, Channel, DeviceId};

/// 队列内的任务，保留调度所需的全部信息。
#[derive(Debug, Clone)]
pub struct QueueJob {
    pub id: JobId,
    pub device: DeviceId,
    pub device_name: String,
    pub item: SendItem,
    pub channel: Channel,
    pub requested_channel: Option<Channel>,
    pub state: JobState,
    pub bytes_sent: u64,
    pub total_bytes: u64,
    pub bps: u64,
    pub attempts: u32,
    pub error: Option<String>,
    pub resumable: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl QueueJob {
    pub fn snapshot(&self) -> JobSnapshot {
        JobSnapshot {
            id: self.id,
            device_id: self.device,
            device_name: self.device_name.clone(),
            item: self.item.clone(),
            channel: self.channel,
            state: self.state,
            bytes_sent: self.bytes_sent,
            total_bytes: self.total_bytes,
            bps: self.bps,
            eta_seconds: eta_seconds(self.bytes_sent, self.total_bytes, self.bps),
            attempts: self.attempts,
            error: self.error.clone(),
            resumable: self.resumable,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }
}

pub fn eta_seconds(bytes_sent: u64, total: u64, bps: u64) -> Option<u64> {
    if bps == 0 || total <= bytes_sent {
        return None;
    }
    Some((total - bytes_sent).div_ceil(bps))
}

/// 顺序执行器：按入队顺序推进，支持逐任务暂停、恢复、取消与移除。
#[derive(Debug, Default)]
pub struct QueueManager {
    jobs: Vec<QueueJob>,
    next_id: u64,
    paused_all: bool,
}

impl QueueManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(
        &mut self,
        device: &DeviceId,
        device_name: &str,
        item: SendItem,
        requested_channel: Option<Channel>,
    ) -> JobId {
        self.next_id += 1;
        let id = JobId(self.next_id);
        let now = now_rfc3339();
        let total_bytes = item.total_bytes.unwrap_or(0);
        let channel = requested_channel.unwrap_or(Channel::Opp);
        self.jobs.push(QueueJob {
            id,
            device: *device,
            device_name: device_name.to_string(),
            item,
            channel,
            requested_channel,
            state: JobState::Queued,
            bytes_sent: 0,
            total_bytes,
            bps: 0,
            attempts: 0,
            error: None,
            resumable: false,
            created_at: now.clone(),
            updated_at: now,
        });
        id
    }

    pub fn jobs(&self) -> &[QueueJob] {
        &self.jobs
    }

    pub fn snapshot(&self) -> Vec<JobSnapshot> {
        self.jobs.iter().map(QueueJob::snapshot).collect()
    }

    pub fn get(&self, id: JobId) -> Option<&QueueJob> {
        self.jobs.iter().find(|job| job.id == id)
    }

    fn get_mut(&mut self, id: JobId) -> Result<&mut QueueJob, BtError> {
        self.jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or_else(|| BtError::not_found(format!("任务不存在 {}", id.0)))
    }

    pub fn next_runnable(&self) -> Option<&QueueJob> {
        if self.paused_all {
            return None;
        }
        self.jobs.iter().find(|job| job.state == JobState::Queued)
    }

    pub fn mark_connecting(&mut self, id: JobId) -> Result<JobSnapshot, BtError> {
        let job = self.get_mut(id)?;
        if job.state.is_terminal() || job.state == JobState::Paused {
            return Err(BtError::busy("任务状态不允许开始"));
        }
        job.state = JobState::Connecting;
        job.attempts = job.attempts.saturating_add(1);
        job.error = None;
        job.updated_at = now_rfc3339();
        Ok(job.snapshot())
    }

    pub fn set_channel(&mut self, id: JobId, channel: Channel) -> Result<JobSnapshot, BtError> {
        let job = self.get_mut(id)?;
        job.channel = channel;
        job.updated_at = now_rfc3339();
        Ok(job.snapshot())
    }

    pub fn set_total(&mut self, id: JobId, total: u64) -> Result<JobSnapshot, BtError> {
        let job = self.get_mut(id)?;
        job.total_bytes = total;
        job.updated_at = now_rfc3339();
        Ok(job.snapshot())
    }

    pub fn pause(&mut self, id: JobId) -> Result<JobSnapshot, BtError> {
        let job = self.get_mut(id)?;
        if job.is_terminal() {
            return Err(BtError::busy("任务已结束，无法暂停"));
        }
        job.state = JobState::Paused;
        job.updated_at = now_rfc3339();
        Ok(job.snapshot())
    }

    pub fn resume(&mut self, id: JobId) -> Result<JobSnapshot, BtError> {
        let job = self.get_mut(id)?;
        if job.state != JobState::Paused {
            return Err(BtError::invalid("任务不处于暂停态"));
        }
        job.state = JobState::Queued;
        job.error = None;
        job.updated_at = now_rfc3339();
        Ok(job.snapshot())
    }

    pub fn cancel(&mut self, id: JobId) -> Result<JobSnapshot, BtError> {
        let job = self.get_mut(id)?;
        if job.is_terminal() {
            return Err(BtError::busy("任务已结束，无法取消"));
        }
        job.state = JobState::Canceled;
        job.error = Some("用户取消".to_string());
        job.updated_at = now_rfc3339();
        Ok(job.snapshot())
    }

    pub fn retry(&mut self, id: JobId) -> Result<JobSnapshot, BtError> {
        let job = self.get_mut(id)?;
        if !job.is_terminal() {
            return Err(BtError::busy("任务尚未结束"));
        }
        job.state = JobState::Queued;
        job.bytes_sent = 0;
        job.bps = 0;
        job.error = None;
        job.resumable = false;
        job.updated_at = now_rfc3339();
        Ok(job.snapshot())
    }

    pub fn remove(&mut self, id: JobId) -> Result<JobSnapshot, BtError> {
        let index = self
            .jobs
            .iter()
            .position(|job| job.id == id)
            .ok_or_else(|| BtError::not_found(format!("任务不存在 {}", id.0)))?;
        if !self.jobs[index].is_terminal() {
            return Err(BtError::busy("任务尚未结束，不能移除"));
        }
        Ok(self.jobs.remove(index).snapshot())
    }

    pub fn cancel_all(&mut self) -> Vec<JobSnapshot> {
        self.cancel_unfinished_except(None)
    }

    /// 取消未完成任务但保留指定活动任务，交由发送线程写入终态与 journal。
    pub fn cancel_unfinished_except(&mut self, except: Option<JobId>) -> Vec<JobSnapshot> {
        let now = now_rfc3339();
        let mut changed = Vec::new();
        for job in &mut self.jobs {
            if Some(job.id) == except {
                continue;
            }
            if !job.is_terminal() {
                job.state = JobState::Canceled;
                job.error = Some("用户取消".to_string());
                job.updated_at = now.clone();
                changed.push(job.snapshot());
            }
        }
        changed
    }

    pub fn set_progress(&mut self, id: JobId, bytes_sent: u64, bps: u64) -> Result<JobSnapshot, BtError> {
        let job = self.get_mut(id)?;
        if job.state.is_terminal() {
            return Err(BtError::busy("任务已结束"));
        }
        job.bytes_sent = bytes_sent;
        job.bps = bps;
        job.state = JobState::Sending;
        job.updated_at = now_rfc3339();
        Ok(job.snapshot())
    }

    pub fn finish(
        &mut self,
        id: JobId,
        state: JobState,
        error: Option<String>,
        bytes_sent: u64,
        resumable: bool,
    ) -> Result<JobSnapshot, BtError> {
        let job = self.get_mut(id)?;
        job.state = state;
        job.error = error;
        job.resumable = resumable;
        if state == JobState::Completed {
            job.bytes_sent = job.total_bytes.max(bytes_sent);
            job.bps = 0;
        } else {
            job.bytes_sent = bytes_sent;
        }
        job.updated_at = now_rfc3339();
        Ok(job.snapshot())
    }

    pub fn set_paused_all(&mut self, paused: bool) {
        self.paused_all = paused;
    }

    pub fn is_paused_all(&self) -> bool {
        self.paused_all
    }

    pub fn is_idle(&self) -> bool {
        self.jobs.iter().all(QueueJob::is_terminal)
    }

    pub fn has_pending(&self) -> bool {
        self.jobs.iter().any(|job| job.state == JobState::Queued)
    }
}

/// 滑动窗口速率与剩余时间估算；时钟可注入便于确定性测试。
pub struct SpeedTracker {
    window_ms: u64,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    samples: Mutex<VecDeque<(u64, u64)>>,
}

impl SpeedTracker {
    pub fn new(window_ms: u64) -> Self {
        Self {
            window_ms,
            clock: Arc::new(monotonic_millis),
            samples: Mutex::new(VecDeque::new()),
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    pub fn add_sample(&self, total_bytes: u64) {
        let now = (self.clock)();
        if let Ok(mut samples) = self.samples.lock() {
            samples.push_back((now, total_bytes));
            while samples
                .front()
                .is_some_and(|(at, _)| now.saturating_sub(*at) > self.window_ms)
            {
                samples.pop_front();
            }
        }
    }

    pub fn bps(&self) -> u64 {
        let Ok(samples) = self.samples.lock() else {
            return 0;
        };
        if samples.len() < 2 {
            return 0;
        }
        let (first_at, first_bytes) = samples.front().copied().unwrap_or((0, 0));
        let (last_at, last_bytes) = samples.back().copied().unwrap_or((0, 0));
        let elapsed_ms = last_at.saturating_sub(first_at);
        if elapsed_ms == 0 || last_bytes <= first_bytes {
            return 0;
        }
        (last_bytes - first_bytes).saturating_mul(1000) / elapsed_ms
    }

    pub fn eta(&self, total_bytes: u64) -> Option<u64> {
        let current = self
            .samples
            .lock()
            .ok()
            .and_then(|samples| samples.back().map(|(_, bytes)| *bytes))
            .unwrap_or(0);
        let speed = self.bps();
        if speed == 0 || total_bytes <= current {
            return None;
        }
        Some((total_bytes - current).div_ceil(speed))
    }

    pub fn reset(&self) {
        if let Ok(mut samples) = self.samples.lock() {
            samples.clear();
        }
    }
}

impl Default for SpeedTracker {
    fn default() -> Self {
        Self::new(5_000)
    }
}

fn monotonic_millis() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    start.elapsed().as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::{RecordKind, SendSource};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn device() -> DeviceId {
        DeviceId::from_bytes([1, 2, 3, 4, 5, 6])
    }

    fn item() -> SendItem {
        SendItem {
            source: SendSource::Text("hello".into()),
            display_name: "hello".into(),
            total_bytes: Some(100),
            kind: RecordKind::Text,
        }
    }

    #[test]
    fn enqueue_yields_sequential_ids_and_snapshots() {
        let mut queue = QueueManager::new();
        let first = queue.enqueue(&device(), "Pixel", item(), Some(Channel::Opp));
        let second = queue.enqueue(&device(), "Pixel", item(), Some(Channel::Assistant));
        assert_eq!(first.0 + 1, second.0);
        assert_eq!(queue.snapshot().len(), 2);
        assert_eq!(queue.next_runnable().map(|job| job.id), Some(first));
    }

    #[test]
    fn pause_resume_progress_and_eta() {
        let mut queue = QueueManager::new();
        let id = queue.enqueue(&device(), "Pixel", item(), Some(Channel::Opp));
        assert!(queue.mark_connecting(id).is_ok());
        queue.pause(id).unwrap();
        assert!(queue.next_runnable().is_none());
        queue.resume(id).unwrap();
        let snapshot = queue.set_progress(id, 50, 10).unwrap();
        assert_eq!(snapshot.state, JobState::Sending);
        assert_eq!(snapshot.eta_seconds, Some(5));
        assert_eq!(snapshot.bps, 10);
    }

    #[test]
    fn terminal_jobs_can_be_removed_and_retried() {
        let mut queue = QueueManager::new();
        let id = queue.enqueue(&device(), "Pixel", item(), Some(Channel::Opp));
        assert!(queue.remove(id).is_err());
        assert!(queue.retry(id).is_err());
        queue
            .finish(id, JobState::Failed, Some("超时".into()), 10, true)
            .unwrap();
        assert!(queue.is_idle());
        assert!(queue.get(id).unwrap().resumable);
        assert_eq!(queue.retry(id).unwrap().state, JobState::Queued);
        queue.mark_connecting(id).unwrap();
        queue.finish(id, JobState::Completed, None, 100, false).unwrap();
        queue.remove(id).unwrap();
        assert!(queue.jobs().is_empty());
    }

    #[test]
    fn cancel_all_marks_unfinished_jobs() {
        let mut queue = QueueManager::new();
        queue.enqueue(&device(), "Pixel", item(), Some(Channel::Opp));
        queue.enqueue(&device(), "Pixel", item(), Some(Channel::Opp));
        let changed = queue.cancel_all();
        assert_eq!(changed.len(), 2);
        assert!(queue.is_idle());
    }

    #[test]
    fn cancel_all_can_leave_the_active_job_to_the_worker() {
        let mut queue = QueueManager::new();
        let first = queue.enqueue(&device(), "Pixel", item(), Some(Channel::Opp));
        let second = queue.enqueue(&device(), "Pixel", item(), Some(Channel::Opp));
        let changed = queue.cancel_unfinished_except(Some(first));
        assert!(changed.iter().any(|job| job.id == second));
        assert_eq!(queue.get(first).unwrap().state, JobState::Queued);
        assert_eq!(queue.get(second).unwrap().state, JobState::Canceled);
    }

    #[test]
    fn speed_tracker_uses_window_and_injected_clock() {
        let now = Arc::new(AtomicU64::new(0));
        let handle = now.clone();
        let tracker = SpeedTracker::new(5_000).with_clock(Arc::new(move || handle.load(Ordering::SeqCst)));
        tracker.add_sample(0);
        now.store(1_000, Ordering::SeqCst);
        tracker.add_sample(1_000);
        assert_eq!(tracker.bps(), 1_000);
        assert_eq!(tracker.eta(3_000), Some(2));
        now.store(7_000, Ordering::SeqCst);
        tracker.add_sample(1_000);
        assert_eq!(tracker.bps(), 0);
        tracker.reset();
        assert_eq!(tracker.bps(), 0);
    }
}
