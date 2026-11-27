use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use bt_api::{
    AskId, Backend, ConfigPatch, CopyItem, CopyKind, CopyPlan, Device, Event, EventBus, ExportFormat, ExportSummary,
    JobId, JobSnapshot, JobState, LogLevel, MigrateOptions, MigrateReport, PinPromptKind, PinRequest, PinRequestId,
    ReceiveAsk, ReceiveOptions, ReceiveSession, RecordQuery, ScanOptions, SelftestReport, SendItem, Stats,
};
use bt_core::{
    parse_transfer_id, BtError, Channel, Config, DeviceId, Direction, Record, RecordKind, RecordStatus,
    ASSISTANT_SERVICE_UUID,
};
use bt_engine::{
    Journal, JournalEntry, JournalKind, JournalState, PendingPackageStore, ReceiveAskNotice, ReceiveDoneNotice,
    ReceiveHooks, ReceiveProgressNotice, ReceiveService, ReceiveSettings, SendEngine, SendEvent, SendSettings,
    SendSink,
};
use bt_store::{
    export_records, load_config, read_legacy_journal, save_config, DeviceRecord, MigrateTarget, Store,
    LEGACY_JOURNAL_FILE,
};
use bt_transport::{BluetoothTransport, PairAnswer, PairPrompt, PairPromptKind, PromptHandler};
use crossbeam_channel::Receiver;

use crate::paths::AppPaths;

const PAIR_PROMPT_TIMEOUT: Duration = Duration::from_secs(120);
const SCAN_POLL: Duration = Duration::from_millis(250);

struct ScanState {
    generation: u64,
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

enum PromptReply {
    Accept,
    Reject,
    Pin(String),
}

struct PendingPrompt {
    id: u64,
    kind: PinPromptKind,
    sender: Sender<PromptReply>,
}

#[derive(Default)]
struct PromptState {
    next: AtomicU64,
    pending: Mutex<Option<PendingPrompt>>,
}

pub struct RealBackend {
    inner: Arc<Inner>,
}

struct Inner {
    paths: AppPaths,
    defaults: Config,
    config: Mutex<Config>,
    config_error: Mutex<Option<String>>,
    store: Arc<Store>,
    bus: EventBus,
    transport: Option<Arc<dyn BluetoothTransport>>,
    transport_error: Option<String>,
    devices: Mutex<Vec<Device>>,
    prompt: PromptState,
    send: Mutex<Option<Arc<SendEngine>>>,
    receive: Mutex<Option<Arc<ReceiveService>>>,
    auth_password: Mutex<Option<String>>,
    scan: Mutex<Option<ScanState>>,
    scan_generation: AtomicU64,
    closing: AtomicBool,
}

impl std::fmt::Debug for RealBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("RealBackend").finish_non_exhaustive()
    }
}

impl RealBackend {
    pub fn open(paths: AppPaths) -> Result<Self, BtError> {
        Self::open_with_options(paths, true)
    }

    /// 打开后端；auto_resume 为假时不自动恢复 journal 中的未完成传输。
    pub fn open_with_options(paths: AppPaths, auto_resume: bool) -> Result<Self, BtError> {
        let inner = Inner::open(paths, auto_resume)?;
        Ok(Self { inner })
    }

    /// 注入替身传输，供无硬件测试与自检使用。
    pub fn open_with_transport(
        paths: AppPaths,
        transport: Option<Arc<dyn BluetoothTransport>>,
    ) -> Result<Self, BtError> {
        let inner = Inner::open_with_transport(paths, transport)?;
        Ok(Self { inner })
    }

    /// 注入替身传输并显式控制是否自动恢复 journal。
    pub fn open_with_transport_options(
        paths: AppPaths,
        transport: Option<Arc<dyn BluetoothTransport>>,
        auto_resume: bool,
    ) -> Result<Self, BtError> {
        let inner = Inner::build(paths, transport, None, auto_resume)?;
        Ok(Self { inner })
    }

    pub fn paths(&self) -> &AppPaths {
        &self.inner.paths
    }
}

impl Inner {
    fn open(paths: AppPaths, auto_resume: bool) -> Result<Arc<Self>, BtError> {
        let (transport, transport_error) = match platform_transport() {
            Ok(transport) => (Some(transport), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self::build(paths, transport, transport_error, auto_resume)
    }

    fn open_with_transport(
        paths: AppPaths,
        transport: Option<Arc<dyn BluetoothTransport>>,
    ) -> Result<Arc<Self>, BtError> {
        Self::build(paths, transport, None, true)
    }

    fn build(
        paths: AppPaths,
        transport: Option<Arc<dyn BluetoothTransport>>,
        transport_error: Option<String>,
        auto_resume: bool,
    ) -> Result<Arc<Self>, BtError> {
        paths.create_dirs()?;
        let defaults = paths.default_config();
        let mut config_error = None;
        let config = match load_config(&paths.config_file, defaults.clone()) {
            Ok(config) => config,
            Err(error) => {
                config_error = Some(error.to_string());
                defaults.clone()
            }
        };
        let store = Arc::new(Store::open(&paths.database_file)?);
        let password = read_secret(&paths.password_file);
        let persisted = store.devices().unwrap_or_default();
        let cached = persisted_devices(&persisted);
        let inner = Arc::new(Self {
            paths,
            defaults,
            config: Mutex::new(config),
            config_error: Mutex::new(config_error),
            store,
            bus: EventBus::new(),
            transport,
            transport_error,
            devices: Mutex::new(cached),
            prompt: PromptState::default(),
            send: Mutex::new(None),
            receive: Mutex::new(None),
            auth_password: Mutex::new(password),
            scan: Mutex::new(None),
            scan_generation: AtomicU64::new(1),
            closing: AtomicBool::new(false),
        });
        inner.clone().install_prompt_handler();
        if inner.transport.is_none() {
            if let Some(error) = inner.transport_error.as_ref() {
                inner.log(LogLevel::Warn, format!("蓝牙传输不可用：{error}"));
            }
        }
        inner.start_send_engine()?;
        if auto_resume {
            if let Some(engine) = inner.send.lock().ok().and_then(|send| send.clone()) {
                match engine.resume_pending() {
                    Ok(0) => {}
                    Ok(count) => inner.log(LogLevel::Info, format!("恢复 {count} 个未完成传输")),
                    Err(error) => inner.log(LogLevel::Warn, format!("恢复未完成传输失败：{error}")),
                }
            }
        }
        Ok(inner)
    }

    fn start_send_engine(self: &Arc<Self>) -> Result<(), BtError> {
        let Some(transport) = self.transport.clone() else {
            return Ok(());
        };
        let packages = PendingPackageStore::new(&self.paths.pending_dir);
        let cleaner = Arc::new(PackageCleaner(packages.clone()));
        let journal = Journal::new(&self.paths.journal_file)
            .with_cleaner(cleaner)
            .with_base(&self.paths.pending_dir);
        if let Ok(entries) = journal.read_all() {
            let referenced: Vec<PathBuf> = entries.iter().map(|entry| entry.payload.clone()).collect();
            let removed = packages.clean_orphans(&referenced, bt_engine::TTL);
            if removed > 0 {
                self.log(LogLevel::Info, format!("清理 {removed} 个过期待续传产物"));
            }
        }
        let weak = Arc::downgrade(self);
        let sink: SendSink = Arc::new(move |event| {
            if let Some(inner) = weak.upgrade() {
                inner.on_send_event(event);
            }
        });
        let password = self.auth_password.lock().ok().and_then(|value| value.clone());
        let settings = SendSettings::from_config(&self.config()).with_password(password.as_deref());
        let engine = SendEngine::new(transport, journal, packages, settings, sink)?;
        if let Ok(mut slot) = self.send.lock() {
            *slot = Some(engine);
        }
        Ok(())
    }

    fn install_prompt_handler(self: Arc<Self>) {
        let Some(transport) = self.transport.clone() else {
            return;
        };
        let weak = Arc::downgrade(&self);
        let handler: PromptHandler = Arc::new(move |prompt: PairPrompt| match weak.upgrade() {
            Some(inner) => inner.handle_pair_prompt(prompt),
            None => PairAnswer::Reject,
        });
        transport.set_prompt_handler(Some(handler));
    }

    fn handle_pair_prompt(&self, prompt: PairPrompt) -> PairAnswer {
        let id = self.prompt.next.fetch_add(1, Ordering::SeqCst) + 1;
        let device = self.device_snapshot(&prompt.device);
        let kind = match prompt.kind {
            PairPromptKind::Pin => PinPromptKind::Pin,
            PairPromptKind::Confirm => PinPromptKind::Confirm,
            PairPromptKind::Passkey(passkey) => PinPromptKind::Passkey(passkey),
        };
        let request = PinRequest {
            id: PinRequestId(id),
            device: prompt.device,
            kind,
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        if let Ok(mut pending) = self.prompt.pending.lock() {
            *pending = Some(PendingPrompt { id, kind, sender });
        }
        self.bus.emit(Event::PairPrompt { request, device, kind });
        let reply = receiver
            .recv_timeout(PAIR_PROMPT_TIMEOUT)
            .unwrap_or(PromptReply::Reject);
        if let Ok(mut pending) = self.prompt.pending.lock() {
            if pending.as_ref().is_some_and(|pending| pending.id == id) {
                *pending = None;
            }
        }
        match reply {
            PromptReply::Accept => PairAnswer::Accept,
            PromptReply::Reject => PairAnswer::Reject,
            PromptReply::Pin(pin) if kind == PinPromptKind::Pin => PairAnswer::Pin(pin),
            PromptReply::Pin(_) => PairAnswer::Reject,
        }
    }

    fn log(&self, level: LogLevel, message: impl Into<String>) {
        self.bus.emit(Event::log(level, message));
    }

    fn transport_error(&self) -> BtError {
        let detail = self
            .transport_error
            .clone()
            .unwrap_or_else(|| "蓝牙传输不可用".to_string());
        BtError::unsupported(detail)
    }

    fn require_transport(&self) -> Result<Arc<dyn BluetoothTransport>, BtError> {
        self.transport.clone().ok_or_else(|| self.transport_error())
    }

    fn config(&self) -> Config {
        self.config
            .lock()
            .map(|config| config.clone())
            .unwrap_or_else(|_| self.defaults.clone())
    }

    fn auth_password(&self) -> Option<String> {
        self.auth_password.lock().ok().and_then(|password| password.clone())
    }

    fn apply_engine_settings(&self) {
        let config = self.config();
        let settings = SendSettings::from_config(&config).with_password(self.auth_password().as_deref());
        if let Some(engine) = self.send.lock().ok().and_then(|send| send.clone()) {
            engine.update_settings(settings);
        }
    }

    fn device_snapshot(&self, id: &DeviceId) -> Device {
        self.devices
            .lock()
            .ok()
            .and_then(|devices| devices.iter().find(|device| device.id == *id).cloned())
            .unwrap_or_else(|| Device::new(*id, id.to_string()))
    }

    fn devices(&self) -> Vec<Device> {
        let cached = self.devices.lock().map(|devices| devices.clone()).unwrap_or_default();
        let persisted = self.store.devices().unwrap_or_default();
        merge_persisted(cached, &persisted)
    }

    fn persist_device(&self, device: &Device) {
        let record = DeviceRecord {
            id: device.id,
            name: device.name.clone(),
            alias: device.alias.clone(),
            favorite: device.favorite,
            last_seen: device.last_seen.clone(),
            last_connected: None,
        };
        if let Err(error) = self.store.upsert_device(&record) {
            self.log(LogLevel::Warn, format!("设备信息保存失败：{error}"));
        }
    }

    fn update_cached_device(&self, id: DeviceId, update: impl FnOnce(&mut Device)) -> Device {
        if let Ok(mut devices) = self.devices.lock() {
            if let Some(device) = devices.iter_mut().find(|device| device.id == id) {
                update(device);
                return device.clone();
            }
            let mut device = Device::new(id, id.to_string());
            update(&mut device);
            devices.push(device.clone());
            return device;
        }
        let mut device = Device::new(id, id.to_string());
        update(&mut device);
        device
    }

    fn on_send_event(&self, event: SendEvent) {
        match event {
            SendEvent::JobAdded(job) => self.bus.emit(Event::JobAdded { job }),
            SendEvent::JobProgress(job) => self.bus.emit(Event::JobProgress {
                job_id: job.id,
                bytes: job.bytes_sent,
                total: job.total_bytes,
                bps: job.bps,
                eta_seconds: job.eta_seconds,
            }),
            SendEvent::JobState(job) => self.on_job_state(job),
            SendEvent::QueueIdle => self.bus.emit(Event::QueueIdle),
            SendEvent::Log { level, message } => self.bus.emit(Event::log(map_level(level), message)),
        }
    }

    fn on_job_state(&self, job: JobSnapshot) {
        if job.state.is_terminal() {
            let mut record = send_record(&job);
            record.status = match job.state {
                JobState::Completed => RecordStatus::Completed,
                JobState::Canceled => RecordStatus::Canceled,
                _ => RecordStatus::Failed,
            };
            record.error = job.error.clone();
            record.resumable = job.resumable;
            record.byte_size = job.total_bytes;
            record.transferred = job.bytes_sent;
            record.finished_at = Some(bt_core::now_rfc3339());
            if let Err(error) = self.store.insert_record(&record) {
                self.log(LogLevel::Error, format!("发送记录写入失败：{error}"));
            } else {
                self.bus.emit(Event::RecordsChanged);
            }
        }
        self.bus.emit(Event::JobStateChanged { job });
    }

    fn scan_devices(&self, discovered: Vec<bt_transport::DiscoveredDevice>) -> Vec<Device> {
        let persisted = self.store.devices().unwrap_or_default();
        let previous = self.devices.lock().map(|devices| devices.clone()).unwrap_or_default();
        let mut merged = Vec::new();
        for found in discovered {
            let prior = previous.iter().find(|device| device.id == found.id);
            let alias = persisted
                .iter()
                .find(|device| device.id == found.id)
                .and_then(|device| device.alias.clone())
                .or_else(|| prior.and_then(|device| device.alias.clone()));
            let favorite = persisted
                .iter()
                .find(|device| device.id == found.id)
                .map(|device| device.favorite)
                .or_else(|| prior.map(|device| device.favorite))
                .unwrap_or(false);
            let mut device = Device::new(found.id, found.name);
            device.alias = alias;
            device.favorite = favorite;
            device.paired = found.paired;
            device.connected = found.connected;
            device.rssi = found.rssi;
            device.channels = found.channels;
            device.assistant_state = prior.map(|device| device.assistant_state).unwrap_or_default();
            device.last_seen = Some(bt_core::now_rfc3339());
            merged.push(device);
        }
        merge_persisted(merged, &persisted)
    }

    fn scan_loop(&self, transport: Arc<dyn BluetoothTransport>, duration: Duration, cancel: Arc<AtomicBool>) {
        self.bus.emit(Event::ScanStarted);
        if let Err(error) = transport.start_scan(Some(duration)) {
            self.log(LogLevel::Error, format!("开始扫描失败：{error}"));
            self.bus.emit(Event::ScanFinished { count: 0 });
            return;
        }
        let deadline = std::time::Instant::now() + duration;
        let mut last: Vec<Device> = Vec::new();
        while std::time::Instant::now() < deadline && !cancel.load(Ordering::SeqCst) {
            std::thread::sleep(SCAN_POLL);
            let merged = self.scan_devices(transport.devices());
            if merged != last {
                last = merged.clone();
                if let Ok(mut devices) = self.devices.lock() {
                    *devices = merged.clone();
                }
                self.bus.emit(Event::DeviceList(merged));
            }
        }
        if !last.is_empty() {
            for device in &last {
                self.bus.emit(Event::DeviceUpdated(device.clone()));
            }
        }
        let _ = transport.cancel_scan();
        let count = last.len();
        self.bus.emit(Event::ScanFinished { count });
    }

    fn start_scan(self: &Arc<Self>, options: ScanOptions) -> Result<(), BtError> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(BtError::canceled("后端正在关闭"));
        }
        let transport = self.require_transport()?;
        let mut slot = self.scan.lock().map_err(|_| BtError::internal("扫描状态被投毒"))?;
        if slot.is_some() {
            return Err(BtError::busy("扫描已在进行"));
        }
        let generation = self.scan_generation.fetch_add(1, Ordering::SeqCst);
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let duration = Duration::from_secs(u64::from(options.duration_s.unwrap_or(10).clamp(1, 60)));
        let service = self.clone();
        let handle = std::thread::Builder::new()
            .name("bt-scan".to_string())
            .spawn(move || {
                service.scan_loop(transport, duration, thread_cancel);
                if let Ok(mut slot) = service.scan.lock() {
                    if slot.as_ref().is_some_and(|state| state.generation == generation) {
                        *slot = None;
                    }
                }
            })
            .map_err(|error| BtError::internal(format!("扫描线程创建失败：{error}")))?;
        *slot = Some(ScanState {
            generation,
            cancel,
            handle,
        });
        Ok(())
    }

    fn cancel_scan(&self) -> Result<(), BtError> {
        let state = self
            .scan
            .lock()
            .map_err(|_| BtError::internal("扫描状态被投毒"))?
            .take();
        if let Some(state) = state {
            state.cancel.store(true, Ordering::SeqCst);
            let _ = state.handle.join();
            if let Some(transport) = self.transport.clone() {
                let _ = transport.cancel_scan();
            }
        }
        Ok(())
    }

    fn pair(self: &Arc<Self>, device: &DeviceId) -> Result<(), BtError> {
        let transport = self.require_transport()?;
        let target = *device;
        let inner = self.clone();
        std::thread::Builder::new()
            .name("bt-pair".to_string())
            .spawn(move || {
                let result = transport.pair(&target);
                match result {
                    Ok(()) => {
                        let updated = inner.update_cached_device(target, |device| {
                            device.paired = true;
                            device.last_seen = Some(bt_core::now_rfc3339());
                        });
                        inner.persist_device(&updated);
                        inner.bus.emit(Event::PairResult {
                            device_id: target,
                            ok: true,
                            error: None,
                        });
                        inner.bus.emit(Event::DeviceUpdated(updated));
                    }
                    Err(error) => {
                        inner.bus.emit(Event::PairResult {
                            device_id: target,
                            ok: false,
                            error: Some(error.to_string()),
                        });
                    }
                }
            })
            .map_err(|error| BtError::internal(format!("配对线程创建失败：{error}")))?;
        Ok(())
    }

    fn answer_prompt(&self, request: PinRequestId, accept: bool) -> Result<(), BtError> {
        let pending = self
            .prompt
            .pending
            .lock()
            .map_err(|_| BtError::internal("配对应答被投毒"))?
            .take();
        match pending {
            Some(pending) if pending.id == request.0 && pending.kind == PinPromptKind::Pin => {
                if let Ok(mut slot) = self.prompt.pending.lock() {
                    *slot = Some(pending);
                }
                Err(BtError::invalid("PIN 配对请使用 answer_pin"))
            }
            Some(pending) if pending.id == request.0 => {
                let reply = if accept {
                    PromptReply::Accept
                } else {
                    PromptReply::Reject
                };
                let _ = pending.sender.send(reply);
                Ok(())
            }
            Some(pending) => {
                if let Ok(mut slot) = self.prompt.pending.lock() {
                    *slot = Some(pending);
                }
                Err(BtError::not_found("配对请求编号不匹配"))
            }
            None => Err(BtError::not_found("没有待应答的配对请求")),
        }
    }

    fn answer_pin(&self, request: PinRequestId, pin: &str) -> Result<(), BtError> {
        let pin = pin.trim();
        if pin.is_empty() {
            return Err(BtError::invalid("PIN 不能为空"));
        }
        if pin.chars().count() > 16 {
            return Err(BtError::invalid("PIN 最多 16 个字符"));
        }
        let pending = self
            .prompt
            .pending
            .lock()
            .map_err(|_| BtError::internal("配对应答被投毒"))?
            .take();
        match pending {
            Some(pending) if pending.id == request.0 && pending.kind == PinPromptKind::Pin => {
                let _ = pending.sender.send(PromptReply::Pin(pin.to_string()));
                Ok(())
            }
            Some(pending) if pending.id == request.0 => {
                if let Ok(mut slot) = self.prompt.pending.lock() {
                    *slot = Some(pending);
                }
                Err(BtError::invalid("该配对请求不需要 PIN"))
            }
            Some(pending) => {
                if let Ok(mut slot) = self.prompt.pending.lock() {
                    *slot = Some(pending);
                }
                Err(BtError::not_found("配对请求编号不匹配"))
            }
            None => Err(BtError::not_found("没有待应答的配对请求")),
        }
    }

    fn set_alias(&self, device: &DeviceId, alias: Option<&str>) -> Result<(), BtError> {
        let snapshot = self.device_snapshot(device);
        let cleaned = alias.map(str::trim).filter(|value| !value.is_empty());
        if self.store.set_device_alias(device, cleaned).is_err() {
            let record = DeviceRecord {
                id: *device,
                name: snapshot.name.clone(),
                alias: cleaned.map(str::to_string),
                favorite: snapshot.favorite,
                last_seen: snapshot.last_seen.clone(),
                last_connected: None,
            };
            self.store.upsert_device(&record)?;
        }
        let updated = self.update_cached_device(*device, |device| {
            device.alias = cleaned.map(str::to_string);
        });
        self.bus.emit(Event::DeviceUpdated(updated));
        Ok(())
    }

    fn set_favorite(&self, device: &DeviceId, favorite: bool) -> Result<(), BtError> {
        let snapshot = self.device_snapshot(device);
        if self.store.set_device_favorite(device, favorite).is_err() {
            let record = DeviceRecord {
                id: *device,
                name: snapshot.name.clone(),
                alias: snapshot.alias.clone(),
                favorite,
                last_seen: snapshot.last_seen.clone(),
                last_connected: None,
            };
            self.store.upsert_device(&record)?;
        }
        let updated = self.update_cached_device(*device, |device| {
            device.favorite = favorite;
        });
        self.bus.emit(Event::DeviceUpdated(updated));
        Ok(())
    }

    fn detect(self: &Arc<Self>, device: &DeviceId) -> Result<(), BtError> {
        let transport = self.require_transport()?;
        let target = *device;
        let previous = self.device_snapshot(device);
        self.bus.emit(Event::AssistantStateChanged {
            device_id: target,
            state: bt_core::AssistantState::Probing,
        });
        let inner = self.clone();
        std::thread::Builder::new()
            .name("bt-detect".to_string())
            .spawn(move || {
                let result = transport.find_service(&target, ASSISTANT_SERVICE_UUID);
                inner.finish_detect(target, previous, result);
            })
            .map_err(|error| BtError::internal(format!("探测线程创建失败：{error}")))?;
        Ok(())
    }

    fn finish_detect(&self, device: DeviceId, previous: Device, result: Result<Option<u8>, BtError>) {
        let state = match result {
            Ok(Some(_)) => bt_core::AssistantState::Available,
            Ok(None) => bt_core::AssistantState::Unavailable,
            Err(error) => {
                if previous.assistant_state == bt_core::AssistantState::Available {
                    self.log(LogLevel::Warn, format!("助手探测失败，保留上次可用结论：{error}"));
                    bt_core::AssistantState::Available
                } else {
                    self.log(LogLevel::Warn, format!("助手探测失败：{error}"));
                    bt_core::AssistantState::Unknown
                }
            }
        };
        let updated = self.update_cached_device(device, |device| {
            device.assistant_state = state;
            match state {
                bt_core::AssistantState::Available => {
                    if !device.channels.contains(&Channel::Assistant) {
                        device.channels.push(Channel::Assistant);
                    }
                }
                _ => {
                    device.channels.retain(|channel| *channel == Channel::Opp);
                }
            }
        });
        self.persist_device(&updated);
        self.bus.emit(Event::AssistantStateChanged {
            device_id: device,
            state,
        });
        self.bus.emit(Event::DeviceUpdated(updated));
    }

    fn open_bluetooth_settings(&self) -> Result<(), BtError> {
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", "ms-settings:bluetooth"])
                .spawn()
                .map(|_| ())
                .map_err(|error| BtError::new(bt_core::BtErrorKind::Io, format!("打开蓝牙设置失败：{error}")))
        }
        #[cfg(target_os = "linux")]
        {
            for (program, args) in [
                ("gnome-control-center", vec!["bluetooth"]),
                ("systemsettings5", vec!["kcm_bluetooth"]),
            ] {
                if std::process::Command::new(program).args(&args).spawn().is_ok() {
                    return Ok(());
                }
            }
            Err(BtError::unsupported("当前桌面没有可识别的蓝牙设置界面，请使用系统设置"))
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            Err(BtError::unsupported("当前平台不支持打开蓝牙设置"))
        }
    }

    fn enqueue(
        &self,
        device: &DeviceId,
        items: Vec<SendItem>,
        channel: Option<Channel>,
    ) -> Result<Vec<JobId>, BtError> {
        if items.is_empty() {
            return Err(BtError::invalid("发送条目为空"));
        }
        let snapshot = self.device_snapshot(device);
        let items = items.into_iter().map(prepare_item).collect::<Vec<_>>();
        let engine = self
            .send
            .lock()
            .map_err(|_| BtError::internal("发送引擎被投毒"))?
            .clone()
            .ok_or_else(|| self.transport_error())?;
        engine.enqueue(device, &snapshot.label(), items, channel)
    }

    fn engine(&self) -> Result<Arc<SendEngine>, BtError> {
        self.send
            .lock()
            .map_err(|_| BtError::internal("发送引擎被投毒"))?
            .clone()
            .ok_or_else(|| self.transport_error())
    }

    fn jobs(&self) -> Vec<JobSnapshot> {
        self.send
            .lock()
            .ok()
            .and_then(|send| send.clone())
            .map(|engine| engine.jobs())
            .unwrap_or_default()
    }

    fn job_action(&self, action: impl FnOnce(&SendEngine) -> Result<(), BtError>) -> Result<(), BtError> {
        let engine = self.engine()?;
        action(&engine)
    }

    fn start_receive(self: &Arc<Self>, options: ReceiveOptions) -> Result<(), BtError> {
        let transport = self.require_transport()?;
        let config = self.config();
        let dir = options.dir.clone().unwrap_or(config.receive_dir.clone());
        let ask = options.ask.unwrap_or(config.receive_ask);
        if let Some(existing) = self
            .receive
            .lock()
            .map_err(|_| BtError::internal("接收服务被投毒"))?
            .take()
        {
            let _ = existing.stop();
        }
        let weak = Arc::downgrade(self);
        let ask_weak = weak.clone();
        let progress_weak = weak.clone();
        let done_weak = weak.clone();
        let log_weak = weak;
        let hooks = ReceiveHooks {
            on_ask: Some(Arc::new(move |notice: ReceiveAskNotice| {
                if let Some(inner) = ask_weak.upgrade() {
                    inner.bus.emit(Event::ReceiveAsk {
                        id: AskId(notice.id),
                        name: notice.name,
                        size: notice.size,
                    });
                }
            })),
            on_progress: Some(Arc::new(move |notice: ReceiveProgressNotice| {
                if let Some(inner) = progress_weak.upgrade() {
                    inner.bus.emit(Event::ReceiveProgress {
                        bytes: notice.bytes,
                        total: Some(notice.total),
                        name: notice.name,
                    });
                }
            })),
            on_done: Some(Arc::new(move |notice: ReceiveDoneNotice| {
                if let Some(inner) = done_weak.upgrade() {
                    inner.on_receive_done(notice);
                }
            })),
            on_log: Some(Arc::new(move |level: &'static str, message: String| {
                if let Some(inner) = log_weak.upgrade() {
                    inner.bus.emit(Event::log(map_level(level), message));
                }
            })),
        };
        let service = ReceiveService::start(transport, ReceiveSettings::new(dir, ask), hooks)?;
        if let Ok(mut slot) = self.receive.lock() {
            *slot = Some(service);
        }
        self.bus.emit(Event::ReceiveListeningChanged { listening: true });
        Ok(())
    }

    fn stop_receive(&self) -> Result<(), BtError> {
        let service = self
            .receive
            .lock()
            .map_err(|_| BtError::internal("接收服务被投毒"))?
            .take();
        if let Some(service) = service {
            service.stop()?;
        }
        self.bus.emit(Event::ReceiveListeningChanged { listening: false });
        Ok(())
    }

    fn receive_session(&self) -> ReceiveSession {
        let config = self.config();
        let service = self.receive.lock().ok().and_then(|receive| receive.clone());
        match service {
            Some(service) => {
                let snapshot = service.snapshot();
                ReceiveSession {
                    listening: snapshot.listening,
                    dir: snapshot.dir,
                    ask: snapshot.ask,
                    pending_ask: snapshot.pending_ask.map(|notice| ReceiveAsk {
                        id: AskId(notice.id),
                        name: notice.name,
                        size: notice.size,
                        device: notice.device,
                    }),
                    active_name: snapshot.active_name,
                    received_bytes: snapshot.received_bytes,
                }
            }
            None => ReceiveSession::idle(config.receive_dir, config.receive_ask),
        }
    }

    fn answer_receive_ask(&self, ask: AskId, accept: bool) -> Result<Option<PathBuf>, BtError> {
        let service = self
            .receive
            .lock()
            .map_err(|_| BtError::internal("接收服务被投毒"))?
            .clone()
            .ok_or_else(|| BtError::not_found("没有待应答的接收询问"))?;
        service.answer(ask.0, accept)
    }

    fn on_receive_done(&self, notice: ReceiveDoneNotice) {
        let mut record = Record::running(
            Direction::Receive,
            Channel::Assistant,
            receive_kind(&notice.name, notice.path.as_deref()),
            notice.name.clone(),
            bt_core::now_rfc3339(),
        );
        record.device_id = notice.device;
        record.device_name = notice
            .device
            .map(|id| self.device_snapshot(&id).label())
            .unwrap_or_default();
        record.status = if notice.ok {
            RecordStatus::Completed
        } else if notice.error.as_deref().is_some_and(|error| error.contains("取消")) {
            RecordStatus::Canceled
        } else {
            RecordStatus::Failed
        };
        record.byte_size = notice.size;
        record.transferred = notice.size;
        record.path = notice.path.clone();
        record.checksum = notice.sha256.as_deref().and_then(|value| parse_transfer_id(value).ok());
        record.error = notice.error.clone();
        record.finished_at = Some(bt_core::now_rfc3339());
        if let Err(error) = self.store.insert_record(&record) {
            self.log(LogLevel::Error, format!("接收记录写入失败：{error}"));
        }
        self.bus.emit(Event::ReceiveDone {
            ok: notice.ok,
            path: notice.path,
            error: notice.error,
        });
        self.bus.emit(Event::RecordsChanged);
    }

    fn records(&self, query: RecordQuery) -> Result<Vec<Record>, BtError> {
        self.store.query_records(&query)
    }

    fn stats(&self) -> Result<Stats, BtError> {
        self.store.stats()
    }

    fn delete_record(&self, id: i64) -> Result<bool, BtError> {
        let removed = self.store.delete_record(id)?;
        if removed {
            self.bus.emit(Event::RecordsChanged);
        }
        Ok(removed)
    }

    fn clear_records(&self) -> Result<u64, BtError> {
        let removed = self.store.clear_records()?;
        self.bus.emit(Event::RecordsChanged);
        Ok(removed)
    }

    fn export(&self, query: RecordQuery, format: ExportFormat, path: PathBuf) -> Result<ExportSummary, BtError> {
        export_records(&self.store, &path, &query, format)
    }

    fn copy_plan(&self, record_ids: &[i64]) -> Result<CopyPlan, BtError> {
        let mut items = Vec::new();
        for id in record_ids {
            let record = self
                .store
                .record_by_id(*id)?
                .ok_or_else(|| BtError::not_found(format!("记录不存在 {id}")))?;
            if let Some(content) = record.content.as_ref() {
                items.push(CopyItem::Text(content.clone()));
                continue;
            }
            match record.path.as_ref() {
                Some(path) if path.exists() => items.push(CopyItem::Path(path.clone())),
                Some(path) => items.push(CopyItem::Missing(path.clone())),
                None => items.push(CopyItem::Text(record.display_name.clone())),
            }
        }
        let has_path = items
            .iter()
            .any(|item| matches!(item, CopyItem::Path(_) | CopyItem::Missing(_)));
        let has_text = items.iter().any(|item| matches!(item, CopyItem::Text(_)));
        let kind = match (has_path, has_text) {
            (true, true) => CopyKind::Mixed,
            (true, false) => CopyKind::Files,
            _ => CopyKind::Text,
        };
        Ok(CopyPlan {
            items,
            kind: Some(kind),
        })
    }

    fn selftest(&self) -> Result<SelftestReport, BtError> {
        let mut report = SelftestReport::default();
        let config = self.config();
        report.push("config", config.validate().is_ok(), "12 项设置已归一化");
        if let Ok(Some(error)) = self.config_error.lock().map(|error| error.clone()) {
            report.push("config_file", false, format!("配置文件读取失败：{error}"));
        } else {
            report.push("config_file", true, self.paths.config_file.display().to_string());
        }
        report.push(
            "store",
            self.store.schema_version().is_ok(),
            format!("记录库 schema v{}", bt_store::SCHEMA_VERSION),
        );
        let writable = probe_writable(&self.paths.data_dir).is_ok();
        report.push("paths", writable, self.paths.data_dir.display().to_string());
        let journal_ok = Journal::new(&self.paths.journal_file).read_all().is_ok();
        report.push("journal", journal_ok, self.paths.journal_file.display().to_string());
        match self.transport.as_ref() {
            Some(transport) => {
                report.push("transport", true, format!("通道 {}", transport.channel().unwrap_or(0)));
                report.push(
                    "engine",
                    self.send.lock().map(|send| send.is_some()).unwrap_or(false),
                    "发送引擎已装配",
                );
            }
            None => {
                let detail = self
                    .transport_error
                    .clone()
                    .unwrap_or_else(|| "蓝牙传输不可用".to_string());
                report.push_skip("transport", format!("蓝牙传输不可用：{detail}"));
                report.push_skip("engine", "无蓝牙传输，发送引擎未装配");
            }
        }
        self.bus.emit(Event::log(LogLevel::Info, "后端自检完成"));
        Ok(report)
    }

    fn migrate_legacy(self: &Arc<Self>, options: MigrateOptions) -> Result<MigrateReport, BtError> {
        let target_paths = options
            .target_dir
            .as_ref()
            .map(|root| AppPaths::under(root))
            .unwrap_or_else(|| self.paths.clone());
        let target = MigrateTarget {
            config_file: target_paths.config_file.clone(),
            database_file: target_paths.database_file.clone(),
            password_file: target_paths.password_file.clone(),
            defaults: self.defaults.clone(),
        };
        let mut report = bt_store::migrate_legacy(&options, &target)?;
        if !options.dry_run {
            match import_legacy_journal(&options.legacy_root, &self.journal_for_migration(&target_paths)) {
                Ok((imported, skipped)) => {
                    if imported > 0 || skipped > 0 {
                        report
                            .warnings
                            .push(format!("旧发送日志导入 {imported} 条，跳过 {skipped} 条"));
                    }
                }
                Err(error) => report.warnings.push(format!("旧发送日志导入失败：{error}")),
            }
            if target_paths == self.paths {
                if report.imported_config {
                    if let Ok(loaded) = load_config(&self.paths.config_file, self.defaults.clone()) {
                        if let Ok(mut config) = self.config.lock() {
                            *config = loaded;
                        }
                        self.apply_engine_settings();
                        self.bus.emit(Event::ConfigChanged { config: self.config() });
                    }
                }
                if let Some(password) = read_secret(&self.paths.password_file) {
                    if let Ok(mut slot) = self.auth_password.lock() {
                        *slot = Some(password);
                    }
                    self.apply_engine_settings();
                }
            }
            self.bus.emit(Event::RecordsChanged);
            self.log(LogLevel::Info, "旧版本数据迁移完成");
        }
        Ok(report)
    }

    fn journal_for_migration(&self, paths: &AppPaths) -> Journal {
        let packages = PendingPackageStore::new(paths.pending_dir.clone());
        let cleaner = Arc::new(PackageCleaner(packages));
        Journal::new(&paths.journal_file)
            .with_cleaner(cleaner)
            .with_base(&paths.pending_dir)
    }

    fn update_config(self: &Arc<Self>, patch: ConfigPatch) -> Result<Config, BtError> {
        let previous = self.config();
        let updated = {
            let mut config = self.config.lock().map_err(|_| BtError::internal("配置被投毒"))?;
            patch.apply(&mut config);
            config.normalize();
            config.validate()?;
            config.clone()
        };
        save_config(&self.paths.config_file, &updated, &self.defaults)?;
        if let Ok(mut config) = self.config.lock() {
            *config = updated.clone();
        }
        if let Ok(mut error) = self.config_error.lock() {
            *error = None;
        }
        self.apply_engine_settings();
        self.bus.emit(Event::ConfigChanged {
            config: updated.clone(),
        });
        let receive_running = self.receive.lock().map(|receive| receive.is_some()).unwrap_or(false);
        if receive_running
            && (previous.receive_dir != updated.receive_dir || previous.receive_ask != updated.receive_ask)
        {
            let _ = self.stop_receive();
            if let Err(error) = self.start_receive(ReceiveOptions::default()) {
                self.log(LogLevel::Warn, format!("按新配置重启接收服务失败：{error}"));
            }
        }
        Ok(updated)
    }

    fn set_auth_password(&self, password: Option<&str>) -> Result<(), BtError> {
        match password {
            Some(value) if !value.is_empty() => {
                write_secret(&self.paths.password_file, value)?;
                if let Ok(mut slot) = self.auth_password.lock() {
                    *slot = Some(value.to_string());
                }
            }
            _ => {
                match std::fs::remove_file(&self.paths.password_file) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                if let Ok(mut slot) = self.auth_password.lock() {
                    *slot = None;
                }
            }
        }
        self.apply_engine_settings();
        Ok(())
    }

    fn shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let _ = self.cancel_scan();
        let receive = self.receive.lock().ok().and_then(|mut receive| receive.take());
        if let Some(service) = receive {
            let _ = service.stop();
        }
        if let Some(engine) = self.send.lock().ok().and_then(|send| send.clone()) {
            engine.shutdown();
        }
    }
}

fn platform_transport() -> Result<Arc<dyn BluetoothTransport>, BtError> {
    #[cfg(target_os = "linux")]
    {
        let transport = bt_transport::linux::BlueZTransport::connect()?;
        Ok(Arc::new(transport))
    }
    #[cfg(target_os = "windows")]
    {
        let transport = bt_transport::windows::WinRtTransport::new()?;
        Ok(Arc::new(transport))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err(BtError::unsupported("当前平台没有蓝牙传输实现"))
    }
}

fn persisted_devices(persisted: &[DeviceRecord]) -> Vec<Device> {
    persisted
        .iter()
        .map(|record| {
            let mut device = Device::new(record.id, record.name.clone());
            device.alias = record.alias.clone();
            device.favorite = record.favorite;
            device.last_seen = record.last_seen.clone();
            device.channels = vec![Channel::Opp];
            device
        })
        .collect()
}

fn merge_persisted(mut devices: Vec<Device>, persisted: &[DeviceRecord]) -> Vec<Device> {
    for record in persisted {
        match devices.iter_mut().find(|device| device.id == record.id) {
            Some(device) => {
                if device.name.trim().is_empty() {
                    device.name = record.name.clone();
                }
                if device.alias.is_none() {
                    device.alias = record.alias.clone();
                }
                device.favorite |= record.favorite;
                if device.last_seen.is_none() {
                    device.last_seen = record.last_seen.clone();
                }
            }
            None => {
                let mut device = Device::new(record.id, record.name.clone());
                device.alias = record.alias.clone();
                device.favorite = record.favorite;
                device.last_seen = record.last_seen.clone();
                device.channels = vec![Channel::Opp];
                devices.push(device);
            }
        }
    }
    devices
}

fn prepare_item(mut item: SendItem) -> SendItem {
    if item.total_bytes.is_none() {
        item.total_bytes = Some(source_size(&item));
    }
    item
}

fn source_size(item: &SendItem) -> u64 {
    match &item.source {
        bt_core::SendSource::File(path) => std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
        bt_core::SendSource::Files(paths) => paths
            .iter()
            .map(|path| std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
            .sum(),
        bt_core::SendSource::Folder(_) => 0,
        bt_core::SendSource::Text(text) => text.len() as u64,
    }
}

fn send_record(job: &JobSnapshot) -> Record {
    let mut record = Record::running(
        Direction::Send,
        job.channel,
        job.item.kind,
        job.item.display_name.clone(),
        job.created_at.clone(),
    );
    record.device_id = Some(job.device_id);
    record.device_name = job.device_name.clone();
    record.byte_size = job.total_bytes;
    record.transferred = job.bytes_sent;
    record.resumable = job.resumable;
    match &job.item.source {
        bt_core::SendSource::File(path) | bt_core::SendSource::Folder(path) => {
            record.path = Some(path.clone());
        }
        bt_core::SendSource::Files(paths) => {
            record.path = paths.first().cloned();
        }
        bt_core::SendSource::Text(text) => {
            if text.len() <= bt_core::MAX_RECORD_CONTENT_BYTES {
                record.content = Some(text.clone());
            }
        }
    }
    record
}

fn receive_kind(name: &str, path: Option<&Path>) -> RecordKind {
    let extension = path
        .and_then(|path| path.extension())
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .or_else(|| {
            Path::new(name)
                .extension()
                .map(|value| value.to_string_lossy().to_ascii_lowercase())
        });
    match extension.as_deref() {
        Some("txt") => RecordKind::Text,
        _ => RecordKind::File,
    }
}

fn import_legacy_journal(root: &Path, journal: &Journal) -> Result<(usize, usize), BtError> {
    let entries = read_legacy_journal(&root.join(LEGACY_JOURNAL_FILE))?;
    let mut converted = Vec::with_capacity(entries.len());
    let mut skipped = 0usize;
    for entry in entries {
        let Ok(peer) = DeviceId::parse(&entry.peer_addr) else {
            skipped += 1;
            continue;
        };
        let Ok(sha256) = parse_transfer_id(&entry.sha256) else {
            skipped += 1;
            continue;
        };
        let kind = match entry.kind.as_str() {
            "file-zip" | "file_zip" => JournalKind::FileZip,
            "folder" => JournalKind::Folder,
            "text" => JournalKind::Text,
            _ => JournalKind::File,
        };
        let mut item = JournalEntry::new(
            peer,
            entry.display_name,
            entry.payload_path,
            sha256,
            entry.size,
            kind,
            entry.produced,
        );
        item.state = if entry.state == "paused" {
            JournalState::Paused
        } else {
            JournalState::Pending
        };
        item.attempts = entry.attempts;
        item.created = entry.created_epoch;
        item.updated = entry.updated_epoch;
        converted.push(item);
    }
    let report = journal.import(converted)?;
    Ok((report.imported, report.skipped + skipped))
}

fn read_secret(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn write_secret(path: &Path, secret: &str) -> Result<(), BtError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, secret.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn probe_writable(dir: &Path) -> Result<(), BtError> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".bt-write-probe-{}", std::process::id()));
    std::fs::write(&probe, b"ok")?;
    std::fs::remove_file(&probe)?;
    Ok(())
}

fn map_level(level: &str) -> LogLevel {
    match level {
        "ERROR" | "error" => LogLevel::Error,
        "WARN" | "warn" => LogLevel::Warn,
        "DEBUG" | "debug" => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

struct PackageCleaner(PendingPackageStore);

impl bt_engine::ArtifactCleaner for PackageCleaner {
    fn delete(&self, path: &Path) -> bool {
        self.0.delete(path)
    }
}

impl Backend for RealBackend {
    fn config(&self) -> Config {
        self.inner.config()
    }

    fn update_config(&self, patch: ConfigPatch) -> Result<Config, BtError> {
        self.inner.update_config(patch)
    }

    fn set_auth_password(&self, password: Option<&str>) -> Result<(), BtError> {
        self.inner.set_auth_password(password)
    }

    fn devices(&self) -> Vec<Device> {
        self.inner.devices()
    }

    fn start_scan(&self, options: ScanOptions) -> Result<(), BtError> {
        self.inner.start_scan(options)
    }

    fn cancel_scan(&self) -> Result<(), BtError> {
        self.inner.cancel_scan()
    }

    fn pair(&self, device: &DeviceId) -> Result<(), BtError> {
        self.inner.pair(device)
    }

    fn answer_prompt(&self, request: PinRequestId, accept: bool) -> Result<(), BtError> {
        self.inner.answer_prompt(request, accept)
    }

    fn answer_pin(&self, request: PinRequestId, pin: &str) -> Result<(), BtError> {
        self.inner.answer_pin(request, pin)
    }

    fn set_alias(&self, device: &DeviceId, alias: Option<&str>) -> Result<(), BtError> {
        self.inner.set_alias(device, alias)
    }

    fn set_favorite(&self, device: &DeviceId, favorite: bool) -> Result<(), BtError> {
        self.inner.set_favorite(device, favorite)
    }

    fn detect(&self, device: &DeviceId) -> Result<(), BtError> {
        self.inner.detect(device)
    }

    fn open_bluetooth_settings(&self) -> Result<(), BtError> {
        self.inner.open_bluetooth_settings()
    }

    fn enqueue(
        &self,
        device: &DeviceId,
        items: Vec<SendItem>,
        channel: Option<Channel>,
    ) -> Result<Vec<JobId>, BtError> {
        self.inner.enqueue(device, items, channel)
    }

    fn jobs(&self) -> Vec<JobSnapshot> {
        self.inner.jobs()
    }

    fn pause_job(&self, job: JobId) -> Result<(), BtError> {
        self.inner.job_action(|engine| engine.pause_job(job))
    }

    fn resume_job(&self, job: JobId) -> Result<(), BtError> {
        self.inner.job_action(|engine| engine.resume_job(job))
    }

    fn cancel_job(&self, job: JobId) -> Result<(), BtError> {
        self.inner.job_action(|engine| engine.cancel_job(job))
    }

    fn cancel_all_jobs(&self) -> Result<(), BtError> {
        self.inner.engine()?.cancel_all_jobs()
    }

    fn retry_job(&self, job: JobId) -> Result<(), BtError> {
        self.inner.job_action(|engine| engine.retry_job(job))
    }

    fn remove_job(&self, job: JobId) -> Result<(), BtError> {
        self.inner.job_action(|engine| engine.remove_job(job))
    }

    fn start_receive(&self, options: ReceiveOptions) -> Result<(), BtError> {
        self.inner.start_receive(options)
    }

    fn stop_receive(&self) -> Result<(), BtError> {
        self.inner.stop_receive()
    }

    fn receive_session(&self) -> ReceiveSession {
        self.inner.receive_session()
    }

    fn answer_receive_ask(&self, ask: AskId, accept: bool) -> Result<Option<PathBuf>, BtError> {
        self.inner.answer_receive_ask(ask, accept)
    }

    fn records(&self, query: RecordQuery) -> Result<Vec<Record>, BtError> {
        self.inner.records(query)
    }

    fn stats(&self) -> Result<Stats, BtError> {
        self.inner.stats()
    }

    fn delete_record(&self, id: i64) -> Result<bool, BtError> {
        self.inner.delete_record(id)
    }

    fn clear_records(&self) -> Result<u64, BtError> {
        self.inner.clear_records()
    }

    fn export(&self, query: RecordQuery, format: ExportFormat, path: PathBuf) -> Result<ExportSummary, BtError> {
        self.inner.export(query, format, path)
    }

    fn copy_plan(&self, record_ids: &[i64]) -> Result<CopyPlan, BtError> {
        self.inner.copy_plan(record_ids)
    }

    fn selftest(&self) -> Result<SelftestReport, BtError> {
        self.inner.selftest()
    }

    fn migrate_legacy(&self, options: MigrateOptions) -> Result<MigrateReport, BtError> {
        self.inner.migrate_legacy(options)
    }

    fn subscribe(&self) -> Receiver<Event> {
        self.inner.bus.subscribe()
    }

    fn shutdown(&self) {
        self.inner.shutdown();
    }
}
