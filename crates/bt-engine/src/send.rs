use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use bt_api::{JobId, JobSnapshot, JobState, SendItem};
use bt_core::{
    hex_lower, sha256_file, BtError, BtErrorKind, Channel, Config, DeviceId, RecordKind, SendSource, TransferMode,
    ASSISTANT_SERVICE_UUID,
};
use bt_transport::BluetoothTransport;

use crate::cancel::CancelFlag;
use crate::client::send_file;
use crate::detect::{PresenceTracker, ProbeResult, ProbeStrategy};
use crate::journal::{Journal, JournalEntry, JournalKind, RENEW};
use crate::opp::{mime_for_name, push_file_with_retry, sleep_cancelable, OppOptions, OppRetryPolicy};
use crate::package::PendingPackageStore;
use crate::protocol::{Hello, MAX_CHUNK_SIZE};
use crate::queue::{QueueManager, SpeedTracker};

pub const ASSISTANT_BUSY_RETRIES: u32 = 2;
pub const ASSISTANT_BUSY_DELAY: Duration = Duration::from_secs(3);
pub const PRESENCE_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct OppSettings {
    pub protection: bt_core::ProtectionLevel,
    pub chunk_size: u32,
    pub connect_timeout: Duration,
    pub idle_timeout: Duration,
    pub retry_count: u32,
    pub retry_delay: Duration,
    pub name_use_bom: bool,
    pub auth_password: String,
}

impl Default for OppSettings {
    fn default() -> Self {
        Self {
            protection: bt_core::ProtectionLevel::Auto,
            chunk_size: 32_768,
            connect_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(300),
            retry_count: 3,
            retry_delay: Duration::from_secs(3),
            name_use_bom: false,
            auth_password: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SendSettings {
    pub mode: TransferMode,
    pub assistant_chunk_size: u32,
    pub assistant_idle_timeout: Duration,
    pub push_text_file_name: String,
    pub opp: OppSettings,
}

impl Default for SendSettings {
    fn default() -> Self {
        Self {
            mode: TransferMode::Auto,
            assistant_chunk_size: bt_core::DEFAULT_ASSISTANT_CHUNK_SIZE,
            assistant_idle_timeout: Duration::from_secs(300),
            push_text_file_name: bt_core::DEFAULT_PUSH_TEXT_FILE_NAME.to_string(),
            opp: OppSettings::default(),
        }
    }
}

impl SendSettings {
    pub fn from_config(config: &Config) -> Self {
        Self {
            mode: config.mode,
            assistant_chunk_size: config.assistant_chunk_size,
            assistant_idle_timeout: Duration::from_secs(u64::from(config.opp_send_idle_timeout_s)),
            push_text_file_name: config.push_text_file_name.clone(),
            opp: OppSettings {
                protection: config.opp_protection,
                chunk_size: config.opp_chunk_size,
                connect_timeout: Duration::from_secs(u64::from(config.opp_connect_timeout_s)),
                idle_timeout: Duration::from_secs(u64::from(config.opp_send_idle_timeout_s)),
                retry_count: config.opp_retry_count,
                retry_delay: Duration::from_secs(u64::from(config.opp_retry_delay_s)),
                name_use_bom: config.opp_name_bom,
                auth_password: String::new(),
            },
        }
    }

    pub fn with_password(mut self, password: Option<&str>) -> Self {
        self.opp.auth_password = password.unwrap_or_default().to_string();
        self
    }
}

#[derive(Debug, Clone)]
pub enum SendEvent {
    JobAdded(JobSnapshot),
    JobProgress(JobSnapshot),
    JobState(JobSnapshot),
    QueueIdle,
    Log { level: &'static str, message: String },
}

pub type SendSink = Arc<dyn Fn(SendEvent) + Send + Sync>;

struct ActiveControl {
    cancel: CancelFlag,
    desired: Mutex<Option<JobState>>,
}

impl ActiveControl {
    fn new() -> Self {
        Self {
            cancel: CancelFlag::new(),
            desired: Mutex::new(None),
        }
    }

    fn desired(&self) -> Option<JobState> {
        self.desired.lock().ok().and_then(|desired| *desired)
    }

    fn stop(&self, state: JobState) {
        if let Ok(mut desired) = self.desired.lock() {
            *desired = Some(state);
        }
        self.cancel.cancel();
    }
}

struct EngineState {
    queue: QueueManager,
    shutdown: bool,
    active: Option<(JobId, Arc<ActiveControl>)>,
}

struct Payload {
    path: PathBuf,
    display_name: String,
    journal_kind: JournalKind,
    produced: bool,
    size: u64,
}

#[derive(Debug, Default)]
struct Execution {
    ok: bool,
    canceled: bool,
    error: Option<String>,
    bytes_sent: u64,
    resumable: bool,
}

enum JournalFinal {
    Complete,
    Release,
    Pause,
}

struct LeaseHeartbeat<'a> {
    journal: &'a Journal,
    id: &'a str,
    interval: Duration,
    last: Instant,
}

impl<'a> LeaseHeartbeat<'a> {
    fn new(journal: &'a Journal, id: &'a str) -> Self {
        Self {
            journal,
            id,
            interval: RENEW,
            last: Instant::now(),
        }
    }

    fn observe(&mut self, now: Instant, offset: u64) -> Result<bool, BtError> {
        if now.saturating_duration_since(self.last) < self.interval {
            return Ok(false);
        }
        self.last = now;
        self.journal.heartbeat(self.id, offset)
    }
}

/// 顺序发送引擎：通道选择、助手断点续传、OPP 重试与逐任务控制。
pub struct SendEngine {
    transport: Arc<dyn BluetoothTransport>,
    settings: Mutex<SendSettings>,
    journal: Journal,
    packages: PendingPackageStore,
    sink: SendSink,
    presence: PresenceTracker,
    state: Mutex<EngineState>,
    wake: Condvar,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    closing: AtomicBool,
}

impl std::fmt::Debug for SendEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SendEngine").finish_non_exhaustive()
    }
}

impl SendEngine {
    pub fn new(
        transport: Arc<dyn BluetoothTransport>,
        journal: Journal,
        packages: PendingPackageStore,
        settings: SendSettings,
        sink: SendSink,
    ) -> Result<Arc<Self>, BtError> {
        let engine = Arc::new(Self {
            transport,
            settings: Mutex::new(settings),
            journal,
            packages,
            sink,
            presence: PresenceTracker::new(PRESENCE_TTL),
            state: Mutex::new(EngineState {
                queue: QueueManager::new(),
                shutdown: false,
                active: None,
            }),
            wake: Condvar::new(),
            worker: Mutex::new(None),
            closing: AtomicBool::new(false),
        });
        let worker_engine = engine.clone();
        let handle = std::thread::Builder::new()
            .name("bt-send".to_string())
            .spawn(move || worker_engine.worker_loop())
            .map_err(|error| BtError::internal(format!("发送线程创建失败：{error}")))?;
        if let Ok(mut slot) = engine.worker.lock() {
            *slot = Some(handle);
        }
        Ok(engine)
    }

    pub fn update_settings(&self, settings: SendSettings) {
        if let Ok(mut current) = self.settings.lock() {
            *current = settings;
        }
    }

    pub fn settings(&self) -> SendSettings {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    pub fn enqueue(
        &self,
        device: &DeviceId,
        device_name: &str,
        items: Vec<SendItem>,
        channel: Option<Channel>,
    ) -> Result<Vec<JobId>, BtError> {
        if items.is_empty() {
            return Err(BtError::invalid("发送条目为空"));
        }
        for item in &items {
            self.ensure_sendable_source(item)?;
        }
        let mut added = Vec::new();
        {
            let mut state = self.state.lock().map_err(|_| BtError::internal("发送队列被投毒"))?;
            for item in items {
                let id = state.queue.enqueue(device, device_name, item, channel);
                added.push(id);
            }
        }
        self.wake.notify_all();
        for id in &added {
            if let Some(snapshot) = self.snapshot(*id) {
                self.emit(SendEvent::JobAdded(snapshot));
            }
        }
        Ok(added)
    }

    /// 用户源文件位于 pending 时拒绝入队；该目录只容纳本应用生成的续传载荷。
    fn ensure_sendable_source(&self, item: &SendItem) -> Result<(), BtError> {
        let paths: Vec<&std::path::Path> = match &item.source {
            SendSource::File(path) => vec![path.as_path()],
            SendSource::Files(paths) => paths.iter().map(std::path::PathBuf::as_path).collect(),
            SendSource::Folder(_) | SendSource::Text(_) => Vec::new(),
        };
        for path in paths {
            if !item.produced && !item.recovered && self.packages.contains(path) {
                return Err(BtError::invalid(format!(
                    "源文件位于待续传目录 pending/：{}；请把源文件移出该目录后再发送",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    pub fn jobs(&self) -> Vec<JobSnapshot> {
        self.state
            .lock()
            .map(|state| state.queue.snapshot())
            .unwrap_or_default()
    }

    pub fn snapshot(&self, id: JobId) -> Option<JobSnapshot> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.queue.get(id).map(|job| job.snapshot()))
    }

    pub fn pause_job(&self, id: JobId) -> Result<(), BtError> {
        let active = {
            let mut state = self.state.lock().map_err(|_| BtError::internal("发送队列被投毒"))?;
            if let Some((active_id, control)) = state.active.as_ref() {
                if *active_id == id {
                    control.stop(JobState::Paused);
                    return Ok(());
                }
            }
            let snapshot = state.queue.pause(id)?;
            Some(snapshot)
        };
        if let Some(snapshot) = active {
            self.emit(SendEvent::JobState(snapshot));
        }
        Ok(())
    }

    pub fn resume_job(&self, id: JobId) -> Result<(), BtError> {
        let snapshot = {
            let mut state = self.state.lock().map_err(|_| BtError::internal("发送队列被投毒"))?;
            state.queue.resume(id)?
        };
        self.emit(SendEvent::JobState(snapshot));
        self.wake.notify_all();
        Ok(())
    }

    pub fn cancel_job(&self, id: JobId) -> Result<(), BtError> {
        {
            let mut state = self.state.lock().map_err(|_| BtError::internal("发送队列被投毒"))?;
            if let Some((active_id, control)) = state.active.as_ref() {
                if *active_id == id {
                    control.stop(JobState::Canceled);
                    return Ok(());
                }
            }
            let snapshot = state.queue.cancel(id)?;
            drop(state);
            self.emit(SendEvent::JobState(snapshot));
        }
        Ok(())
    }

    pub fn cancel_all_jobs(&self) -> Result<(), BtError> {
        let snapshots = {
            let mut state = self.state.lock().map_err(|_| BtError::internal("发送队列被投毒"))?;
            let active = state.active.as_ref().map(|(id, _)| *id);
            if let Some((_, control)) = state.active.as_ref() {
                control.stop(JobState::Canceled);
            }
            state.queue.cancel_unfinished_except(active)
        };
        for snapshot in snapshots {
            self.emit(SendEvent::JobState(snapshot));
        }
        self.wake.notify_all();
        Ok(())
    }

    pub fn retry_job(&self, id: JobId) -> Result<(), BtError> {
        let snapshot = {
            let mut state = self.state.lock().map_err(|_| BtError::internal("发送队列被投毒"))?;
            state.queue.retry(id)?
        };
        self.emit(SendEvent::JobState(snapshot));
        self.wake.notify_all();
        Ok(())
    }

    pub fn remove_job(&self, id: JobId) -> Result<(), BtError> {
        let mut state = self.state.lock().map_err(|_| BtError::internal("发送队列被投毒"))?;
        let snapshot = state.queue.remove(id)?;
        drop(state);
        self.emit(SendEvent::JobState(snapshot));
        Ok(())
    }

    /// 探测助手通道并更新三态缓存；返回结论与实际通道。
    pub fn probe_device(&self, device: &DeviceId) -> (ProbeResult, Channel) {
        let mode = self.settings().mode;
        match self.transport.find_service(device, ASSISTANT_SERVICE_UUID) {
            Ok(Some(_)) => {
                let result = self.presence.record(device, ProbeResult::available());
                (result, ProbeStrategy::default().decide(mode, true))
            }
            Ok(None) => {
                let result = self.presence.record(device, ProbeResult::absent());
                (result, ProbeStrategy::default().decide(mode, false))
            }
            Err(error) => {
                let result = self.presence.record(device, ProbeResult::failed(error.to_string()));
                let available = self.presence.current(device).is_available();
                (result, ProbeStrategy::default().decide(mode, available))
            }
        }
    }

    /// 启动恢复：放弃过期条目并把可续传条目重新入队。
    pub fn resume_pending(&self) -> Result<usize, BtError> {
        let plan = self.journal.plan_recovery()?;
        if !plan.abandoned.is_empty() {
            let ids: Vec<String> = plan.abandoned.iter().map(|item| item.entry.id_hex()).collect();
            let _ = self.journal.apply_abandoned(&ids);
            for item in &plan.abandoned {
                self.emit(SendEvent::Log {
                    level: "WARN",
                    message: format!("放弃未完成传输：{}（{}）", item.entry.display_name, item.reason),
                });
            }
        }
        let mut count = 0usize;
        for entry in plan.recoverable {
            let kind = match entry.kind {
                JournalKind::File => RecordKind::File,
                JournalKind::FileZip => RecordKind::File,
                JournalKind::Folder => RecordKind::Folder,
                JournalKind::Text => RecordKind::Text,
            };
            let item = SendItem {
                source: SendSource::File(entry.payload.clone()),
                display_name: entry.display_name.clone(),
                total_bytes: Some(entry.size),
                kind,
                produced: entry.produced,
                recovered: true,
            };
            let device_name = entry.peer.to_string();
            if self
                .enqueue(&entry.peer, &device_name, vec![item], Some(Channel::Assistant))
                .is_ok()
            {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn resume_entries(&self) -> Result<Vec<JournalEntry>, BtError> {
        self.journal.read_all()
    }

    pub fn cancel_resume(&self, id: &str) -> Result<bool, BtError> {
        self.journal.cancel(id)
    }

    pub fn shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return,
        };
        state.shutdown = true;
        if let Some((_, control)) = state.active.as_ref() {
            control.stop(JobState::Canceled);
        }
        drop(state);
        self.wake.notify_all();
    }

    fn emit(&self, event: SendEvent) {
        (self.sink)(event);
    }

    fn worker_loop(self: Arc<Self>) {
        loop {
            let picked = {
                let mut state = match self.state.lock() {
                    Ok(state) => state,
                    Err(_) => return,
                };
                loop {
                    if state.shutdown {
                        return;
                    }
                    let next = state.queue.next_runnable().map(|job| job.id);
                    match next {
                        Some(id) => {
                            if let Ok(snapshot) = state.queue.mark_connecting(id) {
                                let job = state.queue.get(id).cloned();
                                break job.map(|job| (job, snapshot));
                            }
                            continue;
                        }
                        None => {
                            state = match self.wake.wait(state) {
                                Ok(state) => state,
                                Err(_) => return,
                            };
                        }
                    }
                }
            };
            let Some((job, connecting)) = picked else {
                continue;
            };
            self.emit(SendEvent::JobState(connecting));
            let control = Arc::new(ActiveControl::new());
            if let Ok(mut state) = self.state.lock() {
                state.active = Some((job.id, control.clone()));
            }
            let execution = self.execute(&job, &control);
            let desired = control.desired();
            let (final_state, error, resumable) = match desired {
                Some(JobState::Paused) => (JobState::Paused, None, true),
                Some(JobState::Canceled) => (
                    JobState::Canceled,
                    Some(execution.error.clone().unwrap_or_else(|| "用户取消".to_string())),
                    execution.resumable,
                ),
                _ if execution.ok => (JobState::Completed, None, false),
                _ if execution.canceled => (JobState::Canceled, Some("用户取消".to_string()), execution.resumable),
                _ => (
                    JobState::Failed,
                    execution.error.clone().or_else(|| Some("发送失败".to_string())),
                    execution.resumable,
                ),
            };
            let finished = {
                let mut state = match self.state.lock() {
                    Ok(state) => state,
                    Err(_) => return,
                };
                state.active = None;
                let result = state
                    .queue
                    .finish(job.id, final_state, error, execution.bytes_sent, resumable);
                let idle = state.queue.is_idle();
                result.ok().map(|snapshot| (snapshot, idle))
            };
            if let Some((snapshot, idle)) = finished {
                self.emit(SendEvent::JobState(snapshot));
                if idle {
                    self.emit(SendEvent::QueueIdle);
                }
            }
        }
    }

    fn execute(&self, job: &crate::queue::QueueJob, control: &ActiveControl) -> Execution {
        let settings = self.settings();
        let (channel, fallback_reason) = self.resolve_channel(job, &settings);
        if let Some(reason) = fallback_reason {
            self.emit(SendEvent::Log {
                level: "WARN",
                message: format!("{} 回退 OPP：{reason}", job.device_name),
            });
        }
        if let Some(snapshot) = self.update_channel(job.id, channel) {
            self.emit(SendEvent::JobState(snapshot));
        }
        let payloads = match self.build_payloads(job, &settings) {
            Ok(payloads) => payloads,
            Err(error) => {
                return Execution {
                    ok: false,
                    canceled: false,
                    error: Some(error.to_string()),
                    bytes_sent: 0,
                    resumable: false,
                };
            }
        };
        let total: u64 = payloads.iter().map(|payload| payload.size).sum();
        if let Some(snapshot) = self.update_total(job.id, total) {
            self.emit(SendEvent::JobState(snapshot));
        }
        let tracker = SpeedTracker::new(5_000);
        let mut completed = 0u64;
        let mut resumable = false;
        for payload in &payloads {
            if control.cancel.is_canceled() {
                self.cleanup_unkept(payload);
                return Execution {
                    ok: false,
                    canceled: true,
                    error: Some("用户取消".to_string()),
                    bytes_sent: completed,
                    resumable,
                };
            }
            match channel {
                Channel::Assistant => {
                    let result = self.send_assistant_payload(job, payload, &settings, control, &tracker, completed);
                    match result {
                        Ok(result) => {
                            completed += result.bytes;
                            if result.ok {
                                continue;
                            }
                            if result.canceled {
                                return Execution {
                                    ok: false,
                                    canceled: true,
                                    error: result.error,
                                    bytes_sent: completed,
                                    resumable: result.resumable,
                                };
                            }
                            return Execution {
                                ok: false,
                                canceled: false,
                                error: result.error,
                                bytes_sent: completed,
                                resumable: result.resumable,
                            };
                        }
                        Err(error) => {
                            return Execution {
                                ok: false,
                                canceled: error.kind() == BtErrorKind::Canceled,
                                error: Some(error.to_string()),
                                bytes_sent: completed,
                                resumable,
                            };
                        }
                    }
                }
                Channel::Opp => {
                    let result = self.send_opp_payload(job, payload, &settings, control, &tracker, completed);
                    self.cleanup_opp_artifact(payload);
                    match result {
                        Ok(bytes) => {
                            completed += bytes;
                            if let Some(snapshot) = self.update_progress(job.id, completed, tracker.bps()) {
                                self.emit(SendEvent::JobProgress(snapshot));
                            }
                        }
                        Err(error) => {
                            return Execution {
                                ok: false,
                                canceled: error.kind() == BtErrorKind::Canceled,
                                error: Some(error.to_string()),
                                bytes_sent: completed,
                                resumable,
                            };
                        }
                    }
                }
            }
            resumable = false;
        }
        Execution {
            ok: true,
            canceled: false,
            error: None,
            bytes_sent: completed,
            resumable: false,
        }
    }

    fn resolve_channel(&self, job: &crate::queue::QueueJob, settings: &SendSettings) -> (Channel, Option<String>) {
        if let Some(forced) = job.requested_channel {
            return (forced, None);
        }
        match settings.mode {
            TransferMode::Assistant => (Channel::Assistant, None),
            TransferMode::Opp => (Channel::Opp, None),
            TransferMode::Auto => {
                if self.presence.current(&job.device).is_available() {
                    return (Channel::Assistant, None);
                }
                match self.transport.find_service(&job.device, ASSISTANT_SERVICE_UUID) {
                    Ok(Some(_)) => {
                        self.presence.record(&job.device, ProbeResult::available());
                        (Channel::Assistant, None)
                    }
                    Ok(None) => {
                        self.presence.record(&job.device, ProbeResult::absent());
                        (Channel::Opp, Some("对端未声明助手服务".to_string()))
                    }
                    Err(error) => {
                        self.presence
                            .record(&job.device, ProbeResult::failed(error.to_string()));
                        (Channel::Opp, Some(format!("助手探测失败：{error}")))
                    }
                }
            }
        }
    }

    fn build_payloads(&self, job: &crate::queue::QueueJob, settings: &SendSettings) -> Result<Vec<Payload>, BtError> {
        let mut payloads = Vec::new();
        match &job.item.source {
            SendSource::File(path) => {
                let size = file_size(path)?;
                payloads.push(Payload {
                    path: path.clone(),
                    display_name: job.item.display_name.clone(),
                    journal_kind: JournalKind::File,
                    produced: job.item.produced && self.packages.contains(path),
                    size,
                });
            }
            SendSource::Files(paths) => {
                for path in paths {
                    let size = file_size(path)?;
                    let name = path
                        .file_name()
                        .map(|value| value.to_string_lossy().into_owned())
                        .unwrap_or_else(|| job.item.display_name.clone());
                    payloads.push(Payload {
                        path: path.clone(),
                        display_name: name,
                        journal_kind: JournalKind::File,
                        produced: false,
                        size,
                    });
                }
            }
            SendSource::Folder(path) => {
                if !path.is_dir() {
                    return Err(BtError::not_found(format!("文件夹不存在：{}", path.display())));
                }
                let packed = self.packages.create_zip_from_folder(path)?;
                let size = file_size(&packed.path)?;
                payloads.push(Payload {
                    path: packed.path,
                    display_name: packed.display_name,
                    journal_kind: JournalKind::Folder,
                    produced: true,
                    size,
                });
            }
            SendSource::Text(text) => {
                let packed = self.packages.create_text_file(text, &settings.push_text_file_name)?;
                let size = file_size(&packed.path)?;
                payloads.push(Payload {
                    path: packed.path,
                    display_name: packed.display_name,
                    journal_kind: JournalKind::Text,
                    produced: true,
                    size,
                });
            }
        }
        if payloads.is_empty() {
            return Err(BtError::invalid("没有可发送的载荷"));
        }
        Ok(payloads)
    }

    fn send_assistant_payload(
        &self,
        job: &crate::queue::QueueJob,
        payload: &Payload,
        settings: &SendSettings,
        control: &ActiveControl,
        tracker: &SpeedTracker,
        base: u64,
    ) -> Result<PayloadOutcome, BtError> {
        let sha = sha256_file(&payload.path)?;
        let chunk = settings
            .assistant_chunk_size
            .clamp(bt_core::MIN_ASSISTANT_CHUNK_SIZE, MAX_CHUNK_SIZE);
        if payload.size > i64::MAX as u64 {
            return Err(BtError::invalid("文件超过助手协议单文件上限"));
        }
        let mut attempts = 0u32;
        loop {
            if control.cancel.is_canceled() {
                return Ok(PayloadOutcome::canceled(true));
            }
            let journal_entry = JournalEntry::new(
                job.device,
                payload.display_name.clone(),
                payload.path.clone(),
                sha,
                payload.size,
                payload.journal_kind,
                payload.produced,
            );
            let journal_id = match self.journal.begin(journal_entry) {
                Ok(result) if result.busy => {
                    return Ok(PayloadOutcome::failed("同一传输正在其他进程发送", false));
                }
                Ok(result) => Some(result.entry.id_hex()),
                Err(error) => {
                    self.emit(SendEvent::Log {
                        level: "WARN",
                        message: format!("journal 无法记录，继续发送：{error}"),
                    });
                    None
                }
            };
            let mut lease = journal_id.as_deref().map(|id| LeaseHeartbeat::new(&self.journal, id));
            let mut stream = match self.transport.connect_service(&job.device, ASSISTANT_SERVICE_UUID) {
                Ok(stream) => stream,
                Err(error) => {
                    self.finalize_journal(journal_id.as_deref(), JournalFinal::Release);
                    return Ok(PayloadOutcome::failed(error.to_string(), journal_id.is_some()));
                }
            };
            let hello = Hello::new(&payload.display_name, payload.size as i64, hex_lower(&sha), chunk);
            let engine = self;
            let job_id = job.id;
            let mut progress = |sent: u64, _total: u64| {
                tracker.add_sample(base + sent.min(payload.size));
                if let Some(snapshot) = engine.update_progress(job_id, base + sent.min(payload.size), tracker.bps()) {
                    engine.emit(SendEvent::JobProgress(snapshot));
                }
                if let Some(lease) = lease.as_mut() {
                    match lease.observe(Instant::now(), sent.min(payload.size)) {
                        Ok(_) => {}
                        Err(error) => engine.emit(SendEvent::Log {
                            level: "WARN",
                            message: format!("journal 续租失败：{error}"),
                        }),
                    }
                }
            };
            let result = send_file(
                stream.as_mut(),
                &hello,
                &payload.path,
                &control.cancel,
                settings.assistant_idle_timeout,
                &mut progress,
            );
            match result {
                Ok(outcome) if outcome.ok => {
                    self.finalize_journal(journal_id.as_deref(), JournalFinal::Complete);
                    return Ok(PayloadOutcome {
                        ok: true,
                        canceled: false,
                        bytes: payload.size,
                        error: None,
                        resumable: false,
                    });
                }
                Ok(outcome) if outcome.canceled => {
                    self.finalize_journal(journal_id.as_deref(), JournalFinal::Pause);
                    return Ok(PayloadOutcome::canceled(journal_id.is_some()));
                }
                Ok(outcome) => {
                    let busy = outcome.retryable && outcome.error.as_deref().unwrap_or("").contains("忙");
                    if busy && attempts < ASSISTANT_BUSY_RETRIES && !control.cancel.is_canceled() {
                        attempts += 1;
                        self.finalize_journal(journal_id.as_deref(), JournalFinal::Release);
                        self.emit(SendEvent::Log {
                            level: "WARN",
                            message: format!(
                                "接收端忙，{} 秒后重试（{}/{}）",
                                ASSISTANT_BUSY_DELAY.as_secs(),
                                attempts,
                                ASSISTANT_BUSY_RETRIES
                            ),
                        });
                        if sleep_cancelable(ASSISTANT_BUSY_DELAY, &control.cancel).is_err() {
                            return Ok(PayloadOutcome::canceled(true));
                        }
                        continue;
                    }
                    self.finalize_journal(journal_id.as_deref(), JournalFinal::Release);
                    return Ok(PayloadOutcome::failed(
                        outcome.error.unwrap_or_else(|| "发送失败".to_string()),
                        journal_id.is_some(),
                    ));
                }
                Err(error) => {
                    let stopped = control.cancel.is_canceled() || error.kind() == BtErrorKind::Canceled;
                    let action = if stopped {
                        JournalFinal::Pause
                    } else {
                        JournalFinal::Release
                    };
                    self.finalize_journal(journal_id.as_deref(), action);
                    if stopped {
                        return Ok(PayloadOutcome::canceled(journal_id.is_some()));
                    }
                    return Ok(PayloadOutcome::failed(error.to_string(), journal_id.is_some()));
                }
            }
        }
    }

    fn send_opp_payload(
        &self,
        job: &crate::queue::QueueJob,
        payload: &Payload,
        settings: &SendSettings,
        control: &ActiveControl,
        tracker: &SpeedTracker,
        base: u64,
    ) -> Result<u64, BtError> {
        let policy = OppRetryPolicy::new(settings.opp.retry_count, settings.opp.retry_delay);
        let options = OppOptions {
            protection: settings.opp.protection,
            chunk_size: settings.opp.chunk_size,
            connect_timeout: settings.opp.connect_timeout,
            idle_timeout: settings.opp.idle_timeout,
            name_use_bom: settings.opp.name_use_bom,
            auth_password: settings.opp.auth_password.clone(),
        };
        let engine = self;
        let job_id = job.id;
        let mut progress = |sent: u64, _total: u64| {
            tracker.add_sample(base + sent.min(payload.size));
            if let Some(snapshot) = engine.update_progress(job_id, base + sent.min(payload.size), tracker.bps()) {
                engine.emit(SendEvent::JobProgress(snapshot));
            }
        };
        let mut on_retry = |attempt: u32, error: &BtError, delay: Duration| {
            engine.emit(SendEvent::Log {
                level: "WARN",
                message: format!(
                    "OPP 推送失败（{}）：{}；{} 秒后重试（{}）",
                    payload.display_name,
                    error.message(),
                    delay.as_secs(),
                    attempt
                ),
            });
        };
        let mime = mime_for_name(&payload.display_name);
        push_file_with_retry(
            self.transport.as_ref() as &dyn bt_transport::RfcommConnector,
            &job.device,
            &payload.path,
            &payload.display_name,
            mime,
            &options,
            &policy,
            &control.cancel,
            &mut progress,
            &mut on_retry,
        )
        .map(|result| result.bytes_sent)
    }

    fn finalize_journal(&self, id: Option<&str>, action: JournalFinal) {
        let Some(id) = id else {
            return;
        };
        let result = match action {
            JournalFinal::Complete => self.journal.complete(id),
            JournalFinal::Release => self.journal.release(id, true),
            JournalFinal::Pause => self.journal.pause(id),
        };
        if let Err(error) = result {
            self.emit(SendEvent::Log {
                level: "WARN",
                message: format!("journal 更新失败：{error}"),
            });
        }
    }

    fn cleanup_unkept(&self, payload: &Payload) {
        if payload.produced {
            let _ = self.packages.delete(&payload.path);
        }
    }

    fn cleanup_opp_artifact(&self, payload: &Payload) {
        if payload.produced {
            let _ = self.packages.delete(&payload.path);
        }
    }

    fn update_channel(&self, id: JobId, channel: Channel) -> Option<JobSnapshot> {
        self.state
            .lock()
            .ok()
            .and_then(|mut state| state.queue.set_channel(id, channel).ok())
    }

    fn update_total(&self, id: JobId, total: u64) -> Option<JobSnapshot> {
        self.state
            .lock()
            .ok()
            .and_then(|mut state| state.queue.set_total(id, total).ok())
    }

    fn update_progress(&self, id: JobId, bytes: u64, bps: u64) -> Option<JobSnapshot> {
        self.state
            .lock()
            .ok()
            .and_then(|mut state| state.queue.set_progress(id, bytes, bps).ok())
    }
}

struct PayloadOutcome {
    ok: bool,
    canceled: bool,
    bytes: u64,
    error: Option<String>,
    resumable: bool,
}

impl PayloadOutcome {
    fn canceled(resumable: bool) -> Self {
        Self {
            ok: false,
            canceled: true,
            bytes: 0,
            error: Some("用户取消".to_string()),
            resumable,
        }
    }

    fn failed(message: impl Into<String>, resumable: bool) -> Self {
        Self {
            ok: false,
            canceled: false,
            bytes: 0,
            error: Some(message.into()),
            resumable,
        }
    }
}

fn file_size(path: &std::path::Path) -> Result<u64, BtError> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| BtError::not_found(format!("文件不可读：{}（{error}）", path.display())))?;
    Ok(metadata.len())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Journal, JournalState, LEASE};
    use crate::package::PendingPackageStore;
    use bt_api::SendItem;
    use bt_core::SendSource;
    use bt_transport::mock::MockTransport;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "bt-send-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_engine(tag: &str) -> (Arc<SendEngine>, Arc<MockTransport>, TempDir, PathBuf) {
        let dir = TempDir::new(tag);
        let source = dir.0.join("payload.bin");
        std::fs::write(&source, b"payload").unwrap();
        let transport = Arc::new(MockTransport::new(Vec::new()));
        let journal = Journal::new(dir.0.join("send-journal.bin"));
        let packages = PendingPackageStore::new(dir.0.join("pending"));
        let settings = SendSettings {
            mode: TransferMode::Assistant,
            assistant_idle_timeout: std::time::Duration::from_millis(200),
            ..SendSettings::default()
        };
        let engine = SendEngine::new(transport.clone(), journal, packages, settings, Arc::new(|_event| {})).unwrap();
        (engine, transport, dir, source)
    }

    fn enqueue(engine: &Arc<SendEngine>, source: &std::path::Path) -> JobId {
        let item = SendItem::from_source(SendSource::File(source.to_path_buf()));
        engine
            .enqueue(
                &DeviceId::parse("AA:BB:CC:DD:EE:20").unwrap(),
                "Pixel",
                vec![item],
                Some(Channel::Assistant),
            )
            .unwrap()[0]
    }

    fn wait_state(engine: &Arc<SendEngine>, id: JobId, state: JobState) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if engine.snapshot(id).map(|job| job.state) == Some(state) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn pause_during_connect_marks_job_paused() {
        let (engine, transport, _dir, source) = make_engine("pause");
        let gate = transport.gate_next_connect();
        let id = enqueue(&engine, &source);
        assert!(wait_state(&engine, id, JobState::Connecting));
        engine.pause_job(id).unwrap();
        gate.release();
        assert!(
            wait_state(&engine, id, JobState::Paused),
            "实际状态 {:?}",
            engine.snapshot(id)
        );
        assert!(engine.snapshot(id).unwrap().resumable);
        engine.resume_job(id).unwrap();
        assert!(
            wait_state(&engine, id, JobState::Queued)
                || wait_state(&engine, id, JobState::Failed)
                || wait_state(&engine, id, JobState::Connecting)
        );
        engine.shutdown();
    }

    #[test]
    fn cancel_during_connect_marks_job_canceled() {
        let (engine, transport, _dir, source) = make_engine("cancel");
        let gate = transport.gate_next_connect();
        let id = enqueue(&engine, &source);
        assert!(wait_state(&engine, id, JobState::Connecting));
        engine.cancel_job(id).unwrap();
        gate.release();
        assert!(
            wait_state(&engine, id, JobState::Canceled),
            "实际状态 {:?}",
            engine.snapshot(id)
        );
        engine.shutdown();
    }

    #[test]
    fn cancel_preserves_paused_journal_entry() {
        let (engine, transport, dir, _source) = make_engine("cancel-journal");
        let gate = transport.gate_next_connect();
        let item = SendItem::from_source(SendSource::Text("resume me".to_string()));
        let id = engine
            .enqueue(
                &DeviceId::parse("AA:BB:CC:DD:EE:20").unwrap(),
                "Pixel",
                vec![item],
                Some(Channel::Assistant),
            )
            .unwrap()[0];
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let entries = engine.resume_entries().unwrap();
            if entries.len() == 1 && entries[0].state == JournalState::Sending {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "journal 条目未写入");
            std::thread::sleep(Duration::from_millis(20));
        }
        engine.cancel_all_jobs().unwrap();
        gate.release();
        assert!(wait_state(&engine, id, JobState::Canceled));
        let entries = engine.resume_entries().unwrap();
        assert_eq!(entries.len(), 1, "取消后必须保留 journal 条目");
        assert_eq!(entries[0].state, JournalState::Paused);
        assert!(entries[0].payload.is_file(), "取消后必须保留可续传载荷");
        assert!(entries[0].validate_payload().is_ok(), "取消后的条目必须可续传");
        let journal = Journal::new(dir.0.join("send-journal.bin"));
        assert_eq!(journal.read_all().unwrap().len(), 1);
        engine.shutdown();
    }

    struct PendingArtifactCleaner(PendingPackageStore);

    impl crate::journal::ArtifactCleaner for PendingArtifactCleaner {
        fn delete(&self, path: &std::path::Path) -> bool {
            self.0.delete(path)
        }
    }

    fn make_zip_engine(
        tag: &str,
    ) -> (
        Arc<SendEngine>,
        Arc<MockTransport>,
        TempDir,
        crate::package::PackedSource,
    ) {
        let dir = TempDir::new(tag);
        let pending = dir.0.join("pending");
        let source = dir.0.join("photo.bin");
        std::fs::write(&source, b"zip-source").unwrap();
        let packages = PendingPackageStore::new(pending.clone());
        let packed = packages.create_zip_from_file(&source).unwrap();
        let transport = Arc::new(MockTransport::new(Vec::new()));
        let journal = Journal::new(dir.0.join("send-journal.bin"))
            .with_base(&pending)
            .with_cleaner(Arc::new(PendingArtifactCleaner(packages.clone())));
        let settings = SendSettings {
            mode: TransferMode::Assistant,
            assistant_idle_timeout: std::time::Duration::from_millis(500),
            ..SendSettings::default()
        };
        let engine = SendEngine::new(transport.clone(), journal, packages, settings, Arc::new(|_event| {})).unwrap();
        (engine, transport, dir, packed)
    }

    fn wait_journal_sending(engine: &Arc<SendEngine>) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let entries = engine.resume_entries().unwrap();
            if entries.len() == 1 && entries[0].state == JournalState::Sending {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "journal 条目未写入：{entries:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn cancel_keeps_produced_zip_until_journal_terminal_state() {
        let (engine, transport, dir, packed) = make_zip_engine("zip-cancel");
        let gate = transport.gate_next_connect();
        let mut item = SendItem::from_source(SendSource::File(packed.path.clone()));
        item.produced = true;
        let id = engine
            .enqueue(
                &DeviceId::parse("AA:BB:CC:DD:EE:20").unwrap(),
                "Pixel",
                vec![item],
                Some(Channel::Assistant),
            )
            .unwrap()[0];
        wait_journal_sending(&engine);
        engine.cancel_all_jobs().unwrap();
        gate.release();
        assert!(wait_state(&engine, id, JobState::Canceled));
        let entries = engine.resume_entries().unwrap();
        assert_eq!(entries.len(), 1, "取消后 journal 条目必须保留");
        assert_eq!(entries[0].state, JournalState::Paused);
        assert!(entries[0].produced, "zip 应为自产载荷");
        assert!(packed.path.is_file(), "取消后自产 zip 必须保留以便续传");

        let journal = Journal::new(dir.0.join("send-journal.bin"))
            .with_base(dir.0.join("pending"))
            .with_cleaner(Arc::new(PendingArtifactCleaner(PendingPackageStore::new(
                dir.0.join("pending"),
            ))));
        journal.cancel(&entries[0].id_hex()).unwrap();
        assert!(!packed.path.exists(), "显式取消后清理自产 zip");
        engine.shutdown();
    }

    #[test]
    fn completed_produced_zip_is_removed_with_journal_entry() {
        let (engine, transport, _dir, packed) = make_zip_engine("zip-complete");
        let content = std::fs::read(&packed.path).unwrap();
        let sha = bt_core::hex_lower(&bt_core::sha256(&content));
        let mut inbound = Vec::new();
        inbound.extend_from_slice(
            &crate::protocol::encode_offer(&crate::protocol::Offer::accept(0))
                .unwrap()
                .encode(),
        );
        inbound.extend_from_slice(&crate::protocol::encode_ack(content.len() as i64).unwrap().encode());
        inbound.extend_from_slice(
            &crate::protocol::encode_done(&crate::protocol::Done::new(true, sha))
                .unwrap()
                .encode(),
        );
        transport.script_connect(inbound);
        let mut item = SendItem::from_source(SendSource::File(packed.path.clone()));
        item.produced = true;
        let id = engine
            .enqueue(
                &DeviceId::parse("AA:BB:CC:DD:EE:20").unwrap(),
                "Pixel",
                vec![item],
                Some(Channel::Assistant),
            )
            .unwrap()[0];
        assert!(wait_state(&engine, id, JobState::Completed));
        assert!(engine.resume_entries().unwrap().is_empty(), "完成后 journal 条目应删除");
        assert!(!packed.path.exists(), "完成后自产 zip 应清理");
        engine.shutdown();
    }

    #[test]
    fn enqueue_rejects_user_file_inside_pending() {
        let (engine, _transport, dir, _source) = make_engine("pending-guard");
        let pending = dir.0.join("pending");
        std::fs::create_dir_all(&pending).unwrap();
        let user_file = pending.join("user-source.bin");
        std::fs::write(&user_file, b"user data").unwrap();
        let item = SendItem::from_source(SendSource::File(user_file.clone()));
        let error = engine
            .enqueue(
                &DeviceId::parse("AA:BB:CC:DD:EE:20").unwrap(),
                "Pixel",
                vec![item],
                Some(Channel::Assistant),
            )
            .unwrap_err();
        assert_eq!(error.kind(), BtErrorKind::InvalidArgument);
        assert!(error.message().contains("pending"), "{}", error.message());
        assert!(user_file.is_file(), "拒绝入队不得删除源文件");
        engine.shutdown();
    }

    #[test]
    fn lease_heartbeat_renews_after_interval() {
        let dir = TempDir::new("heartbeat");
        let source = dir.0.join("payload.bin");
        std::fs::write(&source, b"heartbeat-payload").unwrap();
        let now = Arc::new(Mutex::new(1_000u64));
        let clock = {
            let now = now.clone();
            Arc::new(move || *now.lock().unwrap())
        };
        let journal = Journal::new(dir.0.join("send-journal.bin")).with_clock(clock);
        let content = std::fs::read(&source).unwrap();
        let entry = JournalEntry::new(
            DeviceId::parse("AA:BB:CC:DD:EE:20").unwrap(),
            "payload.bin",
            source,
            bt_core::sha256(&content),
            content.len() as u64,
            JournalKind::File,
            false,
        );
        let id = journal.begin(entry).unwrap().entry.id_hex();
        let mut heartbeat = LeaseHeartbeat::new(&journal, &id);
        let base = Instant::now();
        assert!(!heartbeat
            .observe(base + Duration::from_secs(RENEW.as_secs() - 1), 10)
            .unwrap());
        assert_eq!(journal.read_all().unwrap()[0].lease_until, 1_000 + LEASE.as_secs());
        *now.lock().unwrap() = 1_000 + RENEW.as_secs();
        assert!(heartbeat.observe(base + RENEW + Duration::from_secs(1), 10).unwrap());
        let stored = journal.read_all().unwrap()[0].clone();
        assert_eq!(stored.resume_offset, 10);
        assert_eq!(stored.lease_until, 1_000 + RENEW.as_secs() + LEASE.as_secs());
    }
}
