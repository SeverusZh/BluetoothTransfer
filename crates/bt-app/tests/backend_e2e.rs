use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bt_api::{AskId, Backend, ConfigPatch, Event, JobState, ReceiveOptions, RecordQuery, SendItem};
use bt_app::{build_backend, list_resume, AppPaths, RealBackend};
use bt_core::{
    hex_lower, sha256, Channel, Config, DeviceId, Direction, RecordKind, RecordStatus, SendSource, TransferMode,
};
use bt_engine::protocol::{encode_ack, encode_data, encode_done, encode_offer, Done, Frame, FrameKind, Hello, Offer};
use bt_transport::mock::{pipe_pair, MockTransport};
use bt_transport::PairAnswer;
use crossbeam_channel::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bt-app-e2e-{tag}-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn paths(&self) -> AppPaths {
        AppPaths::under(&self.0)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn device() -> DeviceId {
    DeviceId::parse("AA:BB:CC:DD:EE:10").unwrap()
}

fn write_file(dir: &TempDir, name: &str, content: &[u8]) -> PathBuf {
    let path = dir.0.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn wait_for(mut predicate: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    predicate()
}

fn drain_until(events: &Receiver<Event>, predicate: impl Fn(&Event) -> bool, timeout: Duration) -> Option<Event> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(remaining.min(Duration::from_millis(200))) {
            Ok(event) if predicate(&event) => return Some(event),
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    None
}

#[test]
fn opp_send_end_to_end_records_completed_job() {
    let dir = TempDir::new("opp");
    let transport = Arc::new(MockTransport::new(Vec::new()));
    let backend = RealBackend::open_with_transport(dir.paths(), Some(transport.clone())).unwrap();
    backend
        .update_config(ConfigPatch {
            mode: Some(TransferMode::Opp),
            receive_ask: Some(false),
            ..Default::default()
        })
        .unwrap();
    let mut inbound = obex_connect_ok();
    inbound.extend_from_slice(&obex_simple(bt_engine::obex::response::CONTINUE));
    inbound.extend_from_slice(&obex_simple(bt_engine::obex::response::OK));
    transport.script_connect(inbound);
    let content = b"opp-payload";
    let source = write_file(&dir, "payload.bin", content);
    let item = SendItem::from_source(SendSource::File(source));
    let events = backend.subscribe();
    let jobs = backend.enqueue(&device(), vec![item], None).unwrap();
    assert_eq!(jobs.len(), 1);
    let completed = wait_for(
        || backend.jobs().first().map(|job| job.state) == Some(JobState::Completed),
        Duration::from_secs(10),
    );
    assert!(completed, "OPP 任务未完成：{:?}", backend.jobs());
    drain_until(
        &events,
        |event| matches!(event, Event::JobStateChanged { .. }),
        Duration::from_secs(1),
    );
    let records = backend.records(RecordQuery::default()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, RecordStatus::Completed);
    assert_eq!(records[0].channel, Channel::Opp);
    assert_eq!(records[0].byte_size, content.len() as u64);
    assert_eq!(transport.service_requests()[0].1, bt_transport::OPP_SERVICE_UUID);
}

#[test]
fn assistant_send_end_to_end_uses_journal_and_records_completed() {
    let dir = TempDir::new("assistant");
    let transport = Arc::new(MockTransport::new(Vec::new()));
    let backend = RealBackend::open_with_transport(dir.paths(), Some(transport.clone())).unwrap();
    backend
        .update_config(ConfigPatch {
            mode: Some(TransferMode::Assistant),
            ..Default::default()
        })
        .unwrap();
    let content = b"assistant-payload-with-resume";
    let source = write_file(&dir, "payload.bin", content);
    let hello = Hello::new("payload.bin", content.len() as i64, hex_lower(&sha256(content)), 4096);
    let mut inbound = Vec::new();
    inbound.extend_from_slice(&encode_offer(&Offer::accept(0)).unwrap().encode());
    inbound.extend_from_slice(&encode_ack(content.len() as i64).unwrap().encode());
    inbound.extend_from_slice(
        &encode_done(&Done::new(true, hello.expected_sha256.clone()))
            .unwrap()
            .encode(),
    );
    transport.script_connect(inbound);
    let item = SendItem::from_source(SendSource::File(source));
    backend
        .enqueue(&device(), vec![item], Some(Channel::Assistant))
        .unwrap();
    let completed = wait_for(
        || backend.jobs().first().map(|job| job.state) == Some(JobState::Completed),
        Duration::from_secs(10),
    );
    assert!(completed, "助手任务未完成：{:?}", backend.jobs());
    let records = backend.records(RecordQuery::default()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, RecordStatus::Completed);
    assert_eq!(records[0].channel, Channel::Assistant);
    assert_eq!(records[0].kind, RecordKind::File);
    assert!(list_resume(&dir.paths()).unwrap().is_empty());
    assert_eq!(transport.service_requests()[0].1, bt_core::ASSISTANT_SERVICE_UUID);
}

#[test]
fn backend_can_open_without_auto_resuming_the_journal() {
    let dir = TempDir::new("no-auto-resume");
    let paths = dir.paths();
    paths.create_dirs().unwrap();
    let payload = paths.pending_dir.join("resume-payload.bin");
    std::fs::write(&payload, b"resume-me").unwrap();
    let entry = bt_engine::JournalEntry::new(
        device(),
        "resume.bin",
        payload,
        bt_core::sha256(b"resume-me"),
        9,
        bt_engine::JournalKind::File,
        false,
    );
    let journal = bt_engine::Journal::new(&paths.journal_file).with_base(&paths.pending_dir);
    let begun = journal.begin(entry).unwrap();
    journal.release(&begun.entry.id_hex(), false).unwrap();

    let transport = Arc::new(MockTransport::new(Vec::new()));
    let backend = RealBackend::open_with_transport_options(paths, Some(transport), false).unwrap();
    assert!(backend.jobs().is_empty(), "auto_resume=false 不应入队");
    assert_eq!(list_resume(&dir.paths()).unwrap().len(), 1, "journal 应保持原样");
}

#[test]
fn backend_auto_resume_enqueues_journal_entries() {
    let dir = TempDir::new("auto-resume");
    let paths = dir.paths();
    paths.create_dirs().unwrap();
    let payload = paths.pending_dir.join("resume-payload.bin");
    std::fs::write(&payload, b"resume-me").unwrap();
    let entry = bt_engine::JournalEntry::new(
        device(),
        "resume.bin",
        payload,
        bt_core::sha256(b"resume-me"),
        9,
        bt_engine::JournalKind::File,
        false,
    );
    let journal = bt_engine::Journal::new(&paths.journal_file).with_base(&paths.pending_dir);
    let begun = journal.begin(entry).unwrap();
    journal.release(&begun.entry.id_hex(), false).unwrap();

    let transport = Arc::new(MockTransport::new(Vec::new()));
    let backend = RealBackend::open_with_transport_options(paths, Some(transport), true).unwrap();
    assert!(
        wait_for(|| !backend.jobs().is_empty(), Duration::from_secs(5)),
        "auto_resume=true 应把 journal 条目重新入队"
    );
    assert_eq!(backend.jobs()[0].item.display_name, "resume.bin");
    backend.shutdown();
}

#[test]
fn pin_pairing_reaches_the_transport_handler() {
    let dir = TempDir::new("pin");
    let transport = Arc::new(MockTransport::new(Vec::new()));
    transport.set_pair_prompt(bt_transport::PairPromptKind::Pin);
    let backend = RealBackend::open_with_transport(dir.paths(), Some(transport.clone())).unwrap();
    let events = backend.subscribe();
    backend.pair(&device()).unwrap();
    let prompt = drain_until(
        &events,
        |event| matches!(event, Event::PairPrompt { .. }),
        Duration::from_secs(5),
    )
    .expect("未收到 PIN 配对请求");
    let Event::PairPrompt { request, kind, .. } = prompt else {
        unreachable!()
    };
    assert_eq!(kind, bt_api::PinPromptKind::Pin);
    assert!(
        backend.answer_prompt(request.id, true).is_err(),
        "PIN 请求必须走 answer_pin"
    );
    backend.answer_pin(request.id, "1234").unwrap();
    assert!(wait_for(
        || matches!(transport.last_pair_answer(), Some(PairAnswer::Pin(_))),
        Duration::from_secs(5)
    ));
    assert_eq!(transport.last_pair_answer(), Some(PairAnswer::Pin("1234".to_string())));
    let result = drain_until(
        &events,
        |event| matches!(event, Event::PairResult { .. }),
        Duration::from_secs(5),
    )
    .expect("未收到配对结果");
    assert!(matches!(result, Event::PairResult { ok: true, .. }));
}

#[test]
fn receive_ask_flow_writes_file_and_record() {
    let dir = TempDir::new("receive");
    let receive_dir = dir.0.join("incoming");
    let transport = Arc::new(MockTransport::new(Vec::new()));
    let backend = RealBackend::open_with_transport(dir.paths(), Some(transport.clone())).unwrap();
    backend
        .start_receive(ReceiveOptions {
            dir: Some(receive_dir.clone()),
            ask: Some(true),
        })
        .unwrap();
    assert!(backend.receive_session().listening);
    let events = backend.subscribe();
    let (mut client, server) = pipe_pair();
    assert!(transport.deliver(Box::new(server)));
    let content = b"incoming-content";
    let hello = Hello::new("receive.bin", content.len() as i64, hex_lower(&sha256(content)), 4096);
    encode_hello(&hello).unwrap().write_to(&mut client).unwrap();
    let ask = drain_until(
        &events,
        |event| matches!(event, Event::ReceiveAsk { .. }),
        Duration::from_secs(3),
    )
    .expect("未收到接收询问");
    let ask_id = match ask {
        Event::ReceiveAsk { id, .. } => id,
        _ => unreachable!(),
    };
    let planned = backend.answer_receive_ask(AskId(ask_id.0), true).unwrap();
    assert!(planned.is_some());
    let offer = bt_engine::protocol::read_frame_poll(&mut client, || false, Duration::from_secs(2)).unwrap();
    assert_eq!(offer.kind(), FrameKind::Offer);
    Frame::new(FrameKind::Data, encode_data(0, content).unwrap())
        .unwrap()
        .write_to(&mut client)
        .unwrap();
    let _ack = bt_engine::protocol::read_frame_poll(&mut client, || false, Duration::from_secs(2)).unwrap();
    let done = bt_engine::protocol::read_frame_poll(&mut client, || false, Duration::from_secs(2)).unwrap();
    assert_eq!(done.kind(), FrameKind::Done);
    assert!(wait_for(
        || receive_dir.join("receive.bin").is_file(),
        Duration::from_secs(3)
    ));
    assert_eq!(
        std::fs::metadata(receive_dir.join("receive.bin")).unwrap().len(),
        content.len() as u64,
        "接收最终文件不得追加元数据"
    );
    assert!(wait_for(
        || backend
            .records(RecordQuery::default())
            .map(|records| !records.is_empty())
            .unwrap_or(false),
        Duration::from_secs(3)
    ));
    let records = backend.records(RecordQuery::default()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].direction, Direction::Receive);
    assert_eq!(records[0].status, RecordStatus::Completed);
    assert_eq!(records[0].byte_size, content.len() as u64);
    assert_eq!(records[0].checksum, Some(sha256(content)));
    backend.stop_receive().unwrap();
    assert!(!backend.receive_session().listening);
}

#[test]
fn selftest_passes_with_mock_transport() {
    let dir = TempDir::new("selftest");
    let transport = Arc::new(MockTransport::new(Vec::new()));
    let backend = RealBackend::open_with_transport(dir.paths(), Some(transport)).unwrap();
    let report = backend.selftest().unwrap();
    assert!(report.passed(), "自检未通过：{:?}", report.failed_names());
}

#[test]
fn build_backend_honours_mock_environment() {
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new("mock");
    std::env::set_var("BT_BACKEND", "mock");
    let backend = build_backend(dir.paths()).unwrap();
    std::env::remove_var("BT_BACKEND");
    let devices = backend.devices();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].name, "Mock Phone");
    assert!(devices[0].paired);
    assert_eq!(devices[0].rssi, Some(-48));
    assert_eq!(devices[0].assistant_state, bt_core::AssistantState::Available);
    assert!(!dir.paths().data_dir.exists());
    let config = backend.config();
    assert_eq!(config, Config::default_for(dir.paths().receive_dir.clone()));
}

#[test]
fn real_backend_without_transport_reports_selftest_skips_and_rejects_device_ops() {
    let dir = TempDir::new("no-transport");
    let backend = RealBackend::open_with_transport(dir.paths(), None).unwrap();
    let report = backend.selftest().unwrap();
    assert!(report.passed(), "软件项应全部通过：{:?}", report.checks);
    assert!(backend.start_scan(Default::default()).is_err());
    assert!(backend
        .enqueue(
            &device(),
            vec![SendItem::from_source(SendSource::Text("x".into()))],
            None
        )
        .is_err());
    assert!(backend.receive_session().dir == dir.paths().receive_dir);
}

fn encode_hello(hello: &Hello) -> Result<Frame, bt_core::BtError> {
    Frame::new(FrameKind::Hello, hello.encode()?)
}

fn obex_connect_ok() -> Vec<u8> {
    let mut packet = bt_engine::obex::ObexPacket::new(bt_engine::obex::response::OK);
    packet.include_connect_fields = true;
    packet.max_packet_length = u16::MAX;
    packet.encode().unwrap()
}

fn obex_simple(code: u8) -> Vec<u8> {
    bt_engine::obex::ObexPacket::new(code).encode().unwrap()
}
