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

fn eta_seconds(bytes_sent: u64, total: u64, bps: u64) -> Option<u64> {
    if bps == 0 || total <= bytes_sent {
        return None;
    }
    Some((total - bytes_sent).div_ceil(bps))
}

/// 顺序执行器骨架：按入队顺序推进，支持逐任务暂停、恢复、取消与移除。
#[derive(Debug, Default)]
pub struct QueueManager {
    jobs: Vec<QueueJob>,
    next_id: u64,
    paused: bool,
}

impl QueueManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, device: &DeviceId, device_name: &str, item: SendItem, channel: Channel) -> JobId {
        self.next_id += 1;
        let id = JobId(self.next_id);
        let now = now_rfc3339();
        let total_bytes = item.total_bytes.unwrap_or(0);
        self.jobs.push(QueueJob {
            id,
            device: *device,
            device_name: device_name.to_string(),
            item,
            channel,
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

    /// 取下一个可执行任务；暂停态或非队首任务不会被选中。
    pub fn next_runnable(&self) -> Option<&QueueJob> {
        if self.paused {
            return None;
        }
        self.jobs.iter().find(|job| matches!(job.state, JobState::Queued))
    }

    pub fn pause(&mut self, id: JobId) -> Result<(), BtError> {
        let job = self.get_mut(id)?;
        if job.is_terminal() {
            return Err(BtError::busy("任务已结束，无法暂停"));
        }
        job.state = JobState::Paused;
        job.updated_at = now_rfc3339();
        Ok(())
    }

    pub fn resume(&mut self, id: JobId) -> Result<(), BtError> {
        let job = self.get_mut(id)?;
        if job.state != JobState::Paused {
            return Err(BtError::invalid("任务不处于暂停态"));
        }
        job.state = JobState::Queued;
        job.updated_at = now_rfc3339();
        Ok(())
    }

    pub fn cancel(&mut self, id: JobId) -> Result<(), BtError> {
        let job = self.get_mut(id)?;
        if job.is_terminal() {
            return Err(BtError::busy("任务已结束，无法取消"));
        }
        job.state = JobState::Canceled;
        job.updated_at = now_rfc3339();
        Ok(())
    }

    pub fn retry(&mut self, id: JobId) -> Result<(), BtError> {
        let job = self.get_mut(id)?;
        if !job.is_terminal() {
            return Err(BtError::busy("任务尚未结束"));
        }
        job.state = JobState::Queued;
        job.attempts += 1;
        job.error = None;
        job.updated_at = now_rfc3339();
        Ok(())
    }

    pub fn remove(&mut self, id: JobId) -> Result<(), BtError> {
        let index = self
            .jobs
            .iter()
            .position(|job| job.id == id)
            .ok_or_else(|| BtError::not_found(format!("任务不存在 {}", id.0)))?;
        if !self.jobs[index].is_terminal() {
            return Err(BtError::busy("任务尚未结束，不能移除"));
        }
        self.jobs.remove(index);
        Ok(())
    }

    pub fn cancel_all(&mut self) {
        let now = now_rfc3339();
        for job in &mut self.jobs {
            if !job.is_terminal() {
                job.state = JobState::Canceled;
                job.updated_at = now.clone();
            }
        }
    }

    pub fn set_progress(&mut self, id: JobId, bytes_sent: u64, bps: u64) -> Result<(), BtError> {
        let job = self.get_mut(id)?;
        job.bytes_sent = bytes_sent;
        job.bps = bps;
        job.state = JobState::Sending;
        job.updated_at = now_rfc3339();
        Ok(())
    }

    pub fn finish(&mut self, id: JobId, error: Option<String>) -> Result<(), BtError> {
        let job = self.get_mut(id)?;
        job.state = if error.is_some() {
            JobState::Failed
        } else {
            JobState::Completed
        };
        job.error = error;
        if job.state == JobState::Completed {
            job.bytes_sent = job.total_bytes;
        }
        job.updated_at = now_rfc3339();
        Ok(())
    }

    pub fn set_paused_all(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn is_idle(&self) -> bool {
        self.jobs.iter().all(QueueJob::is_terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::{RecordKind, SendSource};

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
    fn enqueue_yields_sequential_ids() {
        let mut queue = QueueManager::new();
        let first = queue.enqueue(&device(), "Pixel", item(), Channel::Opp);
        let second = queue.enqueue(&device(), "Pixel", item(), Channel::Assistant);
        assert_eq!(first.0 + 1, second.0);
        assert_eq!(queue.snapshot().len(), 2);
    }

    #[test]
    fn pause_resume_and_progress_are_reflected() {
        let mut queue = QueueManager::new();
        let id = queue.enqueue(&device(), "Pixel", item(), Channel::Opp);
        assert_eq!(queue.next_runnable().map(|job| job.id), Some(id));
        queue.pause(id).unwrap();
        assert!(queue.next_runnable().is_none());
        queue.resume(id).unwrap();
        queue.set_progress(id, 50, 10).unwrap();
        let snapshot = queue.get(id).unwrap().snapshot();
        assert_eq!(snapshot.bytes_sent, 50);
        assert_eq!(snapshot.eta_seconds, Some(5));
    }

    #[test]
    fn terminal_jobs_can_be_removed_only() {
        let mut queue = QueueManager::new();
        let id = queue.enqueue(&device(), "Pixel", item(), Channel::Opp);
        assert!(queue.remove(id).is_err());
        queue.finish(id, None).unwrap();
        assert!(queue.is_idle());
        queue.remove(id).unwrap();
        assert!(queue.jobs().is_empty());
    }

    #[test]
    fn cancel_all_marks_unfinished_jobs() {
        let mut queue = QueueManager::new();
        queue.enqueue(&device(), "Pixel", item(), Channel::Opp);
        queue.enqueue(&device(), "Pixel", item(), Channel::Opp);
        queue.cancel_all();
        assert!(queue.is_idle());
    }
}
