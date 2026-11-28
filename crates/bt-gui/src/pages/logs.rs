//! 日志页：级别过滤、自动滚动、落盘位置与诊断导出。

use bt_api::LogLevel;
use eframe::egui::{self, RichText};

use super::{section_title, status_chip, StatusKind};
use crate::format;
use crate::state::UiState;
use crate::theme;

pub fn show(state: &mut UiState, ui: &mut egui::Ui) {
    section_title(ui, "运行日志");
    ui.horizontal_wrapped(|ui| {
        ui.label("最低级别");
        egui::ComboBox::from_id_salt("log-level")
            .selected_text(format::log_level(state.log_min_level))
            .show_ui(ui, |ui| {
                for level in [LogLevel::Debug, LogLevel::Info, LogLevel::Warn, LogLevel::Error] {
                    ui.selectable_value(&mut state.log_min_level, level, format::log_level(level));
                }
            });
        ui.checkbox(&mut state.log_follow, "自动滚动");
        if ui.button("打开日志目录").clicked() {
            state.open_log_dir();
        }
        if ui.button("导出诊断").clicked() {
            state.export_diagnostics();
        }
        ui.label(
            RichText::new(format!("内存 {} 条", state.logs.len()))
                .size(11.0)
                .color(theme::TEXT_SECONDARY),
        );
    });
    if let Some(path) = &state.logs.path {
        ui.label(
            RichText::new(format!("日志文件 {}", path.display()))
                .size(11.0)
                .color(theme::TEXT_SECONDARY),
        );
    }
    if let Some(error) = &state.logs.write_error {
        ui.label(RichText::new(error).color(theme::DANGER).size(12.0));
    }
    ui.separator();
    let entries: Vec<crate::log_sink::LogEntry> = state.logs.filtered(state.log_min_level).cloned().collect();
    if entries.is_empty() {
        ui.label(RichText::new("暂无日志").color(theme::TEXT_SECONDARY));
        return;
    }
    for entry in &entries {
        let color = match entry.level {
            LogLevel::Debug => theme::TEXT_SECONDARY,
            LogLevel::Info => theme::TEXT,
            LogLevel::Warn => theme::WARNING,
            LogLevel::Error => theme::DANGER,
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format::timestamp(&entry.at))
                    .monospace()
                    .size(11.0)
                    .color(theme::TEXT_SECONDARY),
            );
            status_chip(
                ui,
                format::log_level(entry.level),
                match entry.level {
                    LogLevel::Debug => StatusKind::Neutral,
                    LogLevel::Info => StatusKind::Neutral,
                    LogLevel::Warn => StatusKind::Warning,
                    LogLevel::Error => StatusKind::Danger,
                },
            );
            ui.label(RichText::new(&entry.message).size(12.0).color(color));
        });
    }
}
