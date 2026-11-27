//! 装配根：把契约、存储、引擎与平台传输拼装成后端实例。
//!
//! GUI 与 CLI 都通过本 crate 的 build_backend 拿到 Arc 化后端，
//! 不直接依赖 bt-engine 与 bt-store，保证上层实现可替换。

pub mod backend;
pub mod paths;

pub use backend::RealBackend;
pub use paths::AppPaths;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bt_api::{Backend, Device, EventBus, MockBackend, SharedBackend};
use bt_core::{now_rfc3339, AssistantState, BtError, Channel, DeviceId};
use bt_engine::{ArtifactCleaner, Journal, JournalState, PackedSource, PendingPackageStore};

/// 后端装配选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendOptions {
    /// 打开时是否自动恢复 journal 中的未完成传输。
    pub auto_resume: bool,
}

impl Default for BackendOptions {
    fn default() -> Self {
        Self { auto_resume: true }
    }
}

/// 构建后端；环境变量 BT_BACKEND=mock 时返回内存 MockBackend。
pub fn build_backend(paths: AppPaths) -> Result<Box<dyn Backend>, BtError> {
    build_backend_with_options(paths, BackendOptions::default())
}

/// 按显式选项构建后端；mock 后端忽略 auto_resume。
pub fn build_backend_with_options(paths: AppPaths, options: BackendOptions) -> Result<Box<dyn Backend>, BtError> {
    if mock_requested() {
        return Ok(Box::new(mock_backend(&paths)?));
    }
    if let Some(transport) = scripted_test_transport() {
        return Ok(Box::new(RealBackend::open_with_transport_options(
            paths,
            Some(transport),
            options.auto_resume,
        )?));
    }
    Ok(Box::new(RealBackend::open_with_options(paths, options.auto_resume)?))
}

/// 构建可跨线程共享的后端句柄。
pub fn build_backend_shared(paths: AppPaths) -> Result<SharedBackend, BtError> {
    build_backend_shared_with_options(paths, BackendOptions::default())
}

/// 按显式选项构建共享后端句柄。
pub fn build_backend_shared_with_options(paths: AppPaths, options: BackendOptions) -> Result<SharedBackend, BtError> {
    if mock_requested() {
        return Ok(Arc::new(mock_backend(&paths)?));
    }
    if let Some(transport) = scripted_test_transport() {
        return Ok(Arc::new(RealBackend::open_with_transport_options(
            paths,
            Some(transport),
            options.auto_resume,
        )?));
    }
    Ok(Arc::new(RealBackend::open_with_options(paths, options.auto_resume)?))
}

/// 调试构建专用测试入口：`BT_TEST_TRANSPORT=mock` 为真实后端注入脚本化传输层。
/// 发布构建编译为空实现，生产二进制不暴露替身传输开关。
fn scripted_test_transport() -> Option<Arc<dyn bt_transport::BluetoothTransport>> {
    #[cfg(debug_assertions)]
    {
        let mode = std::env::var("BT_TEST_TRANSPORT").ok()?;
        if mode.eq_ignore_ascii_case("mock") {
            return Some(Arc::new(bt_transport::mock::MockTransport::new(Vec::new())));
        }
    }
    None
}

pub fn mock_requested() -> bool {
    std::env::var("BT_BACKEND")
        .map(|value| value.eq_ignore_ascii_case("mock"))
        .unwrap_or(false)
}

/// 预置两台确定性设备的 MockBackend，供 CLI e2e 与 GUI 自检重放。
pub fn mock_backend(paths: &AppPaths) -> Result<MockBackend, BtError> {
    let mut phone = Device::new(DeviceId::parse("AA:BB:CC:00:00:01")?, "Mock Phone");
    phone.paired = true;
    phone.rssi = Some(-48);
    phone.last_seen = Some("2026-01-01T00:00:00Z".to_string());
    phone.assistant_state = AssistantState::Available;
    phone.channels = vec![Channel::Opp, Channel::Assistant];
    let mut tablet = Device::new(DeviceId::parse("AA:BB:CC:00:00:02")?, "Mock Tablet");
    tablet.paired = false;
    tablet.rssi = Some(-60);
    tablet.last_seen = Some("2026-01-01T00:00:00Z".to_string());
    tablet.channels = vec![Channel::Opp];
    Ok(MockBackend::with_config(paths.default_config(), vec![phone, tablet]))
}

/// 续传条目摘要，供 CLI 展示与取消。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeInfo {
    pub id: String,
    pub peer: String,
    pub display_name: String,
    pub payload: PathBuf,
    pub size: u64,
    pub state: &'static str,
    pub attempts: u32,
    pub updated_at: String,
}

pub fn list_resume(paths: &AppPaths) -> Result<Vec<ResumeInfo>, BtError> {
    if !paths.journal_file.is_file() {
        return Ok(Vec::new());
    }
    let journal = journal_for(paths);
    let entries = journal.read_all()?;
    Ok(entries.into_iter().map(|entry| resume_info(&entry)).collect())
}

/// 一条可恢复条目被放弃的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeAbandoned {
    pub info: ResumeInfo,
    pub reason: String,
}

/// journal 恢复计划：可重发的条目、应放弃的条目与正在其他进程发送的条目。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResumePlan {
    pub recoverable: Vec<ResumeInfo>,
    pub abandoned: Vec<ResumeAbandoned>,
    pub busy: Vec<ResumeInfo>,
}

/// 按 BTJ1 journal 语义规划续传；显式恢复时把 paused 条目也视为可恢复。
pub fn resume_plan(paths: &AppPaths) -> Result<ResumePlan, BtError> {
    if !paths.journal_file.is_file() {
        return Ok(ResumePlan::default());
    }
    let journal = journal_for(paths);
    let recovery = journal.plan_recovery()?;
    let all = journal.read_all()?;
    let mut plan = ResumePlan::default();
    let mut known = std::collections::HashSet::new();
    for entry in recovery.recoverable {
        known.insert(entry.id_hex());
        plan.recoverable.push(resume_info(&entry));
    }
    for abandoned in recovery.abandoned {
        known.insert(abandoned.entry.id_hex());
        plan.abandoned.push(ResumeAbandoned {
            info: resume_info(&abandoned.entry),
            reason: abandoned.reason,
        });
    }
    let now = u64::try_from(bt_core::unix_now()).unwrap_or(0);
    for entry in all {
        let id = entry.id_hex();
        if known.contains(&id) {
            continue;
        }
        match entry.state {
            JournalState::Paused => {
                if now.saturating_sub(u64::from(entry.updated)) > bt_engine::TTL.as_secs() {
                    plan.abandoned.push(ResumeAbandoned {
                        info: resume_info(&entry),
                        reason: "已超过 7 天保留期，放弃断点续传".to_string(),
                    });
                } else if entry.attempts >= bt_engine::MAX_ATTEMPTS {
                    plan.abandoned.push(ResumeAbandoned {
                        info: resume_info(&entry),
                        reason: format!("已尝试 {} 次仍未完成，放弃断点续传", entry.attempts),
                    });
                } else if let Err(reason) = entry.validate_payload() {
                    plan.abandoned.push(ResumeAbandoned {
                        info: resume_info(&entry),
                        reason,
                    });
                } else {
                    plan.recoverable.push(resume_info(&entry));
                }
            }
            JournalState::Sending => plan.busy.push(resume_info(&entry)),
            JournalState::Pending => {}
        }
    }
    Ok(plan)
}

/// 删除被放弃的 journal 条目与自产载荷。
pub fn drop_resume(paths: &AppPaths, ids: &[String]) -> Result<usize, BtError> {
    if ids.is_empty() || !paths.journal_file.is_file() {
        return Ok(0);
    }
    journal_for(paths).apply_abandoned(ids)
}

pub fn cancel_resume(paths: &AppPaths, id: &str) -> Result<bool, BtError> {
    if !paths.journal_file.is_file() {
        return Ok(false);
    }
    journal_for(paths).cancel(id)
}

/// 按 journal 引用收尾自产载荷：仍被未完成条目引用则保留，否则删除。
/// 用户源文件不在 pending 目录，删除请求会被打包器拒绝。
pub fn settle_owned_payload(paths: &AppPaths, payload: &Path) -> bool {
    let packages = PendingPackageStore::new(paths.pending_dir.clone());
    if !packages.is_owned(payload) {
        return false;
    }
    if paths.journal_file.is_file() {
        match journal_for(paths).read_all() {
            Ok(entries) if entries.iter().any(|entry| same_payload(&entry.payload, payload)) => return false,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    packages.delete(payload)
}

fn same_payload(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// journal 删除自产条目时同步清理 pending/ 下的打包产物。
fn journal_for(paths: &AppPaths) -> Journal {
    Journal::new(&paths.journal_file)
        .with_base(&paths.pending_dir)
        .with_cleaner(Arc::new(PendingArtifactCleaner {
            packages: PendingPackageStore::new(&paths.pending_dir),
        }))
}

struct PendingArtifactCleaner {
    packages: PendingPackageStore,
}

impl ArtifactCleaner for PendingArtifactCleaner {
    fn delete(&self, path: &Path) -> bool {
        self.packages.delete(path)
    }
}

fn resume_info(entry: &bt_engine::JournalEntry) -> ResumeInfo {
    ResumeInfo {
        id: entry.id_hex(),
        peer: entry.peer.to_string(),
        display_name: entry.display_name.clone(),
        payload: entry.payload.clone(),
        size: entry.size,
        state: entry.state.as_str(),
        attempts: entry.attempts,
        updated_at: bt_core::rfc3339_from_unix(i64::from(entry.updated)),
    }
}

/// 把单个文件打包为待续传 zip，产物位于给定路径布局的 pending/。
pub fn pack_zip_file_in(paths: &AppPaths, source: &Path) -> Result<PackedSource, BtError> {
    paths.create_dirs()?;
    PendingPackageStore::new(paths.pending_dir.clone()).create_zip_from_file(source)
}

/// 把文件夹打包为待续传 zip，产物位于给定路径布局的 pending/。
pub fn pack_zip_folder_in(paths: &AppPaths, folder: &Path) -> Result<PackedSource, BtError> {
    paths.create_dirs()?;
    PendingPackageStore::new(paths.pending_dir.clone()).create_zip_from_folder(folder)
}

/// 校验文件夹打包是否满足 ZIP32 上限，不写产物；供命令层在入队前报错。
pub fn check_zip_folder(folder: &Path) -> Result<(), BtError> {
    bt_engine::check_folder_zip32(folder)
}

/// 把单个文件打包为待续传 zip；路径按 BT_HOME 或当前用户目录解析。
pub fn pack_zip_file(source: &Path) -> Result<PackedSource, BtError> {
    let paths = AppPaths::resolve()?;
    pack_zip_file_in(&paths, source)
}

/// 把文件夹打包为待续传 zip；路径按 BT_HOME 或当前用户目录解析。
pub fn pack_zip_folder(folder: &Path) -> Result<PackedSource, BtError> {
    let paths = AppPaths::resolve()?;
    pack_zip_folder_in(&paths, folder)
}

/// 装配层持有异步运行时，供后续慢操作的调度线程复用。
pub fn build_runtime() -> Result<tokio::runtime::Runtime, BtError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| BtError::internal(format!("运行时创建失败：{error}")))
}

/// 便于调用方展示的事件总线实例。
pub fn new_bus() -> EventBus {
    EventBus::new()
}

/// 当前时间的文本形式，保持全库时间格式一致。
pub fn now_text() -> String {
    now_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_api::JobState;
    use bt_engine::JournalState;

    fn temp_paths(tag: &str) -> AppPaths {
        let root = std::env::temp_dir().join(format!(
            "bt-app-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        AppPaths::under(&root)
    }

    #[test]
    fn mock_backend_has_stable_devices_and_completes_enqueue() {
        let paths = temp_paths("mock");
        let backend = mock_backend(&paths).unwrap();
        let devices = backend.devices();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "Mock Phone");
        assert!(devices[0].paired);
        assert_eq!(devices[0].rssi, Some(-48));
        assert_eq!(devices[0].assistant_state, AssistantState::Available);
        assert_eq!(devices[1].name, "Mock Tablet");
        assert!(!devices[1].paired);
        let item = bt_api::SendItem::from_source(bt_core::SendSource::Text("hello".into()));
        let jobs = backend.enqueue(&devices[0].id, vec![item], None).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(backend.jobs()[0].state, JobState::Completed);
        assert!(!paths.data_dir.exists());
    }

    #[test]
    fn resume_helpers_return_empty_without_journal() {
        let paths = temp_paths("resume-empty");
        assert!(list_resume(&paths).unwrap().is_empty());
        assert!(!cancel_resume(&paths, "deadbeefdeadbeef").unwrap());
    }

    #[test]
    fn mock_switch_reads_environment() {
        std::env::set_var("BT_BACKEND", "mock");
        assert!(mock_requested());
        std::env::remove_var("BT_BACKEND");
        assert!(!mock_requested());
    }

    #[test]
    fn resume_helpers_round_trip_real_journal() {
        let paths = temp_paths("resume-real");
        paths.create_dirs().unwrap();
        let payload = paths
            .pending_dir
            .join("bt_asst_pending_0123456789abcdef0123456789abcdef.bin");
        std::fs::write(&payload, b"payload").unwrap();
        let journal = Journal::new(&paths.journal_file).with_base(&paths.pending_dir);
        let entry = bt_engine::JournalEntry::new(
            DeviceId::parse("AA:BB:CC:00:00:03").unwrap(),
            "报告.bin",
            payload.clone(),
            bt_core::sha256(b"payload"),
            7,
            bt_engine::JournalKind::File,
            true,
        );
        let began = journal.begin(entry).unwrap();
        let entries = list_resume(&paths).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, began.entry.id_hex());
        assert_eq!(entries[0].payload, payload);
        assert_eq!(entries[0].peer, "AA:BB:CC:00:00:03");
        assert_eq!(entries[0].state, "sending");
        assert!(cancel_resume(&paths, &entries[0].id).unwrap());
        assert!(list_resume(&paths).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }

    #[test]
    fn settle_owned_payload_follows_journal_references() {
        let paths = temp_paths("settle");
        paths.create_dirs().unwrap();
        let source = paths.data_dir.join("source.bin");
        std::fs::write(&source, b"source").unwrap();
        let packages = PendingPackageStore::new(paths.pending_dir.clone());
        let packed = packages.create_zip_from_file(&source).unwrap();
        assert!(settle_owned_payload(&paths, &packed.path), "无 journal 引用时应删除");
        assert!(!packed.path.exists());

        let packed = packages.create_zip_from_file(&source).unwrap();
        let entry = bt_engine::JournalEntry::new(
            DeviceId::parse("AA:BB:CC:00:00:03").unwrap(),
            packed.display_name.clone(),
            packed.path.clone(),
            bt_core::sha256(&std::fs::read(&packed.path).unwrap()),
            std::fs::metadata(&packed.path).unwrap().len(),
            bt_engine::JournalKind::FileZip,
            true,
        );
        let journal = Journal::new(&paths.journal_file).with_base(&paths.pending_dir);
        let id = journal.begin(entry).unwrap().entry.id_hex();
        journal.pause(&id).unwrap();
        assert!(
            !settle_owned_payload(&paths, &packed.path),
            "paused 条目引用的载荷必须保留"
        );
        assert!(packed.path.is_file());
        assert!(
            !settle_owned_payload(&paths, &source),
            "用户源文件不在 pending，永不由本函数删除"
        );
        assert!(source.is_file());
        assert!(cancel_resume(&paths, &id).unwrap());
        assert!(!packed.path.exists(), "journal 删除条目时清理自产载荷");
        let _ = std::fs::remove_dir_all(&paths.data_dir);
    }

    #[test]
    fn journal_state_names_are_stable() {
        assert_eq!(JournalState::Pending.as_str(), "pending");
        assert_eq!(JournalState::Paused.as_str(), "paused");
    }
}
