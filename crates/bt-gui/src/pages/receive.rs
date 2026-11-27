//! 接收页：启停、目录、询问开关、进度、本会话完成列表与冲突提示。

use eframe::egui::{self, RichText};

use super::{empty_state, section_title, status_chip, StatusKind};
use crate::format;
use crate::state::UiState;
use crate::theme;

pub fn show(state: &mut UiState, ui: &mut egui::Ui) {
    section_title(ui, "接收服务");
    ui.horizontal_wrapped(|ui| {
        if state.receive.listening {
            if ui.button("停止接收").clicked() {
                state.stop_receive();
            }
            status_chip(ui, "接收中", StatusKind::Success);
        } else if ui.button("开始接收").clicked() {
            state.start_receive();
        } else {
            status_chip(ui, "未启动", StatusKind::Neutral);
        }
        ui.add_space(8.0);
        ui.label("保存目录");
        ui.label(
            RichText::new(state.config.receive_dir.display().to_string())
                .size(12.0)
                .color(theme::TEXT_SECONDARY),
        );
        if ui.button("选择目录").clicked() {
            state.choose_receive_dir();
        }
        if ui.button("打开目录").clicked() {
            state.open_receive_dir();
        }
        let mut ask = state.config.receive_ask;
        if ui.checkbox(&mut ask, "每次接收前询问").changed() {
            state.set_receive_ask(ask);
        }
    });

    if state.receive.standalone_running {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(253, 231, 233))
            .stroke(egui::Stroke::new(1.0, theme::DANGER))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("检测到独立接收进程 btrecv 正在运行，请先关闭它再启动界面接收")
                            .color(theme::DANGER),
                    );
                    if ui.button("重新检测").clicked() {
                        state.refresh_receive_probe();
                    }
                });
            });
    }

    ui.add_space(6.0);
    if let Some(ask) = state.receive.pending_ask.clone() {
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(255, 244, 206))
            .stroke(egui::Stroke::new(1.0, theme::WARNING))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("接收请求：{}（{}）", ask.name, format::bytes(ask.size))).strong());
                    if ui.button("接收").clicked() {
                        state.answer_receive(true);
                    }
                    if ui.button("拒绝").clicked() {
                        state.answer_receive(false);
                    }
                });
            });
    }

    if let Some(name) = state.receive.active_name.clone() {
        let total = state.receive.active_total.unwrap_or(0);
        let progress = if total > 0 {
            (state.receive.active_bytes.min(total) as f32 / total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ui.add_space(4.0);
        ui.label(RichText::new(format!("正在接收 {name}")).strong());
        let total_text = if total > 0 {
            format::bytes(total)
        } else {
            "总大小待确认".to_string()
        };
        ui.add(
            egui::ProgressBar::new(progress)
                .desired_height(8.0)
                .show_percentage()
                .text(format!(
                    "{} / {}",
                    format::bytes(state.receive.active_bytes),
                    total_text
                )),
        );
    }

    ui.add_space(8.0);
    section_title(ui, "本会话接收");
    if state.receive.completed.is_empty() {
        empty_state(ui, "本会话暂无接收文件", "点击开始接收后，收到的文件会显示在这里");
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for entry in &state.receive.completed {
            egui::Frame::new()
                .fill(theme::SURFACE)
                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        status_chip(
                            ui,
                            if entry.ok { "完成" } else { "失败" },
                            if entry.ok {
                                StatusKind::Success
                            } else {
                                StatusKind::Danger
                            },
                        );
                        ui.label(RichText::new(&entry.name).strong());
                        if entry.size > 0 {
                            ui.label(
                                RichText::new(format::bytes(entry.size))
                                    .size(11.0)
                                    .color(theme::TEXT_SECONDARY),
                            );
                        }
                        ui.label(
                            RichText::new(format::timestamp(&entry.at))
                                .size(11.0)
                                .color(theme::TEXT_SECONDARY),
                        );
                    });
                    if let Some(path) = &entry.path {
                        ui.label(
                            RichText::new(path.display().to_string())
                                .size(11.0)
                                .color(theme::TEXT_SECONDARY),
                        );
                    }
                    if let Some(error) = &entry.error {
                        ui.label(RichText::new(error).color(theme::DANGER).size(12.0));
                    }
                });
        }
    });
}
