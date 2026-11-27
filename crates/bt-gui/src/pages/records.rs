//! 记录页：筛选、统计、复制、删除、清空与全量导出。

use bt_api::ExportFormat;
use bt_core::{Channel, Direction, Record, RecordKind, RecordStatus};
use eframe::egui::{self, RichText};

use super::{empty_state, section_title, status_chip, StatusKind};
use crate::format;
use crate::state::UiState;
use crate::theme;

pub fn show(state: &mut UiState, ui: &mut egui::Ui) {
    section_title(ui, "筛选");
    ui.horizontal_wrapped(|ui| {
        direction_combo(state, ui);
        kind_combo(state, ui);
        status_combo(state, ui);
        channel_combo(state, ui);
        device_combo(state, ui);
        let search = ui.add(
            egui::TextEdit::singleline(&mut state.filter.search)
                .hint_text("搜索名称或设备")
                .desired_width(180.0),
        );
        if search.changed() {
            state.mark_filter_dirty();
        }
        if ui.button("刷新").clicked() {
            state.reload_records();
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!(
                "共 {} 条 · 发送 {} · 接收 {} · 失败 {} · 发送量 {} · 接收量 {}",
                state.stats.total,
                state.stats.sent,
                state.stats.received,
                state.stats.failed,
                format::bytes(state.stats.bytes_sent),
                format::bytes(state.stats.bytes_received)
            ))
            .color(theme::TEXT_SECONDARY),
        );
        ui.separator();
        if ui.button("导出 CSV").clicked() {
            state.choose_export(ExportFormat::Csv);
        }
        if ui.button("导出 JSONL").clicked() {
            state.choose_export(ExportFormat::Jsonl);
        }
        if ui
            .add_enabled(state.stats.total > 0, egui::Button::new("清空"))
            .clicked()
        {
            state.filter.clear_confirm = true;
        }
    });

    egui::CollapsingHeader::new("按日统计")
        .id_salt("records-day-stats")
        .default_open(false)
        .show(ui, |ui| {
            if state.stats.days.is_empty() {
                ui.label("暂无按日统计");
                return;
            }
            let mut days = state.stats.days.clone();
            days.sort_by(|left, right| right.date.cmp(&left.date));
            egui::Grid::new("records-day-grid")
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new("日期").strong());
                    ui.label(RichText::new("发送").strong());
                    ui.label(RichText::new("接收").strong());
                    ui.label(RichText::new("字节").strong());
                    ui.end_row();
                    for day in days.iter().take(14) {
                        ui.label(&day.date);
                        ui.label(day.sent.to_string());
                        ui.label(day.received.to_string());
                        ui.label(format::bytes(day.bytes));
                        ui.end_row();
                    }
                });
        });

    ui.separator();
    if state.records_truncated {
        ui.label(
            RichText::new("仅显示最近 1000 条；导出使用全部匹配记录")
                .size(11.0)
                .color(theme::TEXT_SECONDARY),
        );
    }
    if state.records.is_empty() {
        empty_state(ui, "没有符合条件的记录", "调整筛选条件，或先完成一次传输");
        return;
    }
    let records: Vec<Record> = state.records.clone();
    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("records-grid")
            .num_columns(10)
            .striped(true)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for header in [
                    "时间", "方向", "类型", "对端", "名称", "大小", "状态", "通道", "备注", "操作",
                ] {
                    ui.label(RichText::new(header).strong().size(12.0));
                }
                ui.end_row();
                for record in &records {
                    record_row(state, ui, record);
                    ui.end_row();
                }
            });
    });

    if state.filter.clear_confirm {
        clear_modal(state, ui);
    }
}

fn record_row(state: &mut UiState, ui: &mut egui::Ui, record: &Record) {
    ui.label(RichText::new(format::timestamp(&record.started_at)).size(12.0));
    ui.label(format::direction(record.direction));
    ui.label(format::kind(record.kind));
    ui.label(if record.device_name.is_empty() {
        "—".to_string()
    } else {
        record.device_name.clone()
    });
    ui.label(&record.display_name);
    ui.label(format::bytes(record.byte_size));
    let status_kind = match record.status {
        RecordStatus::Completed => StatusKind::Success,
        RecordStatus::Failed => StatusKind::Danger,
        RecordStatus::Running => StatusKind::Warning,
        _ => StatusKind::Neutral,
    };
    status_chip(ui, format::status(record.status), status_kind);
    ui.label(format::channel(record.channel));
    if let Some(error) = &record.error {
        ui.label(RichText::new(error).color(theme::DANGER).size(11.0));
    } else {
        ui.label(RichText::new(""));
    }
    ui.horizontal(|ui| {
        if ui.button("复制").clicked() {
            state.copy_record(record.clone());
        }
        if ui.button("删除").clicked() {
            state.delete_record(record.id);
        }
    });
}

fn direction_combo(state: &mut UiState, ui: &mut egui::Ui) {
    let before = state.filter.direction;
    let text = state.filter.direction.map(format::direction).unwrap_or("全部方向");
    egui::ComboBox::from_id_salt("filter-direction")
        .selected_text(text)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.filter.direction, None, "全部方向");
            ui.selectable_value(&mut state.filter.direction, Some(Direction::Send), "发送");
            ui.selectable_value(&mut state.filter.direction, Some(Direction::Receive), "接收");
        });
    if state.filter.direction != before {
        state.mark_filter_dirty();
    }
}

fn kind_combo(state: &mut UiState, ui: &mut egui::Ui) {
    let before = state.filter.kind;
    let text = state.filter.kind.map(format::kind).unwrap_or("全部类型");
    egui::ComboBox::from_id_salt("filter-kind")
        .selected_text(text)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.filter.kind, None, "全部类型");
            for kind in [
                RecordKind::File,
                RecordKind::MultiFile,
                RecordKind::Folder,
                RecordKind::Text,
                RecordKind::Other,
            ] {
                ui.selectable_value(&mut state.filter.kind, Some(kind), format::kind(kind));
            }
        });
    if state.filter.kind != before {
        state.mark_filter_dirty();
    }
}

fn status_combo(state: &mut UiState, ui: &mut egui::Ui) {
    let before = state.filter.status;
    let text = state.filter.status.map(format::status).unwrap_or("全部状态");
    egui::ComboBox::from_id_salt("filter-status")
        .selected_text(text)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.filter.status, None, "全部状态");
            for status in [
                RecordStatus::Running,
                RecordStatus::Completed,
                RecordStatus::Failed,
                RecordStatus::Canceled,
                RecordStatus::Interrupted,
            ] {
                ui.selectable_value(&mut state.filter.status, Some(status), format::status(status));
            }
        });
    if state.filter.status != before {
        state.mark_filter_dirty();
    }
}

fn channel_combo(state: &mut UiState, ui: &mut egui::Ui) {
    let before = state.filter.channel;
    let text = state.filter.channel.map(format::channel).unwrap_or("全部通道");
    egui::ComboBox::from_id_salt("filter-channel")
        .selected_text(text)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.filter.channel, None, "全部通道");
            ui.selectable_value(&mut state.filter.channel, Some(Channel::Opp), "OPP");
            ui.selectable_value(&mut state.filter.channel, Some(Channel::Assistant), "助手");
        });
    if state.filter.channel != before {
        state.mark_filter_dirty();
    }
}

fn device_combo(state: &mut UiState, ui: &mut egui::Ui) {
    let before = state.filter.device;
    let text = state
        .filter
        .device
        .and_then(|id| {
            state
                .devices
                .iter()
                .find(|device| device.id == id)
                .map(|device| device.label())
        })
        .unwrap_or_else(|| "全部设备".to_string());
    let devices: Vec<(bt_core::DeviceId, String)> =
        state.devices.iter().map(|device| (device.id, device.label())).collect();
    egui::ComboBox::from_id_salt("filter-device")
        .selected_text(text)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut state.filter.device, None, "全部设备");
            for (id, label) in devices {
                ui.selectable_value(&mut state.filter.device, Some(id), label);
            }
        });
    if state.filter.device != before {
        state.mark_filter_dirty();
    }
}

fn clear_modal(state: &mut UiState, ui: &mut egui::Ui) {
    let response = egui::Modal::new(egui::Id::unique("clear-records")).show(ui.ctx(), |ui| {
        ui.set_width(380.0);
        ui.label(RichText::new("清空全部传输记录").strong());
        ui.add_space(6.0);
        ui.label(format!(
            "将删除全部 {} 条记录（含失败记录），此操作不可撤销。",
            state.stats.total
        ));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let confirm = ui.button("确认清空").clicked();
            let cancel = ui.button("取消").clicked();
            (confirm, cancel)
        })
        .inner
    });
    let (confirm, cancel) = response.inner;
    if confirm {
        state.confirm_clear_records();
    } else if cancel {
        state.filter.clear_confirm = false;
    }
}
