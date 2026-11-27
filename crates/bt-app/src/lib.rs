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
use bt_engine::{Journal, JournalState, PackedSource, PendingPackageStore};

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
    Ok(Arc::new(RealBackend::open_with_options(paths, options.auto_resume)?))
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
    let journal = Journal::new(&paths.journal_file).with_base(&paths.pending_dir);
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
    let journal = Journal::new(&paths.journal_file).with_base(&paths.pending_dir);
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
    Journal::new(&paths.journal_file)
        .with_base(&paths.pending_dir)
        .apply_abandoned(ids)
}

pub fn cancel_resume(paths: &AppPaths, id: &str) -> Result<bool, BtError> {
    if !paths.journal_file.is_file() {
        return Ok(false);
    }
    Journal::new(&paths.journal_file)
        .with_base(&paths.pending_dir)
        .cancel(id)
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
    fn journal_state_names_are_stable() {
        assert_eq!(JournalState::Pending.as_str(), "pending");
        assert_eq!(JournalState::Paused.as_str(), "paused");
    }
}
