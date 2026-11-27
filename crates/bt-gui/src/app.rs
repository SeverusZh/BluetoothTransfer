use std::sync::Arc;

use bt_api::{Backend, MockBackend};
use bt_app::{build_backend_shared, AppPaths};

use crate::state::UiState;

/// 主窗口应用。
pub struct BtApp {
    state: UiState,
}

impl BtApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(eframe::egui::Visuals::dark());
        let backend = match AppPaths::resolve().and_then(build_backend_shared) {
            Ok(backend) => backend,
            Err(error) => {
                eprintln!("后端装配失败，改用内存后端：{error}");
                let fallback: Arc<dyn Backend> = Arc::new(MockBackend::default());
                fallback
            }
        };
        Self {
            state: UiState::new(backend),
        }
    }

    pub fn state(&self) -> &UiState {
        &self.state
    }
}

impl eframe::App for BtApp {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        use eframe::egui;

        self.state.poll();
        if self.state.scanning {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }

        ui.heading("蓝牙传输");
        ui.horizontal(|ui| {
            if ui.button("扫描设备").clicked() {
                self.state.start_scan();
            }
            if ui.button("刷新记录").clicked() {
                self.state.refresh_clicked();
            }
            if self.state.receive_listening {
                ui.label("接收中");
            }
        });
        ui.separator();

        ui.heading("设备");
        if self.state.devices.is_empty() {
            ui.label("尚未发现设备");
        }
        for device in &self.state.devices {
            ui.group(|ui| {
                ui.label(device.label());
                ui.small(device.id.to_string());
            });
        }
        ui.separator();

        ui.heading("发送队列");
        if self.state.jobs.is_empty() {
            ui.label("队列为空");
        }
        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
            for job in &self.state.jobs {
                ui.horizontal(|ui| {
                    ui.label(&job.item.display_name);
                    ui.small(job.state.as_str());
                    ui.small(format!("{}%", job.percent()));
                });
            }
        });
        ui.separator();

        ui.heading("记录");
        ui.label(format!(
            "共 {} 条，发送 {} 条，接收 {} 条",
            self.state.stats.total, self.state.stats.sent, self.state.stats.received
        ));
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            for record in self.state.records.iter().take(50) {
                ui.horizontal(|ui| {
                    ui.label(&record.display_name);
                    ui.small(record.channel.as_str());
                    ui.small(record.status.as_str());
                });
            }
        });
        ui.separator();
        ui.label(&self.state.status);
        if let Some(last) = self.state.logs.back() {
            ui.small(last);
        }
    }
}
