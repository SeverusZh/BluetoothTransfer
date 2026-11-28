//! 设备页：扫描、过滤、收藏、别名、配对、助手探测与发送入口。

use bt_api::Device;
use eframe::egui::{self, RichText};

use super::{empty_state, status_chip, StatusKind};
use crate::format;
use crate::state::UiState;
use crate::theme;

pub fn show(state: &mut UiState, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if state.scanning {
            if ui.button("取消扫描").clicked() {
                state.cancel_scan();
            }
            ui.spinner();
            ui.label("扫描中");
        } else if ui.button("扫描").clicked() {
            state.start_scan();
        }
        ui.add(
            egui::DragValue::new(&mut state.scan_duration_s)
                .range(1..=300)
                .suffix(" 秒"),
        );
        ui.checkbox(&mut state.scan_paired_only, "仅显示已配对");
        if ui.button("打开系统蓝牙设置").clicked() {
            state.open_bluetooth_settings();
        }
        let count = state
            .devices
            .iter()
            .filter(|device| !state.scan_paired_only || device.paired)
            .count();
        ui.label(RichText::new(format!("共 {count} 台")).color(theme::TEXT_SECONDARY));
    });
    ui.separator();

    let mut devices: Vec<Device> = state
        .devices
        .iter()
        .filter(|device| !state.scan_paired_only || device.paired)
        .cloned()
        .collect();
    devices.sort_by(|left, right| {
        right
            .favorite
            .cmp(&left.favorite)
            .then_with(|| right.paired.cmp(&left.paired))
            .then_with(|| left.label().cmp(&right.label()))
    });
    if devices.is_empty() {
        empty_state(ui, "尚未发现设备", "点击扫描；请确认对端蓝牙已开启并处于可被发现状态");
        return;
    }
    for device in &devices {
        device_row(state, ui, device);
    }

    show_alias_modal(state, ui);
}

fn device_row(state: &mut UiState, ui: &mut egui::Ui, device: &Device) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let star = if device.favorite { "★" } else { "☆" };
                if ui
                    .add(egui::Button::new(RichText::new(star).size(18.0).color(theme::WARNING)).frame(false))
                    .on_hover_text(if device.favorite {
                        "取消收藏"
                    } else {
                        "收藏，排在最前"
                    })
                    .clicked()
                {
                    state.toggle_favorite(device);
                }
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(device.label()).strong().size(14.0));
                        if device.paired {
                            status_chip(ui, "已配对", StatusKind::Success);
                        } else {
                            status_chip(ui, "未配对", StatusKind::Warning);
                        }
                        if device.connected {
                            status_chip(ui, "已连接", StatusKind::Success);
                        }
                        if let Some(rssi) = device.rssi {
                            status_chip(ui, &format!("信号 {rssi} dBm"), StatusKind::Neutral);
                        }
                        let assistant_kind = match device.assistant_state {
                            bt_core::AssistantState::Available => StatusKind::Success,
                            bt_core::AssistantState::Probing => StatusKind::Warning,
                            bt_core::AssistantState::Unavailable => StatusKind::Danger,
                            bt_core::AssistantState::Unknown => StatusKind::Neutral,
                        };
                        status_chip(ui, format::assistant(device.assistant_state), assistant_kind);
                    });
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(device.id.to_string())
                                .size(11.0)
                                .color(theme::TEXT_SECONDARY),
                        );
                        if let Some(last_seen) = &device.last_seen {
                            ui.label(
                                RichText::new(format!("最近 {}", format::timestamp(last_seen)))
                                    .size(11.0)
                                    .color(theme::TEXT_SECONDARY),
                            );
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("发送").clicked() {
                        state.set_send_device(device.id);
                    }
                    if ui.button("别名").clicked() {
                        state.begin_alias(device);
                    }
                    if ui.button("探测").clicked() {
                        state.detect(&device.id);
                    }
                    if !device.paired && ui.button("配对").clicked() {
                        state.pair(&device.id);
                    }
                });
            });
        });
}

fn show_alias_modal(state: &mut UiState, ui: &mut egui::Ui) {
    let Some((device_id, alias)) = state.alias_edit.clone() else {
        return;
    };
    let mut value = alias;
    let response = egui::Modal::new(egui::Id::unique("alias-editor")).show(ui.ctx(), |ui| {
        ui.set_width(360.0);
        ui.label(RichText::new(format!("设备别名 {device_id}")).strong());
        ui.add_space(6.0);
        ui.add(
            egui::TextEdit::singleline(&mut value)
                .hint_text("留空则清除别名")
                .desired_width(f32::INFINITY),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let save = ui.button("保存").clicked();
            let cancel = ui.button("取消").clicked();
            (save, cancel)
        })
        .inner
    });
    let (save, cancel) = response.inner;
    if save {
        state.alias_edit = Some((device_id, value));
        state.save_alias();
    } else if cancel {
        state.cancel_alias();
    }
}
