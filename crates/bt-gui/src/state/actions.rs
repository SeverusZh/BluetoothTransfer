//! UiState 的动作方法：所有后端调用集中在此，页面只负责布局与触发。

use std::path::PathBuf;

use bt_api::{ConfigPatch, ExportFormat, JobState, ReceiveOptions, SendItem};
use bt_core::{BtError, BtErrorKind, DeviceId, Record, SendSource};

use super::*;
use crate::copy::{apply_copy_plan, CopyOutcome};

impl UiState {
    pub fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    pub fn start_scan(&mut self) {
        if self.scanning {
            self.warn("扫描已在进行中");
            return;
        }
        let options = ScanOptions {
            duration_s: Some(self.scan_duration_s.clamp(1, 300)),
            refresh_paired: true,
        };
        match self.backend.start_scan(options) {
            Ok(()) => self.status = "已请求扫描".to_string(),
            Err(error) => self.fail(error),
        }
    }

    pub fn cancel_scan(&mut self) {
        match self.backend.cancel_scan() {
            Ok(()) => self.info("已请求停止扫描"),
            Err(error) => self.fail(error),
        }
    }

    pub fn pair(&mut self, device: &DeviceId) {
        match self.backend.pair(device) {
            Ok(()) => self.status = "已请求配对，请确认两端提示".to_string(),
            Err(error) => self.fail(error),
        }
    }

    pub fn answer_pairing(&mut self, accept: bool) {
        let Some(pairing) = self.pairing.clone() else {
            return;
        };
        let pin = self.pin_input.trim().to_string();
        let result = match pairing.kind {
            bt_api::PinPromptKind::Pin if accept => {
                if pin.is_empty() {
                    self.warn("请输入对端显示的 PIN");
                    return;
                }
                self.backend.answer_pin(pairing.request.id, &pin)
            }
            _ => self.backend.answer_prompt(pairing.request.id, accept),
        };
        match result {
            Ok(()) => {
                self.pairing = None;
                self.pin_input.clear();
                if accept {
                    self.info("已允许配对");
                } else {
                    self.info("已拒绝配对");
                }
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn toggle_favorite(&mut self, device: &bt_api::Device) {
        match self.backend.set_favorite(&device.id, !device.favorite) {
            Ok(()) => self.info(if device.favorite {
                format!("已取消收藏 {}", device.label())
            } else {
                format!("已收藏 {}", device.label())
            }),
            Err(error) => self.fail(error),
        }
    }

    pub fn begin_alias(&mut self, device: &bt_api::Device) {
        self.alias_edit = Some((device.id, device.alias.clone().unwrap_or_default()));
    }

    pub fn cancel_alias(&mut self) {
        self.alias_edit = None;
    }

    pub fn save_alias(&mut self) {
        let Some((device, alias)) = self.alias_edit.clone() else {
            return;
        };
        let trimmed = alias.trim().to_string();
        let value = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.as_str())
        };
        match self.backend.set_alias(&device, value) {
            Ok(()) => {
                self.alias_edit = None;
                self.info(if trimmed.is_empty() {
                    "别名已清除".to_string()
                } else {
                    format!("别名已保存：{trimmed}")
                });
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn detect(&mut self, device: &DeviceId) {
        match self.backend.detect(device) {
            Ok(()) => self.status = "正在探测助手通道".to_string(),
            Err(error) => self.fail(error),
        }
    }

    pub fn open_bluetooth_settings(&mut self) {
        match self.backend.open_bluetooth_settings() {
            Ok(()) => self.info("已请求打开系统蓝牙设置"),
            Err(error) => self.fail(error),
        }
    }

    pub fn set_send_device(&mut self, device: DeviceId) {
        self.send.selected_device = Some(device);
        self.page = Page::Send;
        self.status = "已选择发送目标".to_string();
    }

    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        let mut files = Vec::new();
        let mut folders = Vec::new();
        for path in paths {
            if path.is_dir() {
                folders.push(path);
            } else if path.is_file() {
                files.push(path);
            } else {
                self.send
                    .warnings
                    .push(format!("路径不存在，已忽略：{}", path.display()));
            }
        }
        if !files.is_empty() {
            self.add_files(files);
        }
        for folder in folders {
            self.add_folder(folder);
        }
    }

    pub fn add_files(&mut self, paths: Vec<PathBuf>) {
        let mut added = 0usize;
        for path in paths {
            let source = SendSource::File(path.clone());
            if self.send.items.iter().any(|item| item.source == source) {
                continue;
            }
            let mut item = SendItem::from_source(source);
            item.total_bytes = std::fs::metadata(&path).ok().map(|metadata| metadata.len());
            self.send.items.push(item);
            added += 1;
        }
        if added > 0 {
            self.page = Page::Send;
            self.info(format!("已加入 {added} 个文件"));
        }
    }

    pub fn add_folder(&mut self, path: PathBuf) {
        let source = SendSource::Folder(path.clone());
        if self.send.items.iter().any(|item| item.source == source) {
            return;
        }
        self.send.items.push(SendItem::from_source(source));
        self.page = Page::Send;
        self.info(format!("已加入文件夹：{}", path.display()));
    }

    pub fn remove_send_item(&mut self, index: usize) {
        if index < self.send.items.len() {
            self.send.items.remove(index);
        }
    }

    pub fn clear_send_items(&mut self) {
        self.send.items.clear();
        self.send.warnings.clear();
    }

    pub fn choose_send_files(&mut self) {
        self.spawn_task(|| {
            let picked = bt_platform::pick_files("选择要发送的文件");
            Box::new(move |state: &mut UiState| {
                if !picked.paths.is_empty() {
                    state.add_files(picked.paths);
                }
            })
        });
    }

    pub fn choose_settings_receive_dir(&mut self) {
        let start = self.settings.draft.receive_dir.clone();
        self.spawn_task(move || {
            let picked = bt_platform::pick_folder("选择接收文件保存目录", Some(&start));
            Box::new(move |state: &mut UiState| {
                if let Some(path) = picked {
                    state.settings.receive_dir_text = path.display().to_string();
                    state.settings.draft.receive_dir = path;
                }
            })
        });
    }

    pub fn choose_send_folder(&mut self) {
        self.spawn_task(|| {
            let picked = bt_platform::pick_folder("选择要发送的文件夹", None);
            Box::new(move |state: &mut UiState| {
                if let Some(path) = picked {
                    state.add_folder(path);
                }
            })
        });
    }

    pub fn read_clipboard_text(&mut self) {
        self.spawn_task(|| {
            let clipboard = bt_platform::platform_clipboard();
            let result = clipboard.get_text();
            Box::new(move |state: &mut UiState| match result {
                Ok(Some(text)) if !text.trim().is_empty() => {
                    if !state.send.text.is_empty() {
                        state.send.text.push('\n');
                    }
                    state.send.text.push_str(&text);
                    state.info("已读取剪贴板文本");
                }
                Ok(_) => state.warn("剪贴板里没有可发送的文本"),
                Err(error) => state.fail(error),
            })
        });
    }

    pub fn read_clipboard_image(&mut self) {
        self.spawn_task(|| {
            let clipboard = bt_platform::platform_clipboard();
            let result = clipboard.get_image().and_then(|image| match image {
                Some(image) => {
                    let dir = temp_dir();
                    std::fs::create_dir_all(&dir).map_err(BtError::from)?;
                    let path = dir.join(format!("剪贴板图片-{}.png", now_stamp()));
                    bt_platform::clipboard::save_png(&path, &image)?;
                    Ok(Some(path))
                }
                None => Ok(None),
            });
            Box::new(move |state: &mut UiState| match result {
                Ok(Some(path)) => {
                    state.temp_files.push(path.clone());
                    state.add_files(vec![path]);
                    state.info("已把剪贴板图片加入待发送");
                }
                Ok(None) => state.warn("剪贴板里没有图片"),
                Err(error) => state.fail(error),
            })
        });
    }

    pub fn send_text(&mut self) {
        let Some(device) = self.send.selected_device else {
            self.warn("请先在设备页选择发送目标");
            return;
        };
        let text = self.send.text.clone();
        if text.trim().is_empty() {
            self.warn("文本为空，未加入队列");
            return;
        }
        let mut item = SendItem::from_source(SendSource::Text(text.clone()));
        item.total_bytes = Some(text.len() as u64);
        let channel = self.send.channel.channel();
        match self.backend.enqueue(&device, vec![item], channel) {
            Ok(ids) => {
                self.send.text.clear();
                self.succeed(format!("文本已加入队列（{} 项）", ids.len()));
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn send_pending(&mut self) {
        if self.send.busy {
            return;
        }
        let Some(device) = self.send.selected_device else {
            self.warn("请先在设备页选择发送目标");
            return;
        };
        if self.send.items.is_empty() {
            self.warn("待发送列表为空");
            return;
        }
        let items = self.send.items.clone();
        if let Some(paths) = self.app_paths.as_ref() {
            let pending = items.iter().find_map(|item| match &item.source {
                SendSource::File(path) if bt_app::is_pending_source(paths, path) => Some(path.clone()),
                _ => None,
            });
            if let Some(path) = pending {
                self.fail(BtError::invalid(format!(
                    "源文件位于待续传目录 pending/：{}；请把源文件移出该目录后再发送，该目录只存放本应用生成的待续传载荷",
                    path.display()
                )));
                return;
            }
        }
        let channel = self.send.channel.channel();
        let zip_single = self.send.zip_single;
        if zip_single && items.len() > 1 {
            self.warn("单文件 zip 只对单个文件生效，本次按原样发送");
        }
        if zip_single && items.len() == 1 {
            if let SendSource::File(source_path) = &items[0].source {
                let Some(paths) = self.app_paths.clone() else {
                    self.warn("无法确定应用数据目录，单文件 zip 不可用");
                    return;
                };
                let source = source_path.clone();
                let backend = self.backend.clone();
                self.send.busy = true;
                self.spawn_task(move || {
                    let mut archive = None;
                    let result = (|| -> Result<(Vec<bt_api::JobId>, PathBuf), BtError> {
                        let packed = bt_app::pack_zip_file_in(&paths, &source)?;
                        archive = Some(packed.path.clone());
                        let size = std::fs::metadata(&packed.path)
                            .map_err(|error| BtError::storage(format!("读取 zip 失败：{error}")))?
                            .len();
                        let produced = packed.owned;
                        let mut item = SendItem::from_source(SendSource::File(packed.path.clone()));
                        item.display_name = packed.display_name;
                        item.total_bytes = Some(size);
                        item.produced = produced;
                        let ids = backend.enqueue(&device, vec![item], channel)?;
                        Ok((ids, packed.path))
                    })();
                    if result.is_err() {
                        if let Some(path) = archive {
                            bt_app::settle_owned_payload(&paths, &path);
                        }
                    }
                    Box::new(move |state: &mut UiState| {
                        state.send.busy = false;
                        match result {
                            Ok((ids, archive)) => {
                                for id in &ids {
                                    state.zip_payloads.insert(*id, archive.clone());
                                }
                                for id in &ids {
                                    let terminal =
                                        state.jobs.iter().any(|job| job.id == *id && job.state.is_terminal());
                                    if terminal {
                                        state.settle_zip_payload(*id);
                                    }
                                }
                                state.send.items.clear();
                                state.succeed("已打包为 zip 并加入队列");
                            }
                            Err(error) => state.fail(error),
                        }
                    })
                });
                return;
            }
            self.warn("文件夹发送时会自动打包，单文件 zip 仅对单个文件生效");
        }
        match self.backend.enqueue(&device, items, channel) {
            Ok(ids) => {
                self.send.items.clear();
                self.send.warnings.clear();
                self.succeed(format!("已加入队列（{} 项）", ids.len()));
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn pause_job(&mut self, job: JobId) {
        match self.backend.pause_job(job) {
            Ok(()) => self.info("任务已暂停"),
            Err(error) => self.fail(error),
        }
    }

    pub fn resume_job(&mut self, job: JobId) {
        match self.backend.resume_job(job) {
            Ok(()) => self.info("任务已继续"),
            Err(error) => self.fail(error),
        }
    }

    pub fn cancel_job(&mut self, job: JobId) {
        match self.backend.cancel_job(job) {
            Ok(()) => self.info("已请求取消任务"),
            Err(error) => self.fail(error),
        }
    }

    pub fn cancel_current_job(&mut self) {
        let current = self
            .jobs
            .iter()
            .find(|job| job.state.is_running() || job.state == JobState::Queued)
            .map(|job| job.id);
        match current {
            Some(id) => self.cancel_job(id),
            None => self.warn("没有正在进行的任务"),
        }
    }

    pub fn retry_job(&mut self, job: JobId) {
        match self.backend.retry_job(job) {
            Ok(()) => self.info("任务已重新加入队列"),
            Err(error) => self.fail(error),
        }
    }

    pub fn remove_job(&mut self, job: JobId) {
        match self.backend.remove_job(job) {
            Ok(()) => {
                self.jobs.retain(|entry| entry.id != job);
                self.settle_zip_payload(job);
                self.info("任务已从队列移除");
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn cancel_all_jobs(&mut self) {
        match self.backend.cancel_all_jobs() {
            Ok(()) => self.info("已请求取消全部任务"),
            Err(error) => self.fail(error),
        }
    }

    pub fn start_receive(&mut self) {
        if self.receive.standalone_running {
            self.fail(BtError::busy(
                "检测到独立的 btrecv 接收进程正在运行，请先关闭它，再启动界面接收",
            ));
            return;
        }
        let options = ReceiveOptions {
            dir: Some(self.config.receive_dir.clone()),
            ask: Some(self.config.receive_ask),
        };
        match self.backend.start_receive(options) {
            Ok(()) => {
                self.receive.listening = true;
                self.succeed(format!("接收服务已启动，保存到 {}", self.config.receive_dir.display()));
            }
            Err(error) if error.kind() == BtErrorKind::Busy => {
                self.fail(BtError::busy(format!(
                    "接收服务启动失败：{}；请确认没有其它接收进程占用蓝牙服务",
                    error.message()
                )));
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn stop_receive(&mut self) {
        match self.backend.stop_receive() {
            Ok(()) => {
                self.receive.listening = false;
                self.info("接收服务已停止");
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn choose_receive_dir(&mut self) {
        let start = self.config.receive_dir.clone();
        self.spawn_task(move || {
            let picked = bt_platform::pick_folder("选择接收文件保存目录", Some(&start));
            Box::new(move |state: &mut UiState| {
                if let Some(path) = picked {
                    state.set_receive_dir(path);
                }
            })
        });
    }

    pub fn set_receive_dir(&mut self, path: PathBuf) {
        let was_pristine = self.settings.draft == self.config;
        let was_listening = self.backend.receive_session().listening;
        if was_listening {
            if let Err(error) = self.backend.stop_receive() {
                self.fail(error);
                return;
            }
        }
        let patch = ConfigPatch {
            receive_dir: Some(path.clone()),
            ..Default::default()
        };
        match self.backend.update_config(patch) {
            Ok(config) => {
                self.config = config.clone();
                if was_pristine {
                    self.settings.draft = config.clone();
                    self.settings.receive_dir_text = config.receive_dir.display().to_string();
                }
                if was_listening {
                    self.start_receive();
                } else {
                    self.succeed(format!("接收目录已保存：{}", path.display()));
                }
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn set_receive_ask(&mut self, ask: bool) {
        let was_pristine = self.settings.draft == self.config;
        let was_listening = self.backend.receive_session().listening;
        if was_listening {
            if let Err(error) = self.backend.stop_receive() {
                self.fail(error);
                return;
            }
        }
        let patch = ConfigPatch {
            receive_ask: Some(ask),
            ..Default::default()
        };
        match self.backend.update_config(patch) {
            Ok(config) => {
                self.config = config.clone();
                if was_pristine {
                    self.settings.draft = config.clone();
                }
                if was_listening {
                    self.start_receive();
                } else {
                    self.succeed(if ask {
                        "已开启每次接收前询问".to_string()
                    } else {
                        "已关闭接收前询问".to_string()
                    });
                }
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn answer_receive(&mut self, accept: bool) {
        let Some(ask) = self.receive.pending_ask.take() else {
            return;
        };
        match self.backend.answer_receive_ask(ask.id, accept) {
            Ok(Some(path)) => self.succeed(format!("已接收：{}", path.display())),
            Ok(None) => self.info("已拒绝本次接收"),
            Err(error) => self.fail(error),
        }
    }

    pub fn refresh_receive_probe(&mut self) {
        self.receive.probe_due = Some(Instant::now());
    }

    pub fn mark_filter_dirty(&mut self) {
        self.filter.dirty = true;
    }

    pub fn delete_record(&mut self, id: i64) {
        match self.backend.delete_record(id) {
            Ok(true) => {
                self.records.retain(|record| record.id != id);
                self.refresh_stats();
                self.info("记录已删除");
            }
            Ok(false) => self.warn("记录不存在，可能已被其它操作删除"),
            Err(error) => self.fail(error),
        }
    }

    pub fn confirm_clear_records(&mut self) {
        match self.backend.clear_records() {
            Ok(count) => {
                self.filter.clear_confirm = false;
                self.reload_records();
                self.succeed(format!("已清空 {count} 条记录（含失败记录）"));
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn copy_record(&mut self, record: Record) {
        if record.status == RecordStatus::Running {
            self.warn("记录仍在进行中，完成后才能复制");
            return;
        }
        let backend = self.backend.clone();
        self.spawn_task(move || {
            let outcome = match backend.copy_plan(&[record.id]) {
                Ok(plan) => apply_copy_plan(&plan, &record),
                Err(error) => CopyOutcome::Failure(error.message().to_string()),
            };
            Box::new(move |state: &mut UiState| match outcome {
                CopyOutcome::Success(message) => state.succeed(message),
                CopyOutcome::Failure(message) => state.warn(message),
            })
        });
    }

    pub fn choose_export(&mut self, format: ExportFormat) {
        let backend = self.backend.clone();
        let query = self.record_query(true);
        let extension = format.extension().to_string();
        self.spawn_task(move || {
            let default_name = format!("bluetooth-records-{}.{}", now_date(), extension);
            let picked = bt_platform::save_file("导出传输记录", &default_name, &extension);
            let Some(path) = picked else {
                return Box::new(|_: &mut UiState| {});
            };
            let result = backend.export(query, format, path);
            Box::new(move |state: &mut UiState| match result {
                Ok(summary) => state.succeed(export_message(&summary)),
                Err(error) => state.fail(error),
            })
        });
    }

    pub fn export_diagnostics(&mut self) {
        let default_name = format!("bluetooth-diagnostics-{}.txt", now_date());
        let extra = vec![
            (
                "数据目录",
                self.data_dir
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
            ),
            ("配置", format!("{:?}", self.config)),
            ("设备数", self.devices.len().to_string()),
            ("任务数", self.jobs.len().to_string()),
            ("记录数", self.stats.total.to_string()),
        ];
        let text = self.logs.diagnostics(&extra);
        self.spawn_task(move || {
            let picked = bt_platform::save_file("导出诊断信息", &default_name, "txt");
            let Some(path) = picked else {
                return Box::new(|_: &mut UiState| {});
            };
            let result = std::fs::write(&path, text).map_err(BtError::from);
            Box::new(move |state: &mut UiState| match result {
                Ok(()) => state.succeed(format!("诊断信息已导出：{}", path.display())),
                Err(error) => state.fail(error),
            })
        });
    }

    pub fn open_log_dir(&mut self) {
        let Some(dir) = self.data_dir.as_ref().map(|path| path.join("logs")) else {
            self.warn("日志目录尚未确定");
            return;
        };
        if let Err(error) = bt_platform::open::open_path(&dir) {
            self.fail(error);
        }
    }

    pub fn open_receive_dir(&mut self) {
        if let Err(error) = bt_platform::open::open_path(&self.config.receive_dir) {
            self.fail(error);
        }
    }

    pub fn open_data_dir(&mut self) {
        let Some(dir) = self.data_dir.clone() else {
            self.warn("数据目录尚未确定");
            return;
        };
        if let Err(error) = bt_platform::open::open_path(&dir) {
            self.fail(error);
        }
    }

    pub fn save_settings(&mut self) {
        self.settings.draft.receive_dir = PathBuf::from(self.settings.receive_dir_text.trim());
        let draft = self.settings.draft.clone();
        if let Err(error) = draft.validate() {
            self.fail(error);
            return;
        }
        let patch = ConfigPatch {
            opp_chunk_size: Some(draft.opp_chunk_size),
            opp_connect_timeout_s: Some(draft.opp_connect_timeout_s),
            opp_send_idle_timeout_s: Some(draft.opp_send_idle_timeout_s),
            opp_retry_count: Some(draft.opp_retry_count),
            opp_retry_delay_s: Some(draft.opp_retry_delay_s),
            opp_protection: Some(draft.opp_protection),
            opp_name_bom: Some(draft.opp_name_bom),
            assistant_chunk_size: Some(draft.assistant_chunk_size),
            push_text_file_name: Some(draft.push_text_file_name.clone()),
            receive_dir: Some(draft.receive_dir.clone()),
            receive_ask: Some(draft.receive_ask),
            mode: Some(draft.mode),
        };
        let old_dir = self.config.receive_dir.clone();
        let old_ask = self.config.receive_ask;
        match self.backend.update_config(patch) {
            Ok(config) => {
                self.config = config.clone();
                self.settings.draft = config.clone();
                self.settings.receive_dir_text = config.receive_dir.display().to_string();
                let receive_changed = config.receive_dir != old_dir || config.receive_ask != old_ask;
                if receive_changed && self.receive.listening {
                    if let Err(error) = self.backend.stop_receive() {
                        self.fail(error);
                        return;
                    }
                    self.start_receive();
                }
                self.succeed("设置已保存");
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn reset_settings(&mut self) {
        self.settings.draft = self.default_config.clone();
        self.settings.receive_dir_text = self.default_config.receive_dir.display().to_string();
        self.info("已恢复默认值，点击保存后生效");
    }

    pub fn set_auth_password(&mut self) {
        let password = self.settings.auth_input.clone();
        if password.is_empty() {
            self.warn("认证密码为空，未设置");
            return;
        }
        match self.backend.set_auth_password(Some(&password)) {
            Ok(()) => {
                self.settings.auth_input.clear();
                self.settings.auth_set = true;
                self.succeed("认证密码已设置，输入框与配置文件都不会保留明文");
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn clear_auth_password(&mut self) {
        match self.backend.set_auth_password(None) {
            Ok(()) => {
                self.settings.auth_input.clear();
                self.settings.auth_set = false;
                self.info("认证密码已清除");
            }
            Err(error) => self.fail(error),
        }
    }

    pub fn run_selftest(&mut self) {
        let backend = self.backend.clone();
        self.spawn_task(move || {
            let result = backend.selftest();
            Box::new(move |state: &mut UiState| match result {
                Ok(report) => {
                    let passed = report.passed();
                    let failed: Vec<String> = report.failed_names().iter().map(|name| name.to_string()).collect();
                    state.settings.selftest_report = Some(report);
                    if passed {
                        state.succeed("后端自检全部通过");
                    } else {
                        state.warn(format!("后端自检未通过：{}", failed.join("、")));
                    }
                }
                Err(error) => state.fail(error),
            })
        });
    }

    pub fn choose_migration_root(&mut self) {
        self.spawn_task(|| {
            let picked = bt_platform::pick_folder("选择旧版本数据目录", None);
            Box::new(move |state: &mut UiState| {
                if let Some(path) = picked {
                    state.settings.migration_root = path.display().to_string();
                }
            })
        });
    }

    pub fn run_migration(&mut self) {
        let root = PathBuf::from(self.settings.migration_root.trim());
        if root.as_os_str().is_empty() {
            self.warn("请先选择旧版本数据目录");
            return;
        }
        if !root.is_dir() {
            self.warn(format!("旧数据目录不存在：{}", root.display()));
            return;
        }
        let backend = self.backend.clone();
        let dry_run = self.settings.migration_dry_run;
        self.settings.busy = true;
        self.spawn_task(move || {
            let options = MigrateOptions {
                legacy_root: root,
                target_dir: None,
                dry_run,
            };
            let result = backend.migrate_legacy(options);
            Box::new(move |state: &mut UiState| {
                state.settings.busy = false;
                match result {
                    Ok(report) => {
                        let summary = format!(
                            "迁移{}：记录 {} 条，配置 {}，文件 {} 个，跳过 {}",
                            if report.dry_run { "预演" } else { "完成" },
                            report.imported_records,
                            if report.imported_config {
                                "已导入"
                            } else {
                                "未导入"
                            },
                            report.copied_files,
                            report.skipped
                        );
                        state.settings.migration_report = Some(report);
                        state.reload_records();
                        state.succeed(summary);
                    }
                    Err(error) => state.fail(error),
                }
            })
        });
    }

    pub fn install_shell(&mut self) {
        self.spawn_task(|| {
            let shell = bt_platform::platform_shell();
            let result = shell.install();
            Box::new(move |state: &mut UiState| {
                state.settings.shell_installed = bt_platform::platform_shell().is_installed();
                match result {
                    Ok(report) => {
                        if report.warnings.is_empty() {
                            state.succeed(report.summary);
                        } else {
                            state.notice = Some(Notice {
                                kind: NoticeKind::Info,
                                text: format!("{}；{}", report.summary, report.warnings.join("；")),
                            });
                            state.status = report.summary;
                        }
                    }
                    Err(error) => state.fail(error),
                }
            })
        });
    }

    pub fn uninstall_shell(&mut self) {
        self.spawn_task(|| {
            let shell = bt_platform::platform_shell();
            let result = shell.uninstall();
            Box::new(move |state: &mut UiState| {
                state.settings.shell_installed = bt_platform::platform_shell().is_installed();
                match result {
                    Ok(report) => state.succeed(report.summary),
                    Err(error) => state.fail(error),
                }
            })
        });
    }

    pub fn toggle_autostart(&mut self) {
        let autostart = bt_platform::platform_autostart();
        if autostart.is_enabled() {
            match autostart.disable() {
                Ok(()) => {
                    self.settings.autostart_enabled = false;
                    self.info("已关闭开机自启");
                }
                Err(error) => self.fail(error),
            }
        } else {
            let exe = match bt_platform::shell::current_exe() {
                Ok(exe) => exe,
                Err(error) => {
                    self.fail(error);
                    return;
                }
            };
            let entry = bt_platform::AutostartEntry::new(exe).with_args(vec!["--minimized".to_string()]);
            match autostart.enable(&entry) {
                Ok(()) => {
                    self.settings.autostart_enabled = true;
                    self.info("已开启开机自启（最小化到托盘）");
                }
                Err(error) => self.fail(error),
            }
        }
    }

    pub fn cleanup_temp_files(&mut self) {
        for path in self.temp_files.drain(..) {
            if is_own_temp_file(&path) {
                let _ = std::fs::remove_file(path);
            }
        }
        let dir = temp_dir();
        if dir.starts_with(std::env::temp_dir()) {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("bluetooth-transfer-{}", std::process::id()))
}

pub(super) fn is_own_temp_file(path: &std::path::Path) -> bool {
    let dir = temp_dir();
    path.starts_with(&dir) && path.is_file()
}

fn now_stamp() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

fn now_date() -> String {
    let value = bt_core::now_rfc3339();
    value.chars().take(10).collect()
}

fn export_message(summary: &bt_api::ExportSummary) -> String {
    format!(
        "已导出 {} 条记录到 {}（{} 字节）",
        summary.rows,
        summary.path.display(),
        summary.bytes
    )
}
