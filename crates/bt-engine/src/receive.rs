use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bt_core::{BtError, DeviceId, ASSISTANT_SERVICE_UUID};
use bt_transport::{BluetoothTransport, TransportStream};

use crate::protocol::Hello;
use crate::server::{send_busy, serve_connection, ServerHooks, ServerOutcome};
use crate::sink::{FileSink, STALE_AFTER};

#[derive(Debug, Clone)]
pub struct ReceiveSettings {
    pub dir: PathBuf,
    pub ask: bool,
    pub idle_timeout: Duration,
    pub ask_timeout: Duration,
}

impl ReceiveSettings {
    pub fn new(dir: impl Into<PathBuf>, ask: bool) -> Self {
        Self {
            dir: dir.into(),
            ask,
            idle_timeout: crate::server::DEFAULT_IDLE_TIMEOUT,
            ask_timeout: Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReceiveAskNotice {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub device: Option<DeviceId>,
    pub planned_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ReceiveProgressNotice {
    pub name: String,
    pub bytes: u64,
    pub total: u64,
}

#[derive(Debug, Clone)]
pub struct ReceiveDoneNotice {
    pub ok: bool,
    pub name: String,
    pub size: u64,
    pub path: Option<PathBuf>,
    pub sha256: Option<String>,
    pub error: Option<String>,
    pub device: Option<DeviceId>,
}

#[derive(Clone, Default)]
pub struct ReceiveHooks {
    pub on_ask: Option<Arc<dyn Fn(ReceiveAskNotice) + Send + Sync>>,
    pub on_progress: Option<Arc<dyn Fn(ReceiveProgressNotice) + Send + Sync>>,
    pub on_done: Option<Arc<dyn Fn(ReceiveDoneNotice) + Send + Sync>>,
    pub on_log: Option<Arc<dyn Fn(&'static str, String) + Send + Sync>>,
}

impl ReceiveHooks {
    fn log(&self, level: &'static str, message: String) {
        if let Some(log) = self.on_log.as_ref() {
            log(level, message);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReceiveSnapshot {
    pub listening: bool,
    pub dir: PathBuf,
    pub ask: bool,
    pub pending_ask: Option<ReceiveAskNotice>,
    pub active_name: Option<String>,
    pub received_bytes: u64,
}

struct PendingAsk {
    notice: ReceiveAskNotice,
    responder: Sender<bool>,
}

struct State {
    listening: bool,
    dir: PathBuf,
    ask: bool,
    active_name: Option<String>,
    pending: Option<PendingAsk>,
    received_bytes: u64,
}

/// 单活跃接收服务：第二个连接立即收到 BUSY，接受后落盘、校验并回调。
pub struct ReceiveService {
    transport: Arc<dyn BluetoothTransport>,
    state: Mutex<State>,
    hooks: ReceiveHooks,
    active: AtomicBool,
    next_ask: AtomicU64,
    idle_timeout: Duration,
    ask_timeout: Duration,
}

impl std::fmt::Debug for ReceiveService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ReceiveService").finish_non_exhaustive()
    }
}

impl ReceiveService {
    pub fn start(
        transport: Arc<dyn BluetoothTransport>,
        settings: ReceiveSettings,
        hooks: ReceiveHooks,
    ) -> Result<Arc<Self>, BtError> {
        let sink = FileSink::new(&settings.dir)?;
        sink.clean_stale(STALE_AFTER)?;
        let service = Arc::new(Self {
            transport,
            state: Mutex::new(State {
                listening: false,
                dir: settings.dir,
                ask: settings.ask,
                active_name: None,
                pending: None,
                received_bytes: 0,
            }),
            hooks,
            active: AtomicBool::new(false),
            next_ask: AtomicU64::new(0),
            idle_timeout: settings.idle_timeout,
            ask_timeout: settings.ask_timeout,
        });
        let handler_service = service.clone();
        service.transport.start(
            ASSISTANT_SERVICE_UUID,
            Arc::new(move |stream: Box<dyn TransportStream>| {
                handler_service.accept_connection(stream);
            }),
        )?;
        if let Ok(mut state) = service.state.lock() {
            state.listening = true;
        }
        service.hooks.log(
            "INFO",
            format!("接收助手已开始监听（目录：{}）", service.dir().display()),
        );
        Ok(service)
    }

    pub fn dir(&self) -> PathBuf {
        self.state.lock().map(|state| state.dir.clone()).unwrap_or_default()
    }

    pub fn stop(&self) -> Result<(), BtError> {
        self.transport.stop()?;
        if let Ok(mut state) = self.state.lock() {
            state.listening = false;
            if let Some(pending) = state.pending.take() {
                let _ = pending.responder.send(false);
            }
        }
        self.hooks.log("INFO", "接收助手已停止监听".to_string());
        Ok(())
    }

    pub fn snapshot(&self) -> ReceiveSnapshot {
        let Ok(state) = self.state.lock() else {
            return ReceiveSnapshot {
                listening: false,
                dir: PathBuf::new(),
                ask: true,
                pending_ask: None,
                active_name: None,
                received_bytes: 0,
            };
        };
        ReceiveSnapshot {
            listening: state.listening,
            dir: state.dir.clone(),
            ask: state.ask,
            pending_ask: state.pending.as_ref().map(|pending| pending.notice.clone()),
            active_name: state.active_name.clone(),
            received_bytes: state.received_bytes,
        }
    }

    /// 应答接收询问；接受时返回计划落盘路径。
    pub fn answer(&self, ask_id: u64, accept: bool) -> Result<Option<PathBuf>, BtError> {
        let pending = {
            let mut state = self.state.lock().map_err(|_| BtError::internal("接收状态被投毒"))?;
            match state.pending.as_ref() {
                Some(pending) if pending.notice.id == ask_id => {}
                _ => return Err(BtError::not_found("没有待应答的接收询问")),
            }
            state.pending.take()
        };
        let Some(pending) = pending else {
            return Err(BtError::not_found("没有待应答的接收询问"));
        };
        let _ = pending.responder.send(accept);
        if accept {
            Ok(Some(pending.notice.planned_path))
        } else {
            Ok(None)
        }
    }

    fn accept_connection(self: &Arc<Self>, stream: Box<dyn TransportStream>) {
        if self.active.swap(true, Ordering::SeqCst) {
            let mut stream = stream;
            let _ = send_busy(stream.as_mut());
            self.hooks.log("WARN", "已有传输进行中，已拒绝并发连接".to_string());
            return;
        }
        let service = self.clone();
        let spawned = std::thread::Builder::new()
            .name("bt-receive".to_string())
            .spawn(move || {
                service.serve_stream(stream);
                service.active.store(false, Ordering::SeqCst);
            });
        if spawned.is_err() {
            self.active.store(false, Ordering::SeqCst);
        }
    }

    /// 同步处理一个连接；接收线程与测试共用。
    pub fn serve_stream(self: &Arc<Self>, mut stream: Box<dyn TransportStream>) {
        let device = stream.peer();
        let sink = match FileSink::new(self.dir()) {
            Ok(sink) => sink,
            Err(error) => {
                self.hooks.log("ERROR", format!("接收目录不可用：{error}"));
                let _ = send_busy(stream.as_mut());
                return;
            }
        };
        let approve_service = self.clone();
        let approve_sink = sink.clone();
        let mut approve = move |hello: &Hello| approve_service.should_accept(hello, device, &approve_sink);
        let progress_service = self.clone();
        let mut progress = move |hello: &Hello, sent: u64, total: u64| {
            if let Ok(mut state) = progress_service.state.lock() {
                state.received_bytes = sent;
                state.active_name = Some(hello.file_name.clone());
            }
            if let Some(hook) = progress_service.hooks.on_progress.as_ref() {
                hook(ReceiveProgressNotice {
                    name: hello.file_name.clone(),
                    bytes: sent,
                    total,
                });
            }
        };
        let log_service = self.clone();
        let mut log = move |event: crate::server::ServerEvent| {
            log_service.hooks.log(event.level, event.message);
        };
        let mut hooks = ServerHooks {
            approve: Some(&mut approve),
            progress: Some(&mut progress),
            log: Some(&mut log),
            idle_timeout: self.idle_timeout,
        };
        let outcome = serve_connection(stream.as_mut(), &sink, &mut hooks);
        match outcome {
            ServerOutcome::Completed { hello, path, sha256 } => {
                if let Ok(mut state) = self.state.lock() {
                    state.received_bytes = hello.file_size as u64;
                    state.active_name = None;
                }
                self.emit_done(ReceiveDoneNotice {
                    ok: true,
                    name: hello.file_name,
                    size: hello.file_size as u64,
                    path: Some(path),
                    sha256: Some(sha256),
                    error: None,
                    device,
                });
            }
            ServerOutcome::Rejected { hello } => {
                self.reset_progress();
                self.emit_done(ReceiveDoneNotice {
                    ok: false,
                    name: hello.file_name,
                    size: hello.file_size as u64,
                    path: None,
                    sha256: None,
                    error: Some("已拒绝接收".to_string()),
                    device,
                });
            }
            ServerOutcome::Canceled { hello } => {
                self.reset_progress();
                self.emit_done(ReceiveDoneNotice {
                    ok: false,
                    name: hello.file_name,
                    size: hello.file_size as u64,
                    path: None,
                    sha256: None,
                    error: Some("传输已取消，半成品已保留以便续传".to_string()),
                    device,
                });
            }
            ServerOutcome::Failed { hello, message, .. } => {
                self.reset_progress();
                let size = hello.as_ref().map(|hello| hello.file_size as u64).unwrap_or(0);
                self.emit_done(ReceiveDoneNotice {
                    ok: false,
                    name: hello
                        .map(|hello| hello.file_name)
                        .unwrap_or_else(|| "未知文件".to_string()),
                    size,
                    path: None,
                    sha256: None,
                    error: Some(message),
                    device,
                });
            }
            ServerOutcome::Busy => self.hooks.log("WARN", "接收端忙".to_string()),
        }
    }

    fn reset_progress(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.received_bytes = 0;
            state.active_name = None;
        }
    }

    fn should_accept(&self, hello: &Hello, device: Option<DeviceId>, sink: &FileSink) -> bool {
        if !self.snapshot().ask {
            return true;
        }
        let ask_id = self.next_ask.fetch_add(1, Ordering::SeqCst) + 1;
        let planned_path = sink.planned_path(hello).unwrap_or_else(|_| self.dir().join("unnamed"));
        let notice = ReceiveAskNotice {
            id: ask_id,
            name: hello.file_name.clone(),
            size: hello.file_size as u64,
            device,
            planned_path,
        };
        let (sender, receiver) = channel();
        if let Ok(mut state) = self.state.lock() {
            state.pending = Some(PendingAsk {
                notice: notice.clone(),
                responder: sender,
            });
        }
        if let Some(hook) = self.hooks.on_ask.as_ref() {
            hook(notice);
        }
        let accepted = receiver.recv_timeout(self.ask_timeout).unwrap_or(false);
        if let Ok(mut state) = self.state.lock() {
            if state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.notice.id == ask_id)
            {
                state.pending = None;
            }
        }
        if !accepted {
            self.hooks.log("INFO", format!("已拒绝接收：{}", hello.file_name));
        }
        accepted
    }

    fn emit_done(&self, notice: ReceiveDoneNotice) {
        if let Some(hook) = self.hooks.on_done.as_ref() {
            hook(notice);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{encode_data, encode_hello, read_frame_poll, Frame, FrameKind, Offer};
    use bt_core::{hex_lower, sha256};
    use bt_transport::mock::{pipe_pair, MockStream, MockTransport};
    use std::fs;
    use std::path::Path;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bt-receive-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn hello(name: &str, content: &[u8]) -> Hello {
        Hello::new(name, content.len() as i64, hex_lower(&sha256(content)), 4096)
    }

    fn start_service(dir: &Path, ask: bool, hooks: ReceiveHooks) -> (Arc<MockTransport>, Arc<ReceiveService>) {
        let transport = Arc::new(MockTransport::new(Vec::new()));
        let service = ReceiveService::start(transport.clone(), ReceiveSettings::new(dir, ask), hooks).unwrap();
        (transport, service)
    }

    fn drive_transfer(service: Arc<ReceiveService>, hello: &Hello, content: &[u8]) -> (Frame, Vec<Frame>) {
        let (mut client, server) = pipe_pair();
        let handle = std::thread::spawn(move || service.serve_stream(Box::new(server)));
        encode_hello(hello).unwrap().write_to(&mut client).unwrap();
        let offer = read_frame_poll(&mut client, || false, Duration::from_secs(2)).unwrap();
        assert_eq!(
            crate::protocol::Offer::decode(offer.payload()).unwrap().status,
            crate::protocol::OfferStatus::Accept
        );
        let mut acks = Vec::new();
        let mut offset = 0usize;
        for chunk in content.chunks(3) {
            let frame = Frame::new(FrameKind::Data, encode_data(offset as i64, chunk).unwrap()).unwrap();
            frame.write_to(&mut client).unwrap();
            let ack = read_frame_poll(&mut client, || false, Duration::from_secs(2)).unwrap();
            acks.push(ack);
            offset += chunk.len();
        }
        let done = read_frame_poll(&mut client, || false, Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
        (done, acks)
    }

    #[test]
    fn accepts_and_writes_file_and_callback() {
        let dir = temp_dir("accept");
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = notices.clone();
        let hooks = ReceiveHooks {
            on_done: Some(Arc::new(move |notice: ReceiveDoneNotice| {
                sink.lock().unwrap().push(notice);
            })),
            ..ReceiveHooks::default()
        };
        let (_transport, service) = start_service(&dir, false, hooks);
        assert!(service.snapshot().listening);
        let content = b"receive-me";
        let hello = hello("in.bin", content);
        let (done, acks) = drive_transfer(service, &hello, content);
        assert_eq!(acks.len(), 4);
        let done = crate::protocol::Done::decode(done.payload()).unwrap();
        assert!(done.ok);
        assert_eq!(fs::read(dir.join("in.bin")).unwrap(), content);
        let notices = notices.lock().unwrap();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].ok);
        assert_eq!(notices[0].path.as_deref(), Some(dir.join("in.bin").as_path()));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ask_flow_blocks_until_answer() {
        let dir = temp_dir("ask");
        let asks = Arc::new(Mutex::new(Vec::new()));
        let ask_sink = asks.clone();
        let hooks = ReceiveHooks {
            on_ask: Some(Arc::new(move |notice: ReceiveAskNotice| {
                ask_sink.lock().unwrap().push(notice);
            })),
            ..ReceiveHooks::default()
        };
        let (_transport, service) = start_service(&dir, true, hooks);
        let (mut client, server) = pipe_pair();
        let worker = service.clone();
        let handle = std::thread::spawn(move || worker.serve_stream(Box::new(server)));
        let content = b"ask";
        let hello = hello("ask.bin", content);
        encode_hello(&hello).unwrap().write_to(&mut client).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while asks.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let notice = asks.lock().unwrap()[0].clone();
        assert_eq!(notice.name, "ask.bin");
        assert_eq!(notice.size, 3);
        let planned = service.answer(notice.id, true).unwrap();
        assert!(planned.is_some());
        let offer = read_frame_poll(&mut client, || false, Duration::from_secs(2)).unwrap();
        assert_eq!(
            Offer::decode(offer.payload()).unwrap().status,
            crate::protocol::OfferStatus::Accept
        );
        let frame = Frame::new(FrameKind::Data, encode_data(0, content).unwrap()).unwrap();
        frame.write_to(&mut client).unwrap();
        let _ack = read_frame_poll(&mut client, || false, Duration::from_secs(2)).unwrap();
        let done = read_frame_poll(&mut client, || false, Duration::from_secs(2)).unwrap();
        assert!(crate::protocol::Done::decode(done.payload()).unwrap().ok);
        handle.join().unwrap();
        assert_eq!(fs::read(dir.join("ask.bin")).unwrap(), content);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reject_answer_sends_reject_offer() {
        let dir = temp_dir("reject");
        let asks = Arc::new(Mutex::new(Vec::new()));
        let ask_sink = asks.clone();
        let hooks = ReceiveHooks {
            on_ask: Some(Arc::new(move |notice: ReceiveAskNotice| {
                ask_sink.lock().unwrap().push(notice);
            })),
            ..ReceiveHooks::default()
        };
        let (_transport, service) = start_service(&dir, true, hooks);
        let (mut client, server) = pipe_pair();
        let worker = service.clone();
        let handle = std::thread::spawn(move || worker.serve_stream(Box::new(server)));
        encode_hello(&hello("no.bin", b"x"))
            .unwrap()
            .write_to(&mut client)
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while asks.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let notice = asks.lock().unwrap()[0].clone();
        assert_eq!(service.answer(notice.id, false).unwrap(), None);
        let offer = read_frame_poll(&mut client, || false, Duration::from_secs(2)).unwrap();
        assert_eq!(
            Offer::decode(offer.payload()).unwrap().status,
            crate::protocol::OfferStatus::Reject
        );
        handle.join().unwrap();
        assert!(!dir.join("no.bin").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_second_connection_gets_busy() {
        let dir = temp_dir("busy");
        let (_transport, service) = start_service(&dir, false, ReceiveHooks::default());
        let (mut first_client, first_server) = pipe_pair();
        service.accept_connection(Box::new(first_server));
        std::thread::sleep(Duration::from_millis(50));
        let second = MockStream::new(None, Vec::new());
        let probe = second.clone();
        service.accept_connection(Box::new(second));
        let written = probe.written();
        assert!(!written.is_empty());
        let offer = crate::protocol::Offer::decode(&written[7..]).unwrap();
        assert_eq!(offer.status, crate::protocol::OfferStatus::Busy);
        first_client.close_stream().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!service.active.load(Ordering::SeqCst));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn answer_without_pending_ask_reports_not_found() {
        let dir = temp_dir("noask");
        let (_transport, service) = start_service(&dir, true, ReceiveHooks::default());
        assert_eq!(
            service.answer(99, true).unwrap_err().kind(),
            bt_core::BtErrorKind::NotFound
        );
        let _ = fs::remove_dir_all(dir);
    }
}
