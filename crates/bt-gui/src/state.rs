//! 界面状态：持有后端快照与全部页面状态，并把事件与后台任务归并到 UI 线程。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use bt_api::{
    Device, Event, JobId, JobSnapshot, JobState, LogLevel, MigrateOptions, MigrateReport, PinPromptKind, PinRequest,
    ReceiveAsk, RecordQuery, ScanOptions, SelftestReport, SendItem, SharedBackend, Stats,
};
use bt_core::{
    now_rfc3339, BtError, Channel, Config, DeviceId, Direction, Record, RecordKind, RecordStatus, SendSource,
};
use crossbeam_channel::{Receiver, Sender};

use crate::log_sink::LogStore;
use crate::theme::FontReport;

const RECORD_DISPLAY_LIMIT: u32 = 1000;
const EXTERNAL_DEBOUNCE: Duration = Duration::from_millis(400);
const RECEIVE_PROBE_INTERVAL: Duration = Duration::from_secs(8);

/// 页面标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Devices,
    Send,
    Queue,
    Receive,
    Records,
    Logs,
    Settings,
}

impl Page {
    pub const ALL: [Page; 7] = [
        Page::Devices,
        Page::Send,
        Page::Queue,
        Page::Receive,
        Page::Records,
        Page::Logs,
        Page::Settings,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Page::Devices => "设备",
            Page::Send => "发送",
            Page::Queue => "队列",
            Page::Receive => "接收",
            Page::Records => "记录",
            Page::Logs => "日志",
            Page::Settings => "设置",
        }
    }
}

/// 全局提示等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub text: String,
}

/// 发送通道选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelChoice {
    Auto,
    Assistant,
    Opp,
}

impl ChannelChoice {
    pub fn channel(self) -> Option<Channel> {
        match self {
            Self::Auto => None,
            Self::Assistant => Some(Channel::Assistant),
            Self::Opp => Some(Channel::Opp),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::Assistant => "助手通道",
            Self::Opp => "OPP",
        }
    }
}

/// 发送页状态。
#[derive(Debug)]
pub struct SendPage {
    pub selected_device: Option<DeviceId>,
    pub channel: ChannelChoice,
    pub text: String,
    pub zip_single: bool,
    pub items: Vec<SendItem>,
    pub warnings: Vec<String>,
    pub busy: bool,
}

impl Default for SendPage {
    fn default() -> Self {
        Self {
            selected_device: None,
            channel: ChannelChoice::Auto,
            text: String::new(),
            zip_single: false,
            items: Vec::new(),
            warnings: Vec::new(),
            busy: false,
        }
    }
}

/// 一条本会话接收记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedEntry {
    pub name: String,
    pub path: Option<PathBuf>,
    pub size: u64,
    pub ok: bool,
    pub error: Option<String>,
    pub at: String,
}

/// 接收页状态。
#[derive(Debug, Default)]
pub struct ReceivePage {
    pub listening: bool,
    pub active_name: Option<String>,
    pub active_bytes: u64,
    pub active_total: Option<u64>,
    pub pending_ask: Option<ReceiveAsk>,
    pub completed: Vec<ReceivedEntry>,
    pub standalone_running: bool,
    pub probe_running: bool,
    pub probe_due: Option<Instant>,
}

/// 记录筛选状态。
#[derive(Debug, Default)]
pub struct RecordsFilterState {
    pub direction: Option<Direction>,
    pub kind: Option<RecordKind>,
    pub status: Option<RecordStatus>,
    pub channel: Option<Channel>,
    pub device: Option<DeviceId>,
    pub search: String,
    pub dirty: bool,
    pub clear_confirm: bool,
    pub copying: bool,
}

/// 设置页状态。
#[derive(Debug)]
pub struct SettingsPage {
    pub draft: Config,
    pub receive_dir_text: String,
    pub auth_input: String,
    pub auth_set: bool,
    pub migration_root: String,
    pub migration_dry_run: bool,
    pub migration_report: Option<MigrateReport>,
    pub selftest_report: Option<SelftestReport>,
    pub shell_installed: bool,
    pub shell_label: String,
    pub autostart_enabled: bool,
    pub busy: bool,
}

/// 配对对话框状态。
#[derive(Debug, Clone)]
pub struct PairingState {
    pub request: PinRequest,
    pub device: Device,
    pub kind: PinPromptKind,
}

pub type Task = Box<dyn FnOnce(&mut UiState) + Send>;

/// 界面持有的全部可变状态。
pub struct UiState {
    backend: SharedBackend,
    events: Receiver<Event>,
    task_tx: Sender<Task>,
    task_rx: Receiver<Task>,
    pub default_config: Config,
    pub page: Page,
    pub config: Config,
    pub devices: Vec<Device>,
    pub jobs: Vec<JobSnapshot>,
    pub records: Vec<Record>,
    pub records_truncated: bool,
    pub stats: Stats,
    pub logs: LogStore,
    pub status: String,
    pub notice: Option<Notice>,
    pub scanning: bool,
    pub scan_duration_s: u32,
    pub scan_paired_only: bool,
    pub pairing: Option<PairingState>,
    pub pin_input: String,
    pub alias_edit: Option<(DeviceId, String)>,
    pub send: SendPage,
    pub receive: ReceivePage,
    pub filter: RecordsFilterState,
    pub settings: SettingsPage,
    pub log_min_level: LogLevel,
    pub log_follow: bool,
    pub events_seen: u64,
    pub data_dir: Option<PathBuf>,
    pub app_paths: Option<bt_app::AppPaths>,
    pub font: FontReport,
    pub backend_error: Option<String>,
    pub tray_active: bool,
    pub tray_note: Option<String>,
    pub pending_external: Vec<bt_platform::SendRequest>,
    pub external_deadline: Option<Instant>,
    pub temp_files: Vec<PathBuf>,
    pub temp_zip_jobs: HashMap<JobId, PathBuf>,
    pub last_job_states: HashMap<JobId, JobState>,
    pub focus_requested: bool,
    pub inbox_draining: bool,
    pub allow_exit: bool,
}

impl UiState {
    pub fn new(
        backend: SharedBackend,
        data_dir: Option<PathBuf>,
        app_paths: Option<bt_app::AppPaths>,
        default_config: Config,
        own_request: bt_platform::SendRequest,
    ) -> Self {
        let events = backend.subscribe();
        let config = backend.config();
        let devices = backend.devices();
        let jobs = backend.jobs();
        let session = backend.receive_session();
        let (task_tx, task_rx) = crossbeam_channel::unbounded();
        let logs = data_dir
            .as_ref()
            .map(|dir| LogStore::open(&dir.join("logs")))
            .unwrap_or_default();
        let mut state = Self {
            backend,
            events,
            task_tx,
            task_rx,
            default_config: default_config.clone(),
            page: Page::Devices,
            config: config.clone(),
            devices,
            jobs,
            records: Vec::new(),
            records_truncated: false,
            stats: Stats::default(),
            logs,
            status: "就绪".to_string(),
            notice: None,
            scanning: false,
            scan_duration_s: 15,
            scan_paired_only: false,
            pairing: None,
            pin_input: String::new(),
            alias_edit: None,
            send: SendPage::default(),
            receive: ReceivePage {
                listening: session.listening,
                ..ReceivePage::default()
            },
            filter: RecordsFilterState::default(),
            settings: SettingsPage {
                draft: config.clone(),
                receive_dir_text: config.receive_dir.display().to_string(),
                auth_input: String::new(),
                auth_set: false,
                migration_root: String::new(),
                migration_dry_run: true,
                migration_report: None,
                selftest_report: None,
                shell_installed: bt_platform::platform_shell().is_installed(),
                shell_label: bt_platform::platform_shell().label().to_string(),
                autostart_enabled: bt_platform::platform_autostart().is_enabled(),
                busy: false,
            },
            log_min_level: LogLevel::Info,
            log_follow: true,
            events_seen: 0,
            data_dir,
            app_paths,
            font: FontReport::default(),
            backend_error: None,
            tray_active: false,
            tray_note: None,
            pending_external: Vec::new(),
            external_deadline: None,
            temp_files: Vec::new(),
            temp_zip_jobs: HashMap::new(),
            last_job_states: HashMap::new(),
            focus_requested: false,
            inbox_draining: false,
            allow_exit: false,
        };
        for job in &state.jobs {
            state.last_job_states.insert(job.id, job.state);
        }
        // 提前在界面线程初始化剪贴板对象，Linux 上选区所有权依赖常驻对象。
        let _ = bt_platform::clipboard::warmup();
        state.reload_records();
        state.logs.info("界面已启动");
        if !own_request.is_empty() {
            state.pending_external.push(own_request);
        }
        state
    }

    pub fn backend(&self) -> &SharedBackend {
        &self.backend
    }

    pub fn set_backend_error(&mut self, message: String) {
        self.backend_error = Some(message.clone());
        self.logs.error(message.clone());
        self.notice = Some(Notice {
            kind: NoticeKind::Error,
            text: message,
        });
    }

    pub fn set_font_report(&mut self, report: FontReport) {
        if report.found() {
            self.logs.info(report.detail.clone());
        } else {
            self.logs.warn(report.detail.clone());
        }
        self.font = report;
    }

    pub fn set_tray(&mut self, active: bool, note: Option<String>) {
        self.tray_active = active;
        self.tray_note = note.clone();
        match note {
            Some(note) => {
                self.logs.warn(note.clone());
                self.status = note.clone();
                self.notice = Some(Notice {
                    kind: NoticeKind::Warn,
                    text: note,
                });
            }
            None if active => self.logs.info("系统托盘已启动"),
            None => self.logs.warn("系统托盘不可用，关闭窗口将直接退出"),
        }
    }

    pub fn info(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.status = message.clone();
        self.logs.info(message);
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.status = message.clone();
        self.logs.warn(message.clone());
        self.notice = Some(Notice {
            kind: NoticeKind::Warn,
            text: message,
        });
    }

    pub fn fail(&mut self, error: BtError) {
        let text = format!("{}（{}）", error.message(), error.kind());
        self.status = text.clone();
        self.logs.error(text.clone());
        self.notice = Some(Notice {
            kind: NoticeKind::Error,
            text,
        });
    }

    pub fn succeed(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.status = message.clone();
        self.logs.info(message.clone());
        self.notice = Some(Notice {
            kind: NoticeKind::Info,
            text: message,
        });
    }

    pub fn spawn_task<F>(&self, work: F)
    where
        F: FnOnce() -> Task + Send + 'static,
    {
        let sender = self.task_tx.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("bt-gui-task".to_string())
            .spawn(move || {
                let task = work();
                let _ = sender.send(task);
            })
        {
            let _ = error;
        }
    }

    /// 每帧调用：处理后端事件、后台任务、外部载荷去抖与接收进程探测。
    pub fn poll(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.apply_event(event);
        }
        let tasks = self.task_rx.clone();
        while let Ok(task) = tasks.try_recv() {
            task(self);
        }
        self.poll_external_debounce();
        self.poll_receive_probe();
        if self.filter.dirty {
            self.filter.dirty = false;
            self.reload_records();
        }
    }

    fn apply_event(&mut self, event: Event) {
        self.events_seen += 1;
        match event {
            Event::Log { level, message, at } => self.logs.push(level, message, Some(at)),
            Event::ScanStarted => {
                self.scanning = true;
                self.status = "正在扫描设备".to_string();
            }
            Event::ScanFinished { count } => {
                self.scanning = false;
                self.status = format!("扫描结束，发现 {count} 台设备");
            }
            Event::DeviceList(devices) => self.devices = devices,
            Event::DeviceUpdated(device) => match self.devices.iter_mut().find(|entry| entry.id == device.id) {
                Some(existing) => *existing = device,
                None => self.devices.push(device),
            },
            Event::PairPrompt { request, device, kind } => {
                self.pin_input.clear();
                self.pairing = Some(PairingState { request, device, kind });
                self.focus_requested = true;
                self.status = "等待配对确认".to_string();
            }
            Event::PairResult { device_id, ok, error } => {
                self.pairing = None;
                self.pin_input.clear();
                match (ok, error) {
                    (true, _) => self.succeed(format!("设备 {device_id} 配对成功")),
                    (false, Some(error)) => self.warn(format!("设备 {device_id} 配对失败：{error}")),
                    (false, None) => self.warn(format!("设备 {device_id} 配对失败")),
                }
            }
            Event::AssistantStateChanged { device_id, state } => {
                if let Some(device) = self.devices.iter_mut().find(|entry| entry.id == device_id) {
                    device.assistant_state = state;
                }
            }
            Event::JobAdded { job } => {
                self.last_job_states.insert(job.id, job.state);
                match self.jobs.iter_mut().find(|entry| entry.id == job.id) {
                    Some(existing) => *existing = job,
                    None => self.jobs.push(job),
                }
            }
            Event::JobProgress {
                job_id,
                bytes,
                total,
                bps,
                eta_seconds,
            } => {
                if let Some(job) = self.jobs.iter_mut().find(|entry| entry.id == job_id) {
                    job.bytes_sent = bytes;
                    job.total_bytes = total;
                    job.bps = bps;
                    job.eta_seconds = eta_seconds;
                }
            }
            Event::JobStateChanged { job } => {
                let new_state = job.state;
                let job_id = job.id;
                let previous = self.last_job_states.insert(job.id, new_state);
                let terminal = new_state.is_terminal();
                let name = job.item.display_name.clone();
                let device = job.device_name.clone();
                let error = job.error.clone();
                match self.jobs.iter_mut().find(|entry| entry.id == job.id) {
                    Some(existing) => *existing = job,
                    None => self.jobs.push(job),
                }
                if terminal && previous != Some(new_state) {
                    self.cleanup_job_zip(job_id);
                    self.notify_job(new_state, &name, &device, error.as_deref());
                }
                self.reload_records();
            }
            Event::QueueIdle => {
                if !self.jobs.is_empty() && self.jobs.iter().all(|job| job.state.is_terminal()) {
                    self.status = "队列空闲".to_string();
                }
            }
            Event::ReceiveListeningChanged { listening } => {
                self.receive.listening = listening;
                if listening {
                    self.receive.probe_due = None;
                }
            }
            Event::ReceiveProgress { bytes, total, name } => {
                self.receive.active_name = Some(name);
                self.receive.active_bytes = bytes;
                self.receive.active_total = total;
            }
            Event::ReceiveAsk { id, name, size } => {
                self.receive.pending_ask = Some(ReceiveAsk {
                    id,
                    name: name.clone(),
                    size,
                    device: None,
                });
                self.focus_requested = true;
                self.status = format!("收到接收请求：{name}");
            }
            Event::ReceiveDone { ok, path, error } => {
                let name = self
                    .receive
                    .active_name
                    .take()
                    .or_else(|| {
                        path.as_ref()
                            .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
                    })
                    .unwrap_or_else(|| "接收文件".to_string());
                let size = self.receive.active_bytes;
                self.receive.active_bytes = 0;
                self.receive.active_total = None;
                self.receive.completed.push(ReceivedEntry {
                    name: name.clone(),
                    path: path.clone(),
                    size,
                    ok,
                    error: error.clone(),
                    at: now_rfc3339(),
                });
                if ok {
                    self.succeed(format!("接收完成：{name}"));
                } else {
                    self.warn(format!(
                        "接收失败：{name}（{}）",
                        error.clone().unwrap_or_else(|| "未知原因".to_string())
                    ));
                }
                self.notify_receive(ok, &name, error.as_deref());
                self.reload_records();
            }
            Event::ConfigChanged { config } => {
                let was_pristine = self.settings.draft == self.config;
                self.config = config.clone();
                if was_pristine {
                    self.settings.draft = config.clone();
                    self.settings.receive_dir_text = config.receive_dir.display().to_string();
                }
            }
            Event::RecordsChanged => {
                self.reload_records();
                self.refresh_stats();
            }
        }
    }

    fn cleanup_job_zip(&mut self, job: JobId) {
        if let Some(path) = self.temp_zip_jobs.remove(&job) {
            let _ = std::fs::remove_file(&path);
        }
    }

    fn notify_receive(&mut self, ok: bool, name: &str, error: Option<&str>) {
        let notification = if ok {
            bt_platform::Notification::new("接收完成", name)
        } else {
            bt_platform::Notification::new("接收失败", format!("{name}：{}", error.unwrap_or("未知原因"))).urgent()
        };
        let notifier = bt_platform::platform_notifier();
        self.spawn_task(move || {
            let result = notifier.notify(&notification);
            Box::new(move |state: &mut UiState| {
                if let Err(error) = result {
                    state.logs.warn(error.message().to_string());
                }
            })
        });
    }

    fn notify_job(&mut self, state: JobState, name: &str, device: &str, error: Option<&str>) {
        let notification = match state {
            JobState::Completed => bt_platform::Notification::new("发送完成", format!("{name} → {device}")),
            JobState::Failed => {
                bt_platform::Notification::new("发送失败", format!("{name}：{}", error.unwrap_or("未知原因"))).urgent()
            }
            _ => return,
        };
        let notifier = bt_platform::platform_notifier();
        self.spawn_task(move || {
            let result = notifier.notify(&notification);
            Box::new(move |state: &mut UiState| {
                if let Err(error) = result {
                    state.logs.warn(error.message().to_string());
                }
            })
        });
    }

    fn poll_receive_probe(&mut self) {
        if self.page != Page::Receive {
            return;
        }
        if self.receive.probe_running {
            return;
        }
        let due = self.receive.probe_due.unwrap_or_else(Instant::now);
        if Instant::now() < due {
            return;
        }
        self.receive.probe_running = true;
        self.receive.probe_due = Some(Instant::now() + RECEIVE_PROBE_INTERVAL);
        self.spawn_task(|| {
            let running = bt_platform::process::standalone_receiver_running();
            Box::new(move |state: &mut UiState| {
                state.receive.standalone_running = running;
                state.receive.probe_running = false;
            })
        });
    }

    fn poll_external_debounce(&mut self) {
        let Some(deadline) = self.external_deadline else {
            return;
        };
        if Instant::now() < deadline || self.inbox_draining {
            return;
        }
        self.external_deadline = None;
        let requests = std::mem::take(&mut self.pending_external);
        let Some(root) = self.data_dir.as_ref().map(|dir| dir.join("inbox")) else {
            self.apply_external_requests(requests);
            return;
        };
        self.inbox_draining = true;
        let inbox = bt_platform::ExternalInbox::new(root);
        self.spawn_task(move || {
            let result = inbox.drain();
            Box::new(move |state: &mut UiState| {
                state.inbox_draining = false;
                let mut requests = requests;
                match result {
                    Ok(drain) => requests.push(bt_platform::SendRequest::from_inbox(drain.items, drain.warnings)),
                    Err(error) => state.logs.warn(format!("读取外部收件箱失败：{}", error.message())),
                }
                state.apply_external_requests(requests);
            })
        });
    }

    /// 收到二次启动或外部投递：重置 400ms 去抖计时器，窗口由调用方负责置前。
    pub fn schedule_external(&mut self, request: bt_platform::SendRequest) {
        if !request.items.is_empty() || !request.warnings.is_empty() {
            self.pending_external.push(request);
        }
        self.external_deadline = Some(Instant::now() + EXTERNAL_DEBOUNCE);
    }

    fn drain_inbox(&mut self) {
        let Some(root) = self.data_dir.as_ref().map(|dir| dir.join("inbox")) else {
            return;
        };
        let inbox = bt_platform::ExternalInbox::new(root);
        match inbox.drain() {
            Ok(drain) => {
                let has_warnings = !drain.warnings.is_empty();
                let items = drain.items;
                let warnings = drain.warnings;
                if !items.is_empty() || has_warnings {
                    self.pending_external
                        .push(bt_platform::SendRequest::from_inbox(items, warnings));
                }
            }
            Err(error) => self.logs.warn(format!("读取外部收件箱失败：{}", error.message())),
        }
    }

    /// 启动时排空收件箱，不做去抖。
    pub fn drain_inbox_now(&mut self) {
        self.drain_inbox();
        let requests = std::mem::take(&mut self.pending_external);
        self.apply_external_requests(requests);
    }

    fn apply_external_requests(&mut self, requests: Vec<bt_platform::SendRequest>) {
        let request = bt_platform::SendRequest::merge(requests);
        for warning in request.warnings {
            self.warn(warning);
        }
        if request.items.is_empty() {
            return;
        }
        let count = request.items.len();
        self.send.items.extend(request.items.into_iter().map(|item| {
            let source = match item.kind {
                bt_platform::SendKind::File => SendSource::File(item.path),
                bt_platform::SendKind::Folder => SendSource::Folder(item.path),
                bt_platform::SendKind::Text => SendSource::Text(item.text.unwrap_or_default()),
            };
            let mut send = SendItem::from_source(source);
            send.total_bytes = None;
            send
        }));
        self.page = Page::Send;
        self.succeed(format!("已加入 {count} 项待发送内容，请选择设备后发送"));
    }

    pub fn reload_records(&mut self) {
        let query = self.record_query(false);
        match self.backend.records(query) {
            Ok(records) => {
                self.records_truncated = records.len() >= RECORD_DISPLAY_LIMIT as usize;
                self.records = records;
            }
            Err(error) => self.fail(error),
        }
        self.refresh_stats();
    }

    fn refresh_stats(&mut self) {
        match self.backend.stats() {
            Ok(stats) => self.stats = stats,
            Err(error) => self.logs.warn(format!("读取统计失败：{}", error.message())),
        }
    }

    fn record_query(&self, all: bool) -> RecordQuery {
        RecordQuery {
            direction: self.filter.direction,
            channel: self.filter.channel,
            kind: self.filter.kind,
            status: self.filter.status,
            device_id: self.filter.device,
            search: {
                let search = self.filter.search.trim();
                if search.is_empty() {
                    None
                } else {
                    Some(search.to_string())
                }
            },
            from: None,
            to: None,
            limit: if all { None } else { Some(RECORD_DISPLAY_LIMIT) },
            offset: None,
        }
    }

    pub fn page(&mut self, page: Page) {
        self.page = page;
        if page == Page::Receive && self.receive.probe_due.is_none() {
            self.receive.probe_due = Some(Instant::now());
        }
    }
}

mod actions;
