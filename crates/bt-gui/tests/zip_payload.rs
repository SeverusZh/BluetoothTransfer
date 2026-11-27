//! 验证取消发送时 GUI 不删除仍被 journal 引用的自产 zip。

use std::sync::Arc;
use std::time::{Duration, Instant};

use bt_api::{JobState, SharedBackend};
use bt_app::AppPaths;
use bt_gui::state::UiState;
use bt_transport::mock::MockTransport;

fn temp_root(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "bt-gui-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ))
}

fn wait_until(mut predicate: impl FnMut() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{message}");
}

#[test]
fn canceled_zip_job_keeps_payload_for_resume() {
    let root = temp_root("zip-cancel");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let paths = AppPaths::under(&root);
    paths.create_dirs().unwrap();
    let source = root.join("gui-zip-source.txt");
    std::fs::write(&source, b"gui zip resume").unwrap();

    let transport = Arc::new(MockTransport::new(Vec::new()));
    let backend: SharedBackend = Arc::new(
        bt_app::RealBackend::open_with_transport_options(paths.clone(), Some(transport.clone()), false).unwrap(),
    );
    let config = backend.config();
    let mut state = UiState::new(
        backend.clone(),
        Some(root.clone()),
        Some(paths.clone()),
        config,
        bt_platform::SendRequest::default(),
    );
    state.add_files(vec![source.clone()]);
    state.send.zip_single = true;
    state.send.selected_device = Some(bt_core::DeviceId::parse("AA:BB:CC:00:00:21").unwrap());

    let gate = transport.gate_next_connect();
    state.send_pending();
    wait_until(
        || {
            state.poll();
            bt_app::list_resume(&paths)
                .map(|entries| entries.iter().any(|entry| entry.state == "sending"))
                .unwrap_or(false)
        },
        "GUI zip 发送未写入 journal",
    );
    let job = backend
        .jobs()
        .into_iter()
        .find(|job| !job.state.is_terminal())
        .map(|job| job.id)
        .expect("存在进行中的 zip 任务");
    state.cancel_job(job);
    gate.release();
    wait_until(
        || {
            state.poll();
            backend
                .jobs()
                .iter()
                .any(|snapshot| snapshot.id == job && snapshot.state == JobState::Canceled)
        },
        "GUI zip 任务未进入取消终态",
    );
    state.poll();

    let entries = bt_app::list_resume(&paths).unwrap();
    assert_eq!(entries.len(), 1, "取消后 journal 条目必须保留");
    assert_eq!(entries[0].state, "paused");
    assert!(
        entries[0].payload.is_file(),
        "GUI 不得删除 paused 载荷：{}",
        entries[0].payload.display()
    );
    assert!(state.zip_payloads.is_empty(), "GUI 已收尾该载荷");
    assert!(source.is_file(), "用户源文件不受影响");

    state.backend().shutdown();
    drop(state);
    let _ = std::fs::remove_dir_all(&root);
}
