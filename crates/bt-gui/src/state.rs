use std::collections::VecDeque;

use bt_api::{Device, Event, JobSnapshot, RecordQuery, SharedBackend, Stats};
use bt_core::{now_rfc3339, Config, Record};
use crossbeam_channel::Receiver;

const LOG_LIMIT: usize = 200;

/// 界面持有的全部可变状态；所有数据来自后端快照与事件。
pub struct UiState {
    backend: SharedBackend,
    events: Receiver<Event>,
    pub config: Config,
    pub devices: Vec<Device>,
    pub jobs: Vec<JobSnapshot>,
    pub records: Vec<Record>,
    pub stats: Stats,
    pub logs: VecDeque<String>,
    pub status: String,
    pub scanning: bool,
    pub receive_listening: bool,
    pub events_seen: u64,
}

impl UiState {
    pub fn new(backend: SharedBackend) -> Self {
        let events = backend.subscribe();
        let config = backend.config();
        let devices = backend.devices();
        let jobs = backend.jobs();
        let session = backend.receive_session();
        let mut state = Self {
            backend,
            events,
            config,
            devices,
            jobs,
            records: Vec::new(),
            stats: Stats::default(),
            logs: VecDeque::new(),
            status: "就绪".to_string(),
            scanning: false,
            receive_listening: session.listening,
            events_seen: 0,
        };
        state.refresh();
        state
    }

    pub fn backend(&self) -> &SharedBackend {
        &self.backend
    }

    /// 拉取记录与统计快照。
    pub fn refresh(&mut self) {
        if let Ok(records) = self.backend.records(RecordQuery::latest(200)) {
            self.records = records;
        }
        if let Ok(stats) = self.backend.stats() {
            self.stats = stats;
        }
        self.devices = self.backend.devices();
        self.jobs = self.backend.jobs();
        self.receive_listening = self.backend.receive_session().listening;
    }

    /// 处理一帧内到达的全部事件。
    pub fn poll(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.apply(event);
        }
    }

    fn apply(&mut self, event: Event) {
        self.events_seen += 1;
        match event {
            Event::Log { level, message, at } => {
                self.logs.push_back(format!("{at} {level} {message}"));
                if self.logs.len() > LOG_LIMIT {
                    self.logs.pop_front();
                }
            }
            Event::ScanStarted => {
                self.scanning = true;
                self.status = "扫描中".to_string();
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
            Event::PairPrompt { device, .. } => {
                self.status = format!("等待配对确认：{}", device.label());
            }
            Event::PairResult { ok, error, .. } => {
                self.status = if ok {
                    "配对成功".to_string()
                } else {
                    format!("配对失败：{}", error.unwrap_or_default())
                };
            }
            Event::AssistantStateChanged { device_id, state } => {
                if let Some(device) = self.devices.iter_mut().find(|entry| entry.id == device_id) {
                    device.assistant_state = state;
                }
            }
            Event::JobAdded { job } | Event::JobStateChanged { job } => {
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
            Event::QueueIdle => {
                self.status = "队列空闲".to_string();
            }
            Event::ReceiveListeningChanged { listening } => {
                self.receive_listening = listening;
            }
            Event::ReceiveProgress { bytes, total, name } => {
                self.status = match total {
                    Some(total) if total > 0 => format!("接收 {name} {bytes}/{total}"),
                    _ => format!("接收 {name} {bytes}"),
                };
            }
            Event::ReceiveAsk { name, size, .. } => {
                self.status = format!("等待接收确认：{name} {size} 字节");
            }
            Event::ReceiveDone { ok, path, error } => {
                self.status = if ok {
                    format!("接收完成 {path:?}")
                } else {
                    format!("接收失败：{}", error.unwrap_or_default())
                };
                self.refresh();
            }
            Event::ConfigChanged { config } => self.config = config,
            Event::RecordsChanged => self.refresh(),
        }
    }

    pub fn start_scan(&mut self) {
        match self.backend.start_scan(bt_api::ScanOptions::default()) {
            Ok(()) => self.status = "已请求扫描".to_string(),
            Err(error) => self.status = format!("扫描失败：{error}"),
        }
        self.poll();
    }

    pub fn refresh_clicked(&mut self) {
        self.refresh();
        self.status = format!("已刷新 {}", now_rfc3339());
    }
}
