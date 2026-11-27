use std::path::PathBuf;
use std::sync::Mutex;

use bt_api::{
    AskId, Backend, ConfigPatch, CopyItem, CopyKind, CopyPlan, Device, Event, EventBus, ExportFormat, ExportSummary,
    JobId, JobSnapshot, MigrateOptions, MigrateReport, PinRequestId, ReceiveOptions, ReceiveSession, RecordQuery,
    ScanOptions, SelftestReport, SendItem, Stats,
};
use bt_core::{BtError, Channel, Config, DeviceId, Record};
use bt_store::{export_records, load_config, save_config, Store};
use crossbeam_channel::Receiver;

use crate::paths::AppPaths;

/// 装配骨架：配置、记录与事件总线已接通，蓝牙相关能力等待引擎接入。
pub struct StubBackend {
    paths: AppPaths,
    defaults: Config,
    config: Mutex<Config>,
    store: Store,
    bus: EventBus,
    devices: Mutex<Vec<Device>>,
    jobs: Mutex<Vec<JobSnapshot>>,
    session: Mutex<ReceiveSession>,
    auth_password: Mutex<Option<String>>,
}

impl StubBackend {
    pub fn open(paths: AppPaths) -> Result<Self, BtError> {
        let defaults = paths.default_config();
        let config = load_config(&paths.config_file, defaults.clone())?;
        let store = Store::open(&paths.database_file)?;
        let session = ReceiveSession::idle(config.receive_dir.clone(), config.receive_ask);
        Ok(Self {
            paths,
            defaults,
            config: Mutex::new(config),
            store,
            bus: EventBus::new(),
            devices: Mutex::new(Vec::new()),
            jobs: Mutex::new(Vec::new()),
            session: Mutex::new(session),
            auth_password: Mutex::new(None),
        })
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    fn pending(op: &str) -> BtError {
        BtError::not_implemented(format!("{op} 需要引擎接入，T3 完成"))
    }
}

impl Backend for StubBackend {
    fn config(&self) -> Config {
        self.config
            .lock()
            .map(|config| config.clone())
            .unwrap_or_else(|_| self.defaults.clone())
    }

    fn update_config(&self, patch: ConfigPatch) -> Result<Config, BtError> {
        let updated = {
            let mut config = self.config.lock().map_err(|_| BtError::internal("配置被投毒"))?;
            patch.apply(&mut config);
            config.normalize();
            config.validate()?;
            config.clone()
        };
        save_config(&self.paths.config_file, &updated, &self.defaults)?;
        self.bus.emit(Event::ConfigChanged {
            config: updated.clone(),
        });
        Ok(updated)
    }

    fn set_auth_password(&self, password: Option<&str>) -> Result<(), BtError> {
        let mut slot = self
            .auth_password
            .lock()
            .map_err(|_| BtError::internal("密码槽被投毒"))?;
        *slot = password.map(|value| value.to_string());
        Ok(())
    }

    fn devices(&self) -> Vec<Device> {
        self.devices.lock().map(|devices| devices.clone()).unwrap_or_default()
    }

    fn start_scan(&self, _options: ScanOptions) -> Result<(), BtError> {
        Err(Self::pending("设备扫描"))
    }

    fn cancel_scan(&self) -> Result<(), BtError> {
        Err(Self::pending("取消扫描"))
    }

    fn pair(&self, _device: &DeviceId) -> Result<(), BtError> {
        Err(Self::pending("配对"))
    }

    fn answer_prompt(&self, _request: PinRequestId, _accept: bool) -> Result<(), BtError> {
        Err(Self::pending("配对交互应答"))
    }

    fn set_alias(&self, _device: &DeviceId, _alias: Option<&str>) -> Result<(), BtError> {
        Err(Self::pending("设置别名"))
    }

    fn set_favorite(&self, _device: &DeviceId, _favorite: bool) -> Result<(), BtError> {
        Err(Self::pending("设置收藏"))
    }

    fn detect(&self, _device: &DeviceId) -> Result<(), BtError> {
        Err(Self::pending("助手通道探测"))
    }

    fn open_bluetooth_settings(&self) -> Result<(), BtError> {
        Err(BtError::not_implemented("打开系统蓝牙设置需要平台层接入"))
    }

    fn enqueue(
        &self,
        _device: &DeviceId,
        _items: Vec<SendItem>,
        _channel: Option<Channel>,
    ) -> Result<Vec<JobId>, BtError> {
        Err(Self::pending("发送入队"))
    }

    fn jobs(&self) -> Vec<JobSnapshot> {
        self.jobs.lock().map(|jobs| jobs.clone()).unwrap_or_default()
    }

    fn pause_job(&self, _job: JobId) -> Result<(), BtError> {
        Err(Self::pending("暂停任务"))
    }

    fn resume_job(&self, _job: JobId) -> Result<(), BtError> {
        Err(Self::pending("恢复任务"))
    }

    fn cancel_job(&self, _job: JobId) -> Result<(), BtError> {
        Err(Self::pending("取消任务"))
    }

    fn cancel_all_jobs(&self) -> Result<(), BtError> {
        Err(Self::pending("取消全部任务"))
    }

    fn retry_job(&self, _job: JobId) -> Result<(), BtError> {
        Err(Self::pending("重试任务"))
    }

    fn remove_job(&self, _job: JobId) -> Result<(), BtError> {
        Err(Self::pending("移除任务"))
    }

    fn start_receive(&self, _options: ReceiveOptions) -> Result<(), BtError> {
        Err(Self::pending("接收服务"))
    }

    fn stop_receive(&self) -> Result<(), BtError> {
        Err(Self::pending("停止接收服务"))
    }

    fn receive_session(&self) -> ReceiveSession {
        self.session
            .lock()
            .map(|session| session.clone())
            .unwrap_or_else(|_| ReceiveSession::idle(self.paths.receive_dir.clone(), true))
    }

    fn answer_receive_ask(&self, _ask: AskId, _accept: bool) -> Result<Option<PathBuf>, BtError> {
        Err(Self::pending("接收询问应答"))
    }

    fn records(&self, query: RecordQuery) -> Result<Vec<Record>, BtError> {
        self.store.query_records(&query)
    }

    fn stats(&self) -> Result<Stats, BtError> {
        self.store.stats()
    }

    fn delete_record(&self, id: i64) -> Result<bool, BtError> {
        let removed = self.store.delete_record(id)?;
        if removed {
            self.bus.emit(Event::RecordsChanged);
        }
        Ok(removed)
    }

    fn clear_records(&self) -> Result<u64, BtError> {
        let removed = self.store.clear_records()?;
        self.bus.emit(Event::RecordsChanged);
        Ok(removed)
    }

    fn export(&self, query: RecordQuery, format: ExportFormat, path: PathBuf) -> Result<ExportSummary, BtError> {
        export_records(&self.store, &path, &query, format)
    }

    fn copy_plan(&self, record_ids: &[i64]) -> Result<CopyPlan, BtError> {
        let records = self.store.query_records(&RecordQuery::default())?;
        let mut items = Vec::new();
        for id in record_ids {
            let record = records
                .iter()
                .find(|record| record.id == *id)
                .ok_or_else(|| BtError::not_found(format!("记录不存在 {id}")))?;
            match record.path.as_ref() {
                Some(path) if path.exists() => items.push(CopyItem::Path(path.clone())),
                Some(path) => items.push(CopyItem::Missing(path.clone())),
                None => items.push(CopyItem::Text(record.display_name.clone())),
            }
        }
        let has_path = items
            .iter()
            .any(|item| matches!(item, CopyItem::Path(_) | CopyItem::Missing(_)));
        let has_text = items.iter().any(|item| matches!(item, CopyItem::Text(_)));
        let kind = match (has_path, has_text) {
            (true, true) => CopyKind::Mixed,
            (true, false) => CopyKind::Files,
            _ => CopyKind::Text,
        };
        Ok(CopyPlan {
            items,
            kind: Some(kind),
        })
    }

    fn selftest(&self) -> Result<SelftestReport, BtError> {
        let mut report = SelftestReport::default();
        let config = self.config();
        report.push("config", config.validate().is_ok(), "12 项设置已归一化");
        let store_ok = self.store.schema_version().is_ok();
        report.push("store", store_ok, "记录库可读写");
        report.push(
            "paths",
            self.paths.create_dirs().is_ok(),
            self.paths.data_dir.display().to_string(),
        );
        report.push("engine", false, "引擎与蓝牙传输尚未接入，T3 完成");
        self.bus.emit(Event::info("装配骨架自检完成"));
        Ok(report)
    }

    fn migrate_legacy(&self, options: MigrateOptions) -> Result<MigrateReport, BtError> {
        bt_store::migrate_legacy(&options)
    }

    fn subscribe(&self) -> Receiver<Event> {
        self.bus.subscribe()
    }

    fn shutdown(&self) {}
}
