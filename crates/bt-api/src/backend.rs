use std::path::PathBuf;
use std::sync::Arc;

use bt_core::{BtError, Channel, Config, DeviceId, Record};
use crossbeam_channel::Receiver;

use crate::dto::{
    AskId, ConfigPatch, CopyPlan, Device, ExportFormat, ExportSummary, JobId, JobSnapshot, MigrateOptions,
    MigrateReport, PinRequestId, ReceiveOptions, ReceiveSession, RecordQuery, ScanOptions, SelftestReport, SendItem,
    Stats,
};
use crate::event::Event;

/// 装配层共享的后端句柄。
pub type SharedBackend = Arc<dyn Backend>;

/// 后端契约。
///
/// 约定：
/// - 所有实现必须 `Send + Sync`，可被多线程共享；
/// - 扫描、配对、探测、入队、启停接收、导出、迁移、自检属于慢操作，只排队并立即返回，
///   进度通过 [`Event`] 回传，禁止阻塞调用线程；
/// - 配置、设备快照、任务快照、接收会话属于快查询，同步返回缓存；
/// - 记录类读写同步返回结果；
/// - 事件通过 [`Backend::subscribe`] 获取独立通道，GUI 每帧轮询，CLI 阻塞带超时。
pub trait Backend: Send + Sync {
    /// 当前配置快照。
    fn config(&self) -> Config;

    /// 应用配置增量并返回归一化后的配置；成功后广播 `ConfigChanged`。
    fn update_config(&self, patch: ConfigPatch) -> Result<Config, BtError>;

    /// 设置或清除 OPP 认证密码。密码永不进入配置与配置文件。
    fn set_auth_password(&self, password: Option<&str>) -> Result<(), BtError>;

    /// 设备缓存快照。
    fn devices(&self) -> Vec<Device>;

    /// 开始扫描；结果通过 `DeviceList` 与 `ScanFinished` 回传。
    fn start_scan(&self, options: ScanOptions) -> Result<(), BtError>;

    fn cancel_scan(&self) -> Result<(), BtError>;

    /// 发起配对；需要用户输入的场合通过 `PairPrompt` 回传。
    fn pair(&self, device: &DeviceId) -> Result<(), BtError>;

    /// 应答配对交互。
    fn answer_prompt(&self, request: PinRequestId, accept: bool) -> Result<(), BtError>;

    /// 设置设备别名，传 None 清除别名。
    fn set_alias(&self, device: &DeviceId, alias: Option<&str>) -> Result<(), BtError>;

    fn set_favorite(&self, device: &DeviceId, favorite: bool) -> Result<(), BtError>;

    /// 探测对端助手通道可用性；结果通过 `AssistantStateChanged` 回传。
    fn detect(&self, device: &DeviceId) -> Result<(), BtError>;

    /// 打开操作系统的蓝牙设置界面。
    fn open_bluetooth_settings(&self) -> Result<(), BtError>;

    /// 入队发送条目；`channel` 为空时按配置的通道模式决定。
    fn enqueue(&self, device: &DeviceId, items: Vec<SendItem>, channel: Option<Channel>)
        -> Result<Vec<JobId>, BtError>;

    /// 任务快照列表，按入队顺序返回。
    fn jobs(&self) -> Vec<JobSnapshot>;

    fn pause_job(&self, job: JobId) -> Result<(), BtError>;
    fn resume_job(&self, job: JobId) -> Result<(), BtError>;
    fn cancel_job(&self, job: JobId) -> Result<(), BtError>;
    fn cancel_all_jobs(&self) -> Result<(), BtError>;
    fn retry_job(&self, job: JobId) -> Result<(), BtError>;
    /// 从队列移除任务，仅终态任务可移除。
    fn remove_job(&self, job: JobId) -> Result<(), BtError>;

    /// 启动接收服务；`ReceiveListeningChanged` 与 `ReceiveAsk` 等事件随之产生。
    fn start_receive(&self, options: ReceiveOptions) -> Result<(), BtError>;

    fn stop_receive(&self) -> Result<(), BtError>;

    /// 接收会话快照。
    fn receive_session(&self) -> ReceiveSession;

    /// 应答接收询问，接受时返回最终落盘路径。
    fn answer_receive_ask(&self, ask: AskId, accept: bool) -> Result<Option<PathBuf>, BtError>;

    fn records(&self, query: RecordQuery) -> Result<Vec<Record>, BtError>;

    fn stats(&self) -> Result<Stats, BtError>;

    /// 删除一条记录，返回是否命中。
    fn delete_record(&self, id: i64) -> Result<bool, BtError>;

    /// 清空全部记录，返回删除条数。
    fn clear_records(&self) -> Result<u64, BtError>;

    /// 按查询条件导出，返回落盘摘要。
    fn export(&self, query: RecordQuery, format: ExportFormat, path: PathBuf) -> Result<ExportSummary, BtError>;

    /// 为指定记录生成复制计划，由平台层执行剪贴板或文件管理器操作。
    fn copy_plan(&self, record_ids: &[i64]) -> Result<CopyPlan, BtError>;

    /// 运行自检，逐项返回结果。
    fn selftest(&self) -> Result<SelftestReport, BtError>;

    /// 导入旧版本数据。
    fn migrate_legacy(&self, options: MigrateOptions) -> Result<MigrateReport, BtError>;

    /// 订阅事件，每次调用返回独立通道。
    fn subscribe(&self) -> Receiver<Event>;

    /// 停止所有后台活动并释放资源。
    fn shutdown(&self);
}
