use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use bt_core::{
    now_rfc3339, paths, AssistantState, BtError, Channel, Config, DeviceId, Direction, Record, RecordStatus,
    TransferMode,
};
use crossbeam_channel::Receiver;

use crate::backend::Backend;
use crate::bus::EventBus;
use crate::dto::{
    AskId, ConfigPatch, CopyItem, CopyKind, CopyPlan, DayStat, Device, ExportFormat, ExportSummary, JobId, JobSnapshot,
    JobState, LogLevel, MigrateOptions, MigrateReport, PinPromptKind, PinRequest, PinRequestId, ReceiveOptions,
    ReceiveSession, RecordQuery, ScanOptions, SelftestReport, SendItem, Stats,
};
use crate::event::Event;

/// 一次被记录的后端调用，供无硬件测试断言交互顺序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockCall {
    method: &'static str,
    detail: String,
}

impl MockCall {
    pub fn method(&self) -> &'static str {
        self.method
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for MockCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}({})", self.method, self.detail)
    }
}

/// 可脚本化的内存后端：GUI 与 CLI 在没有硬件与没有真实引擎时仍可开发与测试。
pub struct MockBackend {
    config: Mutex<Config>,
    devices: Mutex<Vec<Device>>,
    jobs: Mutex<Vec<JobSnapshot>>,
    records: Mutex<Vec<Record>>,
    session: Mutex<ReceiveSession>,
    auth_password: Mutex<Option<String>>,
    scripted: Mutex<VecDeque<Event>>,
    calls: Mutex<Vec<MockCall>>,
    bus: EventBus,
    next_job: AtomicU64,
    next_ask: AtomicU64,
    next_pin: AtomicU64,
    next_record: AtomicU64,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl MockBackend {
    pub fn new(devices: Vec<Device>) -> Self {
        Self::with_config(Config::default(), devices)
    }

    pub fn with_config(config: Config, devices: Vec<Device>) -> Self {
        let session = ReceiveSession::idle(config.receive_dir.clone(), config.receive_ask);
        Self {
            config: Mutex::new(config),
            devices: Mutex::new(devices),
            jobs: Mutex::new(Vec::new()),
            records: Mutex::new(Vec::new()),
            session: Mutex::new(session),
            auth_password: Mutex::new(None),
            scripted: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
            bus: EventBus::new(),
            next_job: AtomicU64::new(0),
            next_ask: AtomicU64::new(0),
            next_pin: AtomicU64::new(0),
            next_record: AtomicU64::new(0),
        }
    }

    /// 预置后续要广播的事件，`flush_scripted` 或 `subscribe` 之后统一投递。
    pub fn push_scripted(&self, events: impl IntoIterator<Item = Event>) {
        let mut scripted = self.scripted.lock().expect("脚本事件表被投毒");
        scripted.extend(events);
    }

    pub fn flush_scripted(&self) {
        let pending: Vec<Event> = {
            let mut scripted = self.scripted.lock().expect("脚本事件表被投毒");
            scripted.drain(..).collect()
        };
        for event in pending {
            self.bus.emit(event);
        }
    }

    pub fn emit(&self, event: Event) {
        self.bus.emit(event);
    }

    pub fn calls(&self) -> Vec<MockCall> {
        self.calls.lock().map(|calls| calls.clone()).unwrap_or_default()
    }

    pub fn take_calls(&self) -> Vec<MockCall> {
        let mut calls = self.calls.lock().expect("调用记录表被投毒");
        std::mem::take(&mut *calls)
    }

    pub fn has_call(&self, method: &str) -> bool {
        self.calls
            .lock()
            .map(|calls| calls.iter().any(|call| call.method == method))
            .unwrap_or(false)
    }

    /// 测试用：读取已保存的认证密码。
    pub fn auth_password(&self) -> Option<String> {
        self.auth_password.lock().ok().and_then(|value| value.clone())
    }

    /// 测试与界面联调用：模拟一条接收询问。
    pub fn simulate_receive_ask(&self, name: &str, size: u64) -> AskId {
        let id = AskId(self.next_ask.fetch_add(1, Ordering::SeqCst) + 1);
        let ask = crate::dto::ReceiveAsk {
            id,
            name: name.to_string(),
            size,
            device: None,
        };
        if let Ok(mut session) = self.session.lock() {
            session.pending_ask = Some(ask);
        }
        self.bus.emit(Event::ReceiveAsk {
            id,
            name: name.to_string(),
            size,
        });
        id
    }

    pub fn set_devices(&self, devices: Vec<Device>) {
        if let Ok(mut current) = self.devices.lock() {
            *current = devices;
        }
    }

    pub fn add_record(&self, record: Record) {
        if let Ok(mut records) = self.records.lock() {
            records.push(record);
        }
        self.bus.emit(Event::RecordsChanged);
    }

    fn push_call(&self, method: &'static str, detail: impl Into<String>) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(MockCall {
                method,
                detail: detail.into(),
            });
        }
    }

    fn device_or_error(&self, id: &DeviceId) -> Result<Device, BtError> {
        let devices = self.devices.lock().expect("设备缓存被投毒");
        devices
            .iter()
            .find(|device| device.id == *id)
            .cloned()
            .ok_or_else(|| BtError::not_found(format!("设备不存在 {id}")))
    }

    fn next_record_id(&self) -> i64 {
        self.next_record.fetch_add(1, Ordering::SeqCst) as i64 + 1
    }

    fn emit_job_state(&self, job_id: JobId, state: JobState) -> Result<JobSnapshot, BtError> {
        let snapshot = {
            let mut jobs = self.jobs.lock().expect("任务表被投毒");
            let job = jobs
                .iter_mut()
                .find(|job| job.id == job_id)
                .ok_or_else(|| BtError::not_found(format!("任务不存在 {}", job_id.0)))?;
            job.state = state;
            job.updated_at = now_rfc3339();
            job.clone()
        };
        self.bus.emit(Event::JobStateChanged { job: snapshot.clone() });
        if state.is_terminal() && self.all_jobs_terminal() {
            self.bus.emit(Event::QueueIdle);
        }
        Ok(snapshot)
    }

    fn all_jobs_terminal(&self) -> bool {
        self.jobs
            .lock()
            .map(|jobs| jobs.iter().all(|job| job.state.is_terminal()))
            .unwrap_or(true)
    }
}

impl Backend for MockBackend {
    fn config(&self) -> Config {
        self.config.lock().map(|config| config.clone()).unwrap_or_default()
    }

    fn update_config(&self, patch: ConfigPatch) -> Result<Config, BtError> {
        self.push_call("update_config", format!("empty={}", patch.is_empty()));
        let updated = {
            let mut config = self.config.lock().expect("配置被投毒");
            patch.apply(&mut config);
            config.normalize();
            config.clone()
        };
        self.bus.emit(Event::ConfigChanged {
            config: updated.clone(),
        });
        Ok(updated)
    }

    fn set_auth_password(&self, password: Option<&str>) -> Result<(), BtError> {
        self.push_call("set_auth_password", if password.is_some() { "set" } else { "clear" });
        let mut current = self.auth_password.lock().expect("密码槽被投毒");
        *current = password.map(|value| value.to_string());
        Ok(())
    }

    fn devices(&self) -> Vec<Device> {
        self.devices.lock().map(|devices| devices.clone()).unwrap_or_default()
    }

    fn start_scan(&self, options: ScanOptions) -> Result<(), BtError> {
        self.push_call("start_scan", format!("duration={:?}", options.duration_s));
        let devices = self.devices();
        self.bus.emit(Event::ScanStarted);
        self.bus.emit(Event::DeviceList(devices.clone()));
        self.bus.emit(Event::ScanFinished { count: devices.len() });
        Ok(())
    }

    fn cancel_scan(&self) -> Result<(), BtError> {
        self.push_call("cancel_scan", "");
        self.bus.emit(Event::ScanFinished {
            count: self.devices().len(),
        });
        Ok(())
    }

    fn pair(&self, device: &DeviceId) -> Result<(), BtError> {
        let found = self.device_or_error(device)?;
        self.push_call("pair", device.to_string());
        let id = PinRequestId(self.next_pin.fetch_add(1, Ordering::SeqCst) + 1);
        let kind = PinPromptKind::Confirm;
        let request = PinRequest {
            id,
            device: found.id,
            kind,
        };
        self.bus.emit(Event::PairPrompt {
            request,
            device: found,
            kind,
        });
        Ok(())
    }

    fn answer_prompt(&self, request: PinRequestId, accept: bool) -> Result<(), BtError> {
        self.push_call("answer_prompt", format!("{} accept={accept}", request.0));
        let device_id = self
            .devices
            .lock()
            .ok()
            .and_then(|devices| devices.first().map(|device| device.id))
            .ok_or_else(|| BtError::not_found("没有待应答的配对请求"))?;
        self.bus.emit(Event::PairResult {
            device_id,
            ok: accept,
            error: None,
        });
        Ok(())
    }

    fn set_alias(&self, device: &DeviceId, alias: Option<&str>) -> Result<(), BtError> {
        self.push_call("set_alias", device.to_string());
        let updated = {
            let mut devices = self.devices.lock().expect("设备缓存被投毒");
            let target = devices
                .iter_mut()
                .find(|entry| entry.id == *device)
                .ok_or_else(|| BtError::not_found(format!("设备不存在 {device}")))?;
            target.alias = alias.map(|value| value.to_string());
            target.clone()
        };
        self.bus.emit(Event::DeviceUpdated(updated));
        Ok(())
    }

    fn set_favorite(&self, device: &DeviceId, favorite: bool) -> Result<(), BtError> {
        self.push_call("set_favorite", format!("{device} favorite={favorite}"));
        let updated = {
            let mut devices = self.devices.lock().expect("设备缓存被投毒");
            let target = devices
                .iter_mut()
                .find(|entry| entry.id == *device)
                .ok_or_else(|| BtError::not_found(format!("设备不存在 {device}")))?;
            target.favorite = favorite;
            target.clone()
        };
        self.bus.emit(Event::DeviceUpdated(updated));
        Ok(())
    }

    fn detect(&self, device: &DeviceId) -> Result<(), BtError> {
        self.push_call("detect", device.to_string());
        let updated = {
            let mut devices = self.devices.lock().expect("设备缓存被投毒");
            let target = devices
                .iter_mut()
                .find(|entry| entry.id == *device)
                .ok_or_else(|| BtError::not_found(format!("设备不存在 {device}")))?;
            target.assistant_state = AssistantState::Available;
            if !target.channels.contains(&Channel::Assistant) {
                target.channels.push(Channel::Assistant);
            }
            target.clone()
        };
        self.bus.emit(Event::AssistantStateChanged {
            device_id: updated.id,
            state: updated.assistant_state,
        });
        self.bus.emit(Event::DeviceUpdated(updated));
        Ok(())
    }

    fn open_bluetooth_settings(&self) -> Result<(), BtError> {
        self.push_call("open_bluetooth_settings", "");
        Ok(())
    }

    fn enqueue(
        &self,
        device: &DeviceId,
        items: Vec<SendItem>,
        channel: Option<Channel>,
    ) -> Result<Vec<JobId>, BtError> {
        let found = self.device_or_error(device)?;
        if items.is_empty() {
            return Err(BtError::invalid("发送条目为空"));
        }
        let channel = channel.unwrap_or(match self.config().mode {
            TransferMode::Assistant => Channel::Assistant,
            _ => Channel::Opp,
        });
        self.push_call(
            "enqueue",
            format!("{device} items={} channel={}", items.len(), channel.as_str()),
        );
        let mut queued = Vec::with_capacity(items.len());
        for item in items {
            let id = JobId(self.next_job.fetch_add(1, Ordering::SeqCst) + 1);
            let now = now_rfc3339();
            let total = item.total_bytes.unwrap_or(0);
            let job = JobSnapshot {
                id,
                device_id: found.id,
                device_name: found.label(),
                item: item.clone(),
                channel,
                state: JobState::Completed,
                bytes_sent: total,
                total_bytes: total,
                bps: 0,
                eta_seconds: Some(0),
                attempts: 1,
                error: None,
                resumable: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            if let Ok(mut jobs) = self.jobs.lock() {
                jobs.push(job.clone());
            }
            self.bus.emit(Event::JobAdded { job: job.clone() });
            self.bus.emit(Event::JobProgress {
                job_id: id,
                bytes: total,
                total,
                bps: 0,
                eta_seconds: Some(0),
            });
            self.bus.emit(Event::JobStateChanged { job: job.clone() });
            let mut record = Record::running(Direction::Send, channel, item.kind, item.display_name.clone(), now);
            record.id = self.next_record_id();
            record.device_id = Some(found.id);
            record.device_name = found.label();
            record.status = RecordStatus::Completed;
            record.byte_size = total;
            record.transferred = total;
            record.finished_at = Some(now_rfc3339());
            record.path = match &item.source {
                bt_core::SendSource::File(path) | bt_core::SendSource::Folder(path) => Some(path.clone()),
                _ => None,
            };
            if let Ok(mut records) = self.records.lock() {
                records.push(record);
            }
            queued.push(id);
        }
        self.bus.emit(Event::QueueIdle);
        self.bus.emit(Event::RecordsChanged);
        Ok(queued)
    }

    fn jobs(&self) -> Vec<JobSnapshot> {
        self.jobs.lock().map(|jobs| jobs.clone()).unwrap_or_default()
    }

    fn pause_job(&self, job: JobId) -> Result<(), BtError> {
        self.push_call("pause_job", job.0.to_string());
        self.emit_job_state(job, JobState::Paused).map(|_| ())
    }

    fn resume_job(&self, job: JobId) -> Result<(), BtError> {
        self.push_call("resume_job", job.0.to_string());
        self.emit_job_state(job, JobState::Sending).map(|_| ())
    }

    fn cancel_job(&self, job: JobId) -> Result<(), BtError> {
        self.push_call("cancel_job", job.0.to_string());
        self.emit_job_state(job, JobState::Canceled).map(|_| ())
    }

    fn cancel_all_jobs(&self) -> Result<(), BtError> {
        self.push_call("cancel_all_jobs", "");
        let ids: Vec<JobId> = self
            .jobs
            .lock()
            .map(|jobs| jobs.iter().map(|job| job.id).collect())
            .unwrap_or_default();
        for id in ids {
            self.emit_job_state(id, JobState::Canceled)?;
        }
        Ok(())
    }

    fn retry_job(&self, job: JobId) -> Result<(), BtError> {
        self.push_call("retry_job", job.0.to_string());
        self.emit_job_state(job, JobState::Queued).map(|_| ())
    }

    fn remove_job(&self, job: JobId) -> Result<(), BtError> {
        self.push_call("remove_job", job.0.to_string());
        let mut jobs = self.jobs.lock().expect("任务表被投毒");
        let index = jobs
            .iter()
            .position(|entry| entry.id == job)
            .ok_or_else(|| BtError::not_found(format!("任务不存在 {}", job.0)))?;
        if !jobs[index].state.is_terminal() {
            return Err(BtError::busy("任务尚未结束，不能移除"));
        }
        jobs.remove(index);
        Ok(())
    }

    fn start_receive(&self, options: ReceiveOptions) -> Result<(), BtError> {
        self.push_call("start_receive", format!("dir={:?}", options.dir));
        let mut session = self.session.lock().expect("接收会话被投毒");
        if let Some(dir) = options.dir {
            session.dir = dir;
        }
        if let Some(ask) = options.ask {
            session.ask = ask;
        }
        session.listening = true;
        drop(session);
        self.bus.emit(Event::ReceiveListeningChanged { listening: true });
        Ok(())
    }

    fn stop_receive(&self) -> Result<(), BtError> {
        self.push_call("stop_receive", "");
        let mut session = self.session.lock().expect("接收会话被投毒");
        session.listening = false;
        session.pending_ask = None;
        drop(session);
        self.bus.emit(Event::ReceiveListeningChanged { listening: false });
        Ok(())
    }

    fn receive_session(&self) -> ReceiveSession {
        self.session
            .lock()
            .map(|session| session.clone())
            .unwrap_or_else(|_| ReceiveSession::idle(PathBuf::from("BluetoothReceive"), true))
    }

    fn answer_receive_ask(&self, ask: AskId, accept: bool) -> Result<Option<PathBuf>, BtError> {
        self.push_call("answer_receive_ask", format!("{} accept={accept}", ask.0));
        let pending = {
            let mut session = self.session.lock().expect("接收会话被投毒");
            session.pending_ask.take()
        };
        let pending = pending.ok_or_else(|| BtError::not_found("没有待应答的接收询问"))?;
        if !accept {
            self.bus.emit(Event::ReceiveDone {
                ok: false,
                path: None,
                error: Some("已拒绝接收".into()),
            });
            return Ok(None);
        }
        let dir = self.receive_session().dir;
        let path = paths::safe_join(&dir, &pending.name)?;
        self.bus.emit(Event::ReceiveDone {
            ok: true,
            path: Some(path.clone()),
            error: None,
        });
        Ok(Some(path))
    }

    fn records(&self, query: RecordQuery) -> Result<Vec<Record>, BtError> {
        self.push_call("records", format!("limit={:?}", query.limit));
        let mut matched: Vec<Record> = self
            .records
            .lock()
            .map(|records| records.iter().filter(|record| query.matches(record)).cloned().collect())
            .unwrap_or_default();
        matched.sort_by(|left, right| right.started_at.cmp(&left.started_at));
        let offset = query.offset.unwrap_or(0) as usize;
        let limit = query.limit.unwrap_or(u32::MAX) as usize;
        Ok(matched.into_iter().skip(offset).take(limit).collect())
    }

    fn stats(&self) -> Result<Stats, BtError> {
        self.push_call("stats", "");
        let records = self.records.lock().map(|records| records.clone()).unwrap_or_default();
        let mut stats = Stats::default();
        for record in &records {
            stats.total += 1;
            match record.direction {
                Direction::Send => {
                    stats.sent += 1;
                    stats.bytes_sent += record.byte_size;
                }
                Direction::Receive => {
                    stats.received += 1;
                    stats.bytes_received += record.byte_size;
                }
            }
            if record.status == RecordStatus::Failed {
                stats.failed += 1;
            }
            let date = record.started_at.chars().take(10).collect::<String>();
            match stats.days.iter_mut().find(|day| day.date == date) {
                Some(day) => {
                    match record.direction {
                        Direction::Send => day.sent += 1,
                        Direction::Receive => day.received += 1,
                    }
                    day.bytes += record.byte_size;
                }
                None => {
                    let mut day = DayStat {
                        date,
                        ..DayStat::default()
                    };
                    match record.direction {
                        Direction::Send => day.sent = 1,
                        Direction::Receive => day.received = 1,
                    }
                    day.bytes = record.byte_size;
                    stats.days.push(day);
                }
            }
            if stats
                .last_activity
                .as_deref()
                .is_none_or(|value| record.started_at.as_str() > value)
            {
                stats.last_activity = Some(record.started_at.clone());
            }
        }
        Ok(stats)
    }

    fn delete_record(&self, id: i64) -> Result<bool, BtError> {
        self.push_call("delete_record", id.to_string());
        let removed = {
            let mut records = self.records.lock().expect("记录表被投毒");
            match records.iter().position(|record| record.id == id) {
                Some(index) => {
                    records.remove(index);
                    true
                }
                None => false,
            }
        };
        if removed {
            self.bus.emit(Event::RecordsChanged);
        }
        Ok(removed)
    }

    fn clear_records(&self) -> Result<u64, BtError> {
        self.push_call("clear_records", "");
        let removed = {
            let mut records = self.records.lock().expect("记录表被投毒");
            let count = records.len() as u64;
            records.clear();
            count
        };
        self.bus.emit(Event::RecordsChanged);
        Ok(removed)
    }

    fn export(&self, query: RecordQuery, format: ExportFormat, path: PathBuf) -> Result<ExportSummary, BtError> {
        self.push_call(
            "export",
            format!("format={format:?} rows_in={}", query.limit.unwrap_or(0)),
        );
        let records = self.records(query)?;
        let rendered = match format {
            ExportFormat::Csv => render_csv(&records),
            ExportFormat::Jsonl => render_jsonl(&records),
        };
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(&path, rendered.as_bytes())?;
        let summary = ExportSummary {
            path,
            rows: records.len() as u64,
            bytes: rendered.len() as u64,
            format,
        };
        self.bus.emit(Event::RecordsChanged);
        Ok(summary)
    }

    fn copy_plan(&self, record_ids: &[i64]) -> Result<CopyPlan, BtError> {
        self.push_call("copy_plan", format!("ids={}", record_ids.len()));
        let records = self.records.lock().map(|records| records.clone()).unwrap_or_default();
        let mut items = Vec::new();
        for id in record_ids {
            let record = records
                .iter()
                .find(|record| record.id == *id)
                .ok_or_else(|| BtError::not_found(format!("记录不存在 {id}")))?;
            match &record.path {
                Some(path) if path.exists() => items.push(CopyItem::Path(path.clone())),
                Some(path) => items.push(CopyItem::Missing(path.clone())),
                None => items.push(CopyItem::Text(record.display_name.clone())),
            }
        }
        let kind = copy_kind(&items);
        Ok(CopyPlan {
            items,
            kind: Some(kind),
        })
    }

    fn selftest(&self) -> Result<SelftestReport, BtError> {
        self.push_call("selftest", "");
        let mut report = SelftestReport::default();
        let config = self.config();
        report.push("config", config.validate().is_ok(), "配置在合法区间");
        report.push("devices", true, format!("缓存 {} 台设备", self.devices().len()));
        report.push(
            "bus",
            self.bus.subscriber_count() > 0,
            format!("订阅者 {}", self.bus.subscriber_count()),
        );
        self.bus.emit(Event::log(LogLevel::Info, "模拟后端自检完成"));
        Ok(report)
    }

    fn migrate_legacy(&self, options: MigrateOptions) -> Result<MigrateReport, BtError> {
        self.push_call("migrate_legacy", options.legacy_root.display().to_string());
        let exists = options.legacy_root.exists();
        let mut report = MigrateReport {
            dry_run: options.dry_run,
            ..MigrateReport::default()
        };
        if !exists {
            report
                .warnings
                .push(format!("旧数据目录不存在 {}", options.legacy_root.display()));
        }
        Ok(report)
    }

    fn subscribe(&self) -> Receiver<Event> {
        self.bus.subscribe()
    }

    fn shutdown(&self) {
        self.push_call("shutdown", "");
    }
}

fn copy_kind(items: &[CopyItem]) -> CopyKind {
    let has_path = items
        .iter()
        .any(|item| matches!(item, CopyItem::Path(_) | CopyItem::Missing(_)));
    let has_text = items.iter().any(|item| matches!(item, CopyItem::Text(_)));
    match (has_path, has_text) {
        (true, true) => CopyKind::Mixed,
        (true, false) => CopyKind::Files,
        _ => CopyKind::Text,
    }
}

fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn render_csv(records: &[Record]) -> String {
    let mut out = String::from("id,direction,channel,kind,status,device,name,bytes,started_at,finished_at\n");
    for record in records {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            record.id,
            record.direction.as_str(),
            record.channel.as_str(),
            record.kind.as_str(),
            record.status.as_str(),
            csv_escape(&record.device_name),
            csv_escape(&record.display_name),
            record.byte_size,
            record.started_at,
            record.finished_at.clone().unwrap_or_default()
        ));
    }
    out
}

fn render_jsonl(records: &[Record]) -> String {
    let mut out = String::new();
    for record in records {
        out.push_str(&format!(
            "{{\"id\":{},\"direction\":\"{}\",\"channel\":\"{}\",\"kind\":\"{}\",\"status\":\"{}\",\"device\":\"{}\",\"name\":\"{}\",\"bytes\":{},\"started_at\":\"{}\",\"finished_at\":{}}}\n",
            record.id,
            record.direction.as_str(),
            record.channel.as_str(),
            record.kind.as_str(),
            record.status.as_str(),
            json_escape(&record.device_name),
            json_escape(&record.display_name),
            record.byte_size,
            record.started_at,
            match record.finished_at.as_deref() {
                Some(value) => format!("\"{}\"", json_escape(value)),
                None => "null".to_string(),
            }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::SendSource;

    fn assert_send_sync<T: Send + Sync + ?Sized>() {}

    #[test]
    fn backend_types_are_send_and_sync() {
        assert_send_sync::<MockBackend>();
        assert_send_sync::<dyn Backend>();
    }

    #[test]
    fn scan_emits_started_list_and_finished() {
        let id = DeviceId::parse("AA:BB:CC:DD:EE:01").unwrap();
        let backend = MockBackend::new(vec![Device::new(id, "Pixel")]);
        let events = backend.subscribe();
        backend.start_scan(ScanOptions::default()).unwrap();
        assert_eq!(events.recv().unwrap(), Event::ScanStarted);
        assert!(matches!(events.recv().unwrap(), Event::DeviceList(_)));
        assert_eq!(events.recv().unwrap(), Event::ScanFinished { count: 1 });
        assert!(backend.has_call("start_scan"));
    }

    #[test]
    fn enqueue_produces_completed_job_and_record() {
        let id = DeviceId::parse("AA:BB:CC:DD:EE:02").unwrap();
        let backend = MockBackend::new(vec![Device::new(id, "Pixel")]);
        let events = backend.subscribe();
        let item = SendItem {
            source: SendSource::Text("hello".into()),
            display_name: "hello".into(),
            total_bytes: Some(5),
            kind: bt_core::RecordKind::Text,
        };
        let jobs = backend.enqueue(&id, vec![item], None).unwrap();
        assert_eq!(jobs.len(), 1);
        let mut saw_added = false;
        while let Ok(event) = events.try_recv() {
            if matches!(event, Event::JobAdded { .. }) {
                saw_added = true;
            }
        }
        assert!(saw_added);
        assert_eq!(backend.jobs()[0].state, JobState::Completed);
        let records = backend.records(RecordQuery::default()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, RecordStatus::Completed);
    }

    #[test]
    fn auth_password_is_never_part_of_config() {
        let backend = MockBackend::default();
        backend.set_auth_password(Some("secret")).unwrap();
        assert_eq!(backend.auth_password().as_deref(), Some("secret"));
        assert!(!format!("{:?}", backend.config()).contains("secret"));
        assert!(backend
            .take_calls()
            .iter()
            .any(|call| call.method() == "set_auth_password"));
    }

    #[test]
    fn export_writes_csv_rows() {
        let backend = MockBackend::default();
        let dir = std::env::temp_dir().join("bt-api-mock-export");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("records.csv");
        let summary = backend
            .export(RecordQuery::default(), ExportFormat::Csv, path.clone())
            .unwrap();
        assert_eq!(summary.rows, 0);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("id,direction"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unknown_job_reports_not_found() {
        let backend = MockBackend::default();
        let error = backend.cancel_job(JobId(99)).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::NotFound);
    }

    #[test]
    fn scripted_events_flush_in_order() {
        let backend = MockBackend::default();
        let events = backend.subscribe();
        backend.push_scripted([Event::QueueIdle, Event::RecordsChanged]);
        backend.flush_scripted();
        assert_eq!(events.recv().unwrap(), Event::QueueIdle);
        assert_eq!(events.recv().unwrap(), Event::RecordsChanged);
    }

    #[test]
    fn copy_plan_marks_missing_files() {
        let backend = MockBackend::default();
        let mut record = Record::running(
            Direction::Send,
            Channel::Opp,
            bt_core::RecordKind::File,
            "gone.bin",
            "2026-01-01T00:00:00Z",
        );
        record.id = 7;
        record.path = Some(PathBuf::from("/nonexistent/blue-tooth-transfer/gone.bin"));
        backend.add_record(record);
        let plan = backend.copy_plan(&[7]).unwrap();
        assert_eq!(plan.missing_count(), 1);
        assert_eq!(plan.kind, Some(CopyKind::Files));
    }
}
