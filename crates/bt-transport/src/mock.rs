//! 可注入脚本的传输层替身，用于无硬件条件下的引擎与界面测试。

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use bt_core::{BtError, BtErrorKind, DeviceId};

use crate::{
    AcceptHandler, DiscoveredDevice, Discovery, PairAnswer, PairPrompt, PairPromptKind, Pairing, PromptHandler,
    RfcommConnector, RfcommService, TransportResult, TransportStream,
};

#[derive(Debug, Default)]
struct StreamState {
    inbound: VecDeque<u8>,
    outbound: Vec<u8>,
    peer: Option<DeviceId>,
    closed: bool,
    fail_reads: bool,
    stall: bool,
}

/// 内存单向流：读取取自预置入站缓冲，写入累积到出站缓冲。
#[derive(Debug, Clone)]
pub struct MockStream {
    state: Arc<Mutex<StreamState>>,
}

impl MockStream {
    pub fn new(peer: Option<DeviceId>, inbound: Vec<u8>) -> Self {
        let state = StreamState {
            inbound: inbound.into(),
            peer,
            ..StreamState::default()
        };
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub fn written(&self) -> Vec<u8> {
        self.state
            .lock()
            .map(|state| state.outbound.clone())
            .unwrap_or_default()
    }

    pub fn is_closed(&self) -> bool {
        self.state.lock().map(|state| state.closed).unwrap_or(false)
    }

    /// 模拟对端提前断开：后续读取直接失败。
    pub fn break_stream(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.fail_reads = true;
        }
    }

    /// 追加可读入站字节，便于分阶段脚本。
    pub fn push_inbound(&self, bytes: &[u8]) {
        if let Ok(mut state) = self.state.lock() {
            state.inbound.extend(bytes.iter().copied());
        }
    }

    /// 模拟只建立连接、不返回数据的链路；读取持续 WouldBlock，传输可按取消标志退出。
    pub fn set_stall(&self, stall: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.stall = stall;
        }
    }
}

impl Read for MockStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut state = self.state.lock().map_err(|_| std::io::Error::other("流状态被投毒"))?;
        if state.fail_reads {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "模拟连接中断",
            ));
        }
        if state.stall && state.inbound.is_empty() {
            drop(state);
            std::thread::sleep(Duration::from_millis(10));
            return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "模拟链路停顿"));
        }
        let count = buffer.len().min(state.inbound.len());
        for slot in buffer.iter_mut().take(count) {
            *slot = state.inbound.pop_front().unwrap_or(0);
        }
        Ok(count)
    }
}

impl Write for MockStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let mut state = self.state.lock().map_err(|_| std::io::Error::other("流状态被投毒"))?;
        if state.closed {
            return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "流已关闭"));
        }
        state.outbound.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TransportStream for MockStream {
    fn set_timeouts(&mut self, _read: Option<Duration>, _write: Option<Duration>) -> TransportResult<()> {
        Ok(())
    }

    fn peer(&self) -> Option<DeviceId> {
        self.state.lock().ok().and_then(|state| state.peer)
    }

    fn close_stream(&mut self) -> TransportResult<()> {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
        Ok(())
    }
}

/// 连接闸门：测试可在连接建立前暂停，验证取消与暂停状态迁移。
#[derive(Debug, Default)]
pub struct ConnectGate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl ConnectGate {
    pub fn release(&self) {
        if let Ok(mut open) = self.open.lock() {
            *open = true;
        }
        self.changed.notify_all();
    }

    fn wait(&self) {
        if let Ok(mut open) = self.open.lock() {
            while !*open {
                match self.changed.wait(open) {
                    Ok(guard) => open = guard,
                    Err(_) => return,
                }
            }
        }
    }
}

#[derive(Debug)]
enum ScriptedConnect {
    Bytes(Vec<u8>),
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceResult {
    Available(u8),
    Absent,
    Failure,
}

/// 传输层替身：可预置设备、连接脚本、服务解析结果与配对交互。
pub struct MockTransport {
    devices: Mutex<Vec<DiscoveredDevice>>,
    connects: Mutex<Vec<(DeviceId, u8)>>,
    service_requests: Mutex<Vec<(DeviceId, String)>>,
    scripts: Mutex<VecDeque<ScriptedConnect>>,
    inbound: Mutex<Vec<u8>>,
    streams: Mutex<Vec<MockStream>>,
    handler: Mutex<Option<AcceptHandler>>,
    service_uuid: Mutex<Option<String>>,
    channel: Mutex<Option<u8>>,
    fail_next_connect: AtomicBool,
    scan_count: AtomicUsize,
    pair_handler: Mutex<Option<PromptHandler>>,
    fail_next_pair: AtomicBool,
    pair_prompt: Mutex<PairPromptKind>,
    last_pair_answer: Mutex<Option<PairAnswer>>,
    service_result: Mutex<ServiceResult>,
    gate: Mutex<Option<Arc<ConnectGate>>>,
    stall: AtomicBool,
}

impl Default for MockTransport {
    fn default() -> Self {
        Self {
            devices: Mutex::new(Vec::new()),
            connects: Mutex::new(Vec::new()),
            service_requests: Mutex::new(Vec::new()),
            scripts: Mutex::new(VecDeque::new()),
            inbound: Mutex::new(Vec::new()),
            streams: Mutex::new(Vec::new()),
            handler: Mutex::new(None),
            service_uuid: Mutex::new(None),
            channel: Mutex::new(None),
            fail_next_connect: AtomicBool::new(false),
            scan_count: AtomicUsize::new(0),
            pair_handler: Mutex::new(None),
            fail_next_pair: AtomicBool::new(false),
            pair_prompt: Mutex::new(PairPromptKind::Confirm),
            last_pair_answer: Mutex::new(None),
            service_result: Mutex::new(ServiceResult::Available(3)),
            gate: Mutex::new(None),
            stall: AtomicBool::new(false),
        }
    }
}

impl MockTransport {
    pub fn new(devices: Vec<DiscoveredDevice>) -> Self {
        Self {
            devices: Mutex::new(devices),
            ..Self::default()
        }
    }

    pub fn set_devices(&self, devices: Vec<DiscoveredDevice>) {
        if let Ok(mut current) = self.devices.lock() {
            *current = devices;
        }
    }

    pub fn set_inbound(&self, bytes: Vec<u8>) {
        if let Ok(mut current) = self.inbound.lock() {
            *current = bytes;
        }
    }

    /// 为下一次连接排队脚本化入站字节。
    pub fn script_connect(&self, inbound: Vec<u8>) {
        if let Ok(mut scripts) = self.scripts.lock() {
            scripts.push_back(ScriptedConnect::Bytes(inbound));
        }
    }

    /// 为下一次连接排队失败。
    pub fn script_failure(&self) {
        if let Ok(mut scripts) = self.scripts.lock() {
            scripts.push_back(ScriptedConnect::Failure);
        }
    }

    pub fn fail_next_connect(&self) {
        self.fail_next_connect.store(true, Ordering::SeqCst);
    }

    pub fn fail_next_pair(&self) {
        self.fail_next_pair.store(true, Ordering::SeqCst);
    }

    pub fn set_pair_prompt(&self, kind: PairPromptKind) {
        if let Ok(mut current) = self.pair_prompt.lock() {
            *current = kind;
        }
    }

    /// 最近一次配对回调返回的应答。
    pub fn last_pair_answer(&self) -> Option<PairAnswer> {
        self.last_pair_answer.lock().ok().and_then(|answer| answer.clone())
    }

    /// 让新建的连接只保持打开、不返回任何入站数据。
    pub fn set_stall(&self, stall: bool) {
        self.stall.store(stall, Ordering::SeqCst);
    }

    pub fn set_service_absent(&self) {
        if let Ok(mut current) = self.service_result.lock() {
            *current = ServiceResult::Absent;
        }
    }

    /// 让下一次连接在返回前等待闸门释放。
    pub fn gate_next_connect(&self) -> Arc<ConnectGate> {
        let gate = Arc::new(ConnectGate::default());
        if let Ok(mut slot) = self.gate.lock() {
            *slot = Some(gate.clone());
        }
        gate
    }

    fn wait_gate(&self) {
        let gate = self.gate.lock().ok().and_then(|slot| slot.clone());
        if let Some(gate) = gate {
            gate.wait();
            if let Ok(mut slot) = self.gate.lock() {
                *slot = None;
            }
        }
    }

    pub fn set_service_failure(&self) {
        if let Ok(mut current) = self.service_result.lock() {
            *current = ServiceResult::Failure;
        }
    }

    pub fn set_service_channel(&self, channel: u8) {
        if let Ok(mut current) = self.service_result.lock() {
            *current = ServiceResult::Available(channel);
        }
    }

    pub fn connects(&self) -> Vec<(DeviceId, u8)> {
        self.connects
            .lock()
            .map(|connects| connects.clone())
            .unwrap_or_default()
    }

    pub fn service_requests(&self) -> Vec<(DeviceId, String)> {
        self.service_requests
            .lock()
            .map(|requests| requests.clone())
            .unwrap_or_default()
    }

    pub fn streams(&self) -> Vec<MockStream> {
        self.streams.lock().map(|streams| streams.clone()).unwrap_or_default()
    }

    pub fn last_stream(&self) -> Option<MockStream> {
        self.streams.lock().ok().and_then(|streams| streams.last().cloned())
    }

    pub fn last_written(&self) -> Vec<u8> {
        self.last_stream().map(|stream| stream.written()).unwrap_or_default()
    }

    pub fn scan_count(&self) -> usize {
        self.scan_count.load(Ordering::SeqCst)
    }

    pub fn service_uuid(&self) -> Option<String> {
        self.service_uuid.lock().ok().and_then(|uuid| uuid.clone())
    }

    /// 把预构造的流交给服务端 handler，模拟一次入站连接。
    pub fn deliver(&self, stream: Box<dyn TransportStream>) -> bool {
        let handler = self.handler.lock().ok().and_then(|slot| slot.clone());
        match handler {
            Some(handler) => {
                handler(stream);
                true
            }
            None => false,
        }
    }

    fn open_stream(&self, device: &DeviceId, inbound: Vec<u8>) -> TransportResult<Box<dyn TransportStream>> {
        let stream = MockStream::new(Some(*device), inbound);
        stream.set_stall(self.stall.load(Ordering::SeqCst));
        if let Ok(mut streams) = self.streams.lock() {
            streams.push(stream.clone());
        }
        Ok(Box::new(stream))
    }

    fn next_inbound(&self) -> TransportResult<Vec<u8>> {
        let script = self.scripts.lock().ok().and_then(|mut scripts| scripts.pop_front());
        match script {
            Some(ScriptedConnect::Bytes(bytes)) => Ok(bytes),
            Some(ScriptedConnect::Failure) => Err(BtError::timeout("模拟连接失败")),
            None => Ok(self.inbound.lock().map(|bytes| bytes.clone()).unwrap_or_default()),
        }
    }
}

impl Discovery for MockTransport {
    fn start_scan(&self, _duration: Option<Duration>) -> TransportResult<()> {
        self.scan_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn cancel_scan(&self) -> TransportResult<()> {
        Ok(())
    }

    fn devices(&self) -> Vec<DiscoveredDevice> {
        self.devices.lock().map(|devices| devices.clone()).unwrap_or_default()
    }
}

impl Pairing for MockTransport {
    fn pair(&self, device: &DeviceId) -> TransportResult<()> {
        if self.fail_next_pair.swap(false, Ordering::SeqCst) {
            return Err(BtError::new(BtErrorKind::Io, "模拟配对失败"));
        }
        let handler = self.pair_handler.lock().ok().and_then(|slot| slot.clone());
        if let Some(handler) = handler {
            let kind = self
                .pair_prompt
                .lock()
                .map(|kind| *kind)
                .unwrap_or(PairPromptKind::Confirm);
            let answer = handler(PairPrompt { device: *device, kind });
            if let Ok(mut last) = self.last_pair_answer.lock() {
                *last = Some(answer.clone());
            }
            if !answer.accepted() {
                return Err(BtError::permission("配对被拒绝"));
            }
            if kind == PairPromptKind::Pin && answer.pin().is_none_or(str::is_empty) {
                return Err(BtError::permission("配对需要 PIN，但回调未提供"));
            }
        }
        Ok(())
    }

    fn answer(&self, _accept: bool) -> TransportResult<()> {
        Ok(())
    }

    fn unpair(&self, _device: &DeviceId) -> TransportResult<()> {
        Ok(())
    }

    fn set_prompt_handler(&self, handler: Option<PromptHandler>) {
        if let Ok(mut slot) = self.pair_handler.lock() {
            *slot = handler;
        }
    }
}

impl RfcommConnector for MockTransport {
    fn connect_channel(&self, device: &DeviceId, channel: u8) -> TransportResult<Box<dyn TransportStream>> {
        self.wait_gate();
        if self.fail_next_connect.swap(false, Ordering::SeqCst) {
            return Err(BtError::timeout("模拟连接失败"));
        }
        let inbound = self.next_inbound()?;
        if let Ok(mut connects) = self.connects.lock() {
            connects.push((*device, channel));
        }
        self.open_stream(device, inbound)
    }

    fn connect_service(&self, device: &DeviceId, uuid: &str) -> TransportResult<Box<dyn TransportStream>> {
        self.wait_gate();
        if self.fail_next_connect.swap(false, Ordering::SeqCst) {
            return Err(BtError::timeout("模拟连接失败"));
        }
        if let Ok(mut requests) = self.service_requests.lock() {
            requests.push((*device, uuid.to_string()));
        }
        let inbound = self.next_inbound()?;
        self.open_stream(device, inbound)
    }

    fn find_service(&self, _device: &DeviceId, _uuid: &str) -> TransportResult<Option<u8>> {
        match self.service_result.lock().map(|result| *result) {
            Ok(ServiceResult::Available(channel)) => Ok(Some(channel)),
            Ok(ServiceResult::Absent) => Ok(None),
            Ok(ServiceResult::Failure) => Err(BtError::timeout("模拟 SDP 查询失败")),
            Err(_) => Err(BtError::internal("服务结果被投毒")),
        }
    }
}

impl RfcommService for MockTransport {
    fn start(&self, uuid: &str, handler: AcceptHandler) -> TransportResult<u8> {
        if let Ok(mut slot) = self.handler.lock() {
            *slot = Some(handler);
        }
        if let Ok(mut slot) = self.service_uuid.lock() {
            *slot = Some(uuid.to_string());
        }
        if let Ok(mut slot) = self.channel.lock() {
            *slot = Some(3);
        }
        Ok(3)
    }

    fn stop(&self) -> TransportResult<()> {
        if let Ok(mut slot) = self.handler.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.channel.lock() {
            *slot = None;
        }
        Ok(())
    }

    fn is_listening(&self) -> bool {
        self.channel.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }

    fn channel(&self) -> Option<u8> {
        self.channel.lock().map(|slot| *slot).unwrap_or(None)
    }
}

#[derive(Debug, Default)]
struct PipeState {
    queue: VecDeque<u8>,
    closed: bool,
}

struct PipeInner {
    state: Mutex<PipeState>,
    changed: Condvar,
}

impl PipeInner {
    fn new() -> Self {
        Self {
            state: Mutex::new(PipeState::default()),
            changed: Condvar::new(),
        }
    }
}

/// 双向内存管道的一端，读写分别落到对端的队列，用于客户端与服务端联调。
pub struct PipeStream {
    inbound: Arc<PipeInner>,
    outbound: Arc<PipeInner>,
    peer: Option<DeviceId>,
    read_timeout: Option<Duration>,
    closed: bool,
}

impl PipeStream {
    pub fn peer_id(&self) -> Option<DeviceId> {
        self.peer
    }
}

/// 创建一对互相连通的管道流。
pub fn pipe_pair() -> (PipeStream, PipeStream) {
    let first_in = Arc::new(PipeInner::new());
    let second_in = Arc::new(PipeInner::new());
    let first = PipeStream {
        inbound: first_in.clone(),
        outbound: second_in.clone(),
        peer: None,
        read_timeout: None,
        closed: false,
    };
    let second = PipeStream {
        inbound: second_in,
        outbound: first_in,
        peer: None,
        read_timeout: None,
        closed: false,
    };
    (first, second)
}

impl Read for PipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut state = self
            .inbound
            .state
            .lock()
            .map_err(|_| std::io::Error::other("管道被投毒"))?;
        loop {
            if !state.queue.is_empty() {
                let count = buffer.len().min(state.queue.len());
                for slot in buffer.iter_mut().take(count) {
                    *slot = state.queue.pop_front().unwrap_or(0);
                }
                return Ok(count);
            }
            if state.closed {
                return Ok(0);
            }
            state = match self.read_timeout {
                Some(timeout) => {
                    let (guard, result) = self
                        .inbound
                        .changed
                        .wait_timeout(state, timeout)
                        .map_err(|_| std::io::Error::other("管道被投毒"))?;
                    if result.timed_out() && guard.queue.is_empty() && !guard.closed {
                        return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "读超时"));
                    }
                    guard
                }
                None => self
                    .inbound
                    .changed
                    .wait(state)
                    .map_err(|_| std::io::Error::other("管道被投毒"))?,
            };
        }
    }
}

impl Write for PipeStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let mut state = self
            .outbound
            .state
            .lock()
            .map_err(|_| std::io::Error::other("管道被投毒"))?;
        if state.closed {
            return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "管道已关闭"));
        }
        state.queue.extend(buffer.iter().copied());
        self.outbound.changed.notify_all();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TransportStream for PipeStream {
    fn set_timeouts(&mut self, read: Option<Duration>, _write: Option<Duration>) -> TransportResult<()> {
        self.read_timeout = read;
        Ok(())
    }

    fn peer(&self) -> Option<DeviceId> {
        self.peer
    }

    fn close_stream(&mut self) -> TransportResult<()> {
        self.closed = true;
        if let Ok(mut state) = self.inbound.state.lock() {
            state.closed = true;
        }
        self.inbound.changed.notify_all();
        if let Ok(mut state) = self.outbound.state.lock() {
            state.closed = true;
        }
        self.outbound.changed.notify_all();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceId {
        DeviceId::parse("AA:BB:CC:DD:EE:03").unwrap()
    }

    #[test]
    fn connect_records_channel_and_returns_scripted_stream() {
        let transport = MockTransport::new(Vec::new());
        transport.script_connect(b"pong".to_vec());
        let mut stream = transport.connect_channel(&device(), 10).unwrap();
        let mut buffer = [0u8; 4];
        assert_eq!(stream.read(&mut buffer).unwrap(), 4);
        assert_eq!(&buffer, b"pong");
        assert_eq!(stream.read(&mut [0u8; 1]).unwrap(), 0);
        stream.write_all(b"ping").unwrap();
        assert_eq!(transport.connects(), vec![(device(), 10)]);
        assert_eq!(stream.peer(), Some(device()));
        assert_eq!(transport.last_written(), b"ping");
    }

    #[test]
    fn fail_next_connect_surfaces_timeout_once() {
        let transport = MockTransport::new(Vec::new());
        transport.fail_next_connect();
        assert!(transport.connect_channel(&device(), 10).is_err());
        assert!(transport.connect_channel(&device(), 10).is_ok());
    }

    #[test]
    fn scripted_connections_are_consumed_in_order() {
        let transport = MockTransport::new(Vec::new());
        transport.script_connect(b"one".to_vec());
        transport.script_failure();
        transport.script_connect(b"three".to_vec());
        let mut first = transport.connect_channel(&device(), 1).unwrap();
        let mut buffer = [0u8; 3];
        assert_eq!(first.read(&mut buffer).unwrap(), 3);
        assert!(transport.connect_channel(&device(), 1).is_err());
        let mut third = transport.connect_channel(&device(), 1).unwrap();
        let mut scripted = [0u8; 5];
        assert_eq!(third.read(&mut scripted).unwrap(), 5);
        assert_eq!(&scripted, b"three");
        assert_eq!(transport.connects().len(), 2);
    }

    #[test]
    fn service_start_records_uuid_and_delivers_to_handler() {
        let transport = MockTransport::new(Vec::new());
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = received.clone();
        let handler: AcceptHandler = Arc::new(move |mut stream: Box<dyn TransportStream>| {
            let mut buffer = [0u8; 3];
            let _ = stream.read(&mut buffer);
            if let Ok(mut sink) = sink.lock() {
                sink.push(buffer.to_vec());
            }
        });
        assert!(!transport.is_listening());
        let channel = transport.start("uuid", handler).unwrap();
        assert_eq!(channel, 3);
        assert!(transport.is_listening());
        assert_eq!(transport.service_uuid().as_deref(), Some("uuid"));
        let stream = MockStream::new(Some(device()), b"abc".to_vec());
        assert!(transport.deliver(Box::new(stream)));
        assert_eq!(received.lock().unwrap()[0], b"abc");
        transport.start_scan(None).unwrap();
        assert_eq!(transport.scan_count(), 1);
    }

    #[test]
    fn find_service_reports_three_states() {
        let transport = MockTransport::new(Vec::new());
        assert_eq!(transport.find_service(&device(), "uuid").unwrap(), Some(3));
        transport.set_service_absent();
        assert_eq!(transport.find_service(&device(), "uuid").unwrap(), None);
        transport.set_service_failure();
        assert!(transport.find_service(&device(), "uuid").is_err());
        transport.set_service_channel(7);
        assert_eq!(transport.find_service(&device(), "uuid").unwrap(), Some(7));
    }

    #[test]
    fn pair_uses_prompt_handler_and_can_fail() {
        let transport = MockTransport::new(Vec::new());
        let accepted = Arc::new(AtomicBool::new(true));
        let flag = accepted.clone();
        transport.set_prompt_handler(Some(Arc::new(move |_prompt: PairPrompt| {
            if flag.load(Ordering::SeqCst) {
                PairAnswer::Accept
            } else {
                PairAnswer::Reject
            }
        })));
        transport.pair(&device()).unwrap();
        accepted.store(false, Ordering::SeqCst);
        assert!(transport.pair(&device()).is_err());
        transport.set_prompt_handler(None);
        transport.fail_next_pair();
        assert!(transport.pair(&device()).is_err());
    }

    #[test]
    fn pin_prompt_carries_the_entered_value() {
        let transport = MockTransport::new(Vec::new());
        transport.set_pair_prompt(PairPromptKind::Pin);
        transport.set_prompt_handler(Some(Arc::new(|_prompt: PairPrompt| {
            PairAnswer::Pin("1234".to_string())
        })));
        transport.pair(&device()).unwrap();
        assert_eq!(transport.last_pair_answer(), Some(PairAnswer::Pin("1234".to_string())));

        transport.set_prompt_handler(Some(Arc::new(|_prompt: PairPrompt| PairAnswer::Accept)));
        assert!(transport.pair(&device()).is_err(), "PIN 类请求缺少 PIN 必须失败");
    }

    #[test]
    fn pipe_pair_transfers_bytes_both_ways() {
        let (mut client, mut server) = pipe_pair();
        client.write_all(b"hello").unwrap();
        let mut buffer = [0u8; 5];
        assert_eq!(server.read(&mut buffer).unwrap(), 5);
        assert_eq!(&buffer, b"hello");
        server.write_all(b"world").unwrap();
        let mut echo = [0u8; 5];
        assert_eq!(client.read(&mut echo).unwrap(), 5);
        assert_eq!(&echo, b"world");
        server.close_stream().unwrap();
        assert_eq!(client.read(&mut echo).unwrap(), 0);
    }

    #[test]
    fn pipe_blocks_with_timeout_when_no_data() {
        let (_client, mut server) = pipe_pair();
        server.set_timeouts(Some(Duration::from_millis(10)), None).unwrap();
        let error = server.read(&mut [0u8; 1]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    }
}
