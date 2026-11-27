//! 无显示环境自检：构造全部页面状态并用内存后端驱动事件与动作。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bt_api::{Backend, Event, JobState, LogLevel, MockBackend, SelftestReport};
use bt_core::{DeviceId, Direction, Record, RecordKind, RecordStatus, SendSource};
use eframe::egui;

use crate::app::render_headless;
use crate::state::{Page, UiState};
use crate::theme;

pub fn run() -> Result<u8, bt_core::BtError> {
    let report = self_check();
    for check in &report.checks {
        let mark = if check.skipped {
            "skip"
        } else if check.ok {
            "ok"
        } else {
            "fail"
        };
        println!("[{mark}] {}: {}", check.name, check.detail);
    }
    Ok(if report.passed() { 0 } else { 1 })
}

pub fn self_check() -> SelftestReport {
    let mut report = SelftestReport::default();
    let backend = seeded_backend();
    let config = backend.config();
    let temp_dir = std::env::temp_dir().join(format!("bt-gui-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);
    let app_paths = bt_app::AppPaths::under(&temp_dir);
    let mut state = UiState::new(
        backend.clone(),
        Some(temp_dir.clone()),
        Some(app_paths.clone()),
        config.clone(),
        bt_platform::SendRequest::default(),
    );
    let ctx = egui::Context::default();
    theme::apply_theme(&ctx);
    let font = theme::install_cjk_font(&ctx);
    state.set_font_report(font.clone());
    seed_events(&backend, &state);
    state.poll();
    seed_actions(&mut state);
    state.poll();
    let gui_zip = gui_zip_action(&mut state, &temp_dir);
    report.push("gui_zip", gui_zip.0, gui_zip.1);
    for page in Page::ALL {
        state.page(page);
        render_headless(&mut state, &ctx);
        render_headless(&mut state, &ctx);
    }
    state.page = Page::Queue;
    render_headless(&mut state, &ctx);
    let pairing_ready = state.pairing.is_some();
    let dialog_output = ctx.run_ui(egui::RawInput::default(), |_ui| {
        crate::app::show_pairing_dialog(&mut state, &ctx);
    });
    dialog_output.drop_without_applying_deltas();

    report.push(
        "contract",
        bt_api::CONTRACT_VERSION >= 1,
        format!("契约版本 {}", bt_api::CONTRACT_VERSION),
    );
    report.push(
        "backend",
        backend.selftest().map(|report| report.passed()).unwrap_or(false),
        "内存后端自检可用",
    );
    report.push(
        "devices",
        state.devices.len() >= 3,
        format!("设备 {}", state.devices.len()),
    );
    report.push("events", state.events_seen > 10, format!("事件 {}", state.events_seen));
    report.push("jobs", state.jobs.len() >= 2, format!("任务 {}", state.jobs.len()));
    report.push(
        "records",
        state.records.len() >= 3,
        format!("记录 {}", state.records.len()),
    );
    report.push(
        "receive",
        !state.receive.completed.is_empty(),
        format!("本会话接收 {}", state.receive.completed.len()),
    );
    report.push(
        "pages",
        Page::ALL.len() == 7,
        "设备、发送、队列、接收、记录、日志、设置全部渲染",
    );
    report.push("dialogs", pairing_ready, "配对确认对话框已渲染");
    if font.found() {
        report.push("font", true, font.detail.clone());
    } else {
        report.push_skip(
            "font",
            format!("缺少中文系统字体：{}；设置 BT_FONT_PATH 后重跑", font.detail),
        );
    }
    report.push("config", state.config.validate().is_ok(), "配置在合法区间");

    let inbox_root = temp_dir.join("inbox");
    let inbox_check = inbox_round_trip(&inbox_root);
    report.push("inbox", inbox_check.0, inbox_check.1);

    let zip_check = zip_round_trip(&temp_dir, &app_paths);
    report.push("zip", zip_check.0, zip_check.1);

    let pin_check = pin_round_trip(&mut state, &backend);
    report.push("pin", pin_check.0, pin_check.1);

    let log_check = log_round_trip(&temp_dir);
    report.push("log", log_check.0, log_check.1);

    let _ = std::fs::remove_dir_all(&temp_dir);
    report
}

fn seeded_backend() -> Arc<MockBackend> {
    let first = DeviceId::parse("AA:BB:CC:DD:EE:01").unwrap();
    let second = DeviceId::parse("AA:BB:CC:DD:EE:02").unwrap();
    let third = DeviceId::parse("AA:BB:CC:DD:EE:03").unwrap();
    let mut favorite = bt_api::Device::new(first, "Pixel 9");
    favorite.alias = Some("工作手机".to_string());
    favorite.paired = true;
    favorite.connected = true;
    favorite.favorite = true;
    favorite.rssi = Some(-42);
    favorite.assistant_state = bt_core::AssistantState::Available;
    favorite.channels = vec![bt_core::Channel::Opp, bt_core::Channel::Assistant];
    let mut paired = bt_api::Device::new(second, "ThinkPad");
    paired.paired = true;
    paired.assistant_state = bt_core::AssistantState::Probing;
    let mut unpaired = bt_api::Device::new(third, "Speaker");
    unpaired.assistant_state = bt_core::AssistantState::Unavailable;
    let backend = MockBackend::new(vec![favorite, paired, unpaired]);
    for (index, status) in [
        RecordStatus::Completed,
        RecordStatus::Failed,
        RecordStatus::Canceled,
        RecordStatus::Interrupted,
    ]
    .into_iter()
    .enumerate()
    {
        let mut record = Record::running(
            if index.is_multiple_of(2) {
                Direction::Send
            } else {
                Direction::Receive
            },
            if index.is_multiple_of(2) {
                bt_core::Channel::Opp
            } else {
                bt_core::Channel::Assistant
            },
            if index == 0 { RecordKind::File } else { RecordKind::Text },
            format!("示例文件-{index}.txt"),
            format!("2026-11-2{}T10:00:00Z", index + 1),
        );
        record.id = (index + 1) as i64;
        record.device_name = "Pixel 9".to_string();
        record.status = status;
        record.byte_size = 1024 * (index as u64 + 1);
        record.transferred = record.byte_size;
        record.finished_at = Some("2026-11-27T10:00:10Z".to_string());
        if status == RecordStatus::Failed {
            record.error = Some("对端拒绝连接".to_string());
        }
        backend.add_record(record);
    }
    Arc::new(backend)
}

fn seed_events(backend: &MockBackend, state: &UiState) {
    let _ = state;
    let first = DeviceId::parse("AA:BB:CC:DD:EE:01").unwrap();
    backend.emit(Event::ScanStarted);
    backend.emit(Event::DeviceList(backend.devices()));
    backend.emit(Event::ScanFinished { count: 3 });
    backend.emit(Event::AssistantStateChanged {
        device_id: first,
        state: bt_core::AssistantState::Available,
    });
    backend.emit(Event::log(LogLevel::Info, "自检事件"));
    backend.emit(Event::log(LogLevel::Warn, "自检警告"));
    for (index, job_state) in [
        JobState::Queued,
        JobState::Sending,
        JobState::Paused,
        JobState::Completed,
        JobState::Failed,
    ]
    .into_iter()
    .enumerate()
    {
        let id = bt_api::JobId((index + 1) as u64);
        let mut job = bt_api::JobSnapshot {
            id,
            device_id: first,
            device_name: "Pixel 9".to_string(),
            item: bt_api::SendItem::from_source(SendSource::File(PathBuf::from(format!("/tmp/示例-{index}.bin")))),
            channel: if index % 2 == 0 {
                bt_core::Channel::Opp
            } else {
                bt_core::Channel::Assistant
            },
            state: job_state,
            bytes_sent: (index as u64) * 512,
            total_bytes: 4096,
            bps: 2048,
            eta_seconds: Some(3),
            attempts: 1,
            error: if job_state == JobState::Failed {
                Some("连接超时".to_string())
            } else {
                None
            },
            resumable: index % 2 == 0,
            created_at: "2026-11-27T10:00:00Z".to_string(),
            updated_at: "2026-11-27T10:00:05Z".to_string(),
        };
        if index == 4 {
            job.total_bytes = 0;
            job.bytes_sent = 0;
        }
        backend.emit(Event::JobAdded { job: job.clone() });
        backend.emit(Event::JobProgress {
            job_id: id,
            bytes: job.bytes_sent,
            total: job.total_bytes,
            bps: job.bps,
            eta_seconds: job.eta_seconds,
        });
        backend.emit(Event::JobStateChanged { job });
    }
    backend.emit(Event::ReceiveListeningChanged { listening: true });
    backend.emit(Event::ReceiveProgress {
        bytes: 2048,
        total: Some(4096),
        name: "接收示例.bin".to_string(),
    });
    backend.emit(Event::ReceiveAsk {
        id: bt_api::AskId(1),
        name: "待确认.bin".to_string(),
        size: 8192,
    });
    backend.emit(Event::ReceiveDone {
        ok: true,
        path: Some(PathBuf::from("/tmp/接收示例.bin")),
        error: None,
    });
    backend.emit(Event::ReceiveDone {
        ok: false,
        path: None,
        error: Some("对端中断".to_string()),
    });
    backend.emit(Event::QueueIdle);
    backend.emit(pair_prompt_event(first));
}

fn seed_actions(state: &mut UiState) {
    state.start_scan();
    state.poll();
    state.scanning = false;
    state.send.selected_device = Some(DeviceId::parse("AA:BB:CC:DD:EE:01").unwrap());
    state.send.channel = crate::state::ChannelChoice::Assistant;
    state.send.text = "自检文本".to_string();
    state.send_text();
    state.add_files(vec![PathBuf::from("/tmp/不存在-自检.bin")]);
    state.add_folder(PathBuf::from("/tmp"));
    state.send_pending();
    state.filter.search = "示例".to_string();
    state.mark_filter_dirty();
    state.poll();
    state.log_min_level = LogLevel::Info;
    state.settings.draft.opp_retry_count = 5;
    state.save_settings();
    state.settings.auth_input = "selftest-password".to_string();
    state.set_auth_password();
    state.clear_auth_password();
}

fn inbox_round_trip(root: &std::path::Path) -> (bool, String) {
    let inbox = bt_platform::ExternalInbox::new(root.to_path_buf());
    let file = root.join("payload.txt");
    let _ = std::fs::create_dir_all(root);
    std::fs::write(&file, b"x").unwrap();
    let items = vec![bt_platform::InboxItem::file(file.clone())];
    match inbox.write(&items, &[]).and_then(|_| inbox.drain()) {
        Ok(drain) => (
            drain.items.len() == 1 && drain.items[0].path == file,
            format!("收件箱往返 {} 项", drain.items.len()),
        ),
        Err(error) => (false, format!("收件箱失败：{}", error.message())),
    }
}

fn gui_zip_action(state: &mut UiState, dir: &Path) -> (bool, String) {
    let source = dir.join("gui-zip.txt");
    if std::fs::write(&source, b"gui zip payload").is_err() {
        return (false, "无法写入 zip 测试文件".to_string());
    }
    state.add_files(vec![source]);
    state.send.zip_single = true;
    state.send_pending();
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.send.busy && Instant::now() < deadline {
        state.poll();
        std::thread::sleep(Duration::from_millis(20));
    }
    state.poll();
    let packed = state.jobs.iter().any(|job| job.item.display_name == "gui-zip.txt.zip");
    let cleaned = state.zip_payloads.is_empty();
    (packed && cleaned, format!("zip 任务 {packed}，临时产物清理 {cleaned}"))
}

fn zip_round_trip(dir: &std::path::Path, paths: &bt_app::AppPaths) -> (bool, String) {
    let source = dir.join("zip-source.txt");
    std::fs::write(&source, b"selftest").unwrap();
    match bt_app::pack_zip_file_in(paths, &source) {
        Ok(packed) => {
            let bytes = std::fs::read(&packed.path).unwrap_or_default();
            let size = bytes.len();
            let source_size = source.metadata().map(|meta| meta.len()).unwrap_or(0);
            let valid = bytes.starts_with(b"PK\x03\x04") && size as u64 > source_size;
            (valid, format!("zip {} 字节", size))
        }
        Err(error) => (false, format!("zip 失败：{}", error.message())),
    }
}

fn pin_round_trip(state: &mut UiState, backend: &Arc<MockBackend>) -> (bool, String) {
    let Some(device) = state.devices.first().map(|device| device.id) else {
        return (false, "没有可配对设备".to_string());
    };
    backend.set_pair_prompt(bt_api::PinPromptKind::Pin);
    if let Err(error) = backend.pair(&device) {
        return (false, format!("配对请求失败：{}", error.message()));
    }
    state.poll();
    let prompted = matches!(
        state.pairing.as_ref().map(|pairing| pairing.kind),
        Some(bt_api::PinPromptKind::Pin)
    );
    if !prompted {
        return (false, "未进入 PIN 对话框".to_string());
    }
    state.pin_input = "1234".to_string();
    state.answer_pairing(true);
    state.poll();
    let ok = state.pairing.is_none() && backend.has_call("answer_pin");
    (
        ok,
        if ok {
            "PIN 已回填后端".to_string()
        } else {
            "PIN 未回填后端".to_string()
        },
    )
}

fn log_round_trip(dir: &std::path::Path) -> (bool, String) {
    let mut store = crate::log_sink::LogStore::open(&dir.join("logs"));
    store.info("自检日志");
    store.flush();
    let ok = store.path.as_ref().is_some_and(|path| path.is_file());
    (
        ok,
        if ok {
            "日志已落盘".to_string()
        } else {
            "日志文件未生成".to_string()
        },
    )
}

fn pair_prompt_event(device: DeviceId) -> Event {
    let request = bt_api::PinRequest {
        id: bt_api::PinRequestId(1),
        device,
        kind: bt_api::PinPromptKind::Passkey(123456),
    };
    Event::PairPrompt {
        request,
        device: bt_api::Device::new(device, "Pixel 9"),
        kind: bt_api::PinPromptKind::Passkey(123456),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes_without_display() {
        let report = self_check();
        assert!(report.passed(), "{:?}", report.checks);
    }

    #[test]
    fn all_pages_render() {
        let report = self_check();
        assert!(report.checks.iter().any(|check| check.name == "pages" && check.ok));
    }
}
