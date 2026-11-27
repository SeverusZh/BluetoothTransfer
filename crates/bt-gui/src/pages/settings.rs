//! 设置页：12 项配置、认证密码、自检、旧数据迁移与平台集成。

use std::path::PathBuf;

use bt_core::{ProtectionLevel, TransferMode};
use eframe::egui::{self, RichText};

use super::{section_title, status_chip, StatusKind};
use crate::state::UiState;
use crate::theme;

pub fn show(state: &mut UiState, ui: &mut egui::Ui) {
    let dirty = state.settings.draft != state.config;
    section_title(ui, "传输");
    egui::Grid::new("settings-transfer")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("OPP 分片大小");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut state.settings.draft.opp_chunk_size)
                        .range(4096..=65023)
                        .speed(64.0),
                );
                ui.label(hint("4096–65023 字节"));
            });
            ui.end_row();

            ui.label("OPP 连接超时");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut state.settings.draft.opp_connect_timeout_s)
                        .range(1..=3600)
                        .suffix(" 秒"),
                );
            });
            ui.end_row();

            ui.label("OPP 空闲超时");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut state.settings.draft.opp_send_idle_timeout_s)
                        .range(1..=86400)
                        .suffix(" 秒"),
                );
            });
            ui.end_row();

            ui.label("重试次数");
            ui.add(egui::DragValue::new(&mut state.settings.draft.opp_retry_count).range(0..=10));
            ui.end_row();

            ui.label("重试间隔");
            ui.add(
                egui::DragValue::new(&mut state.settings.draft.opp_retry_delay_s)
                    .range(0..=300)
                    .suffix(" 秒"),
            );
            ui.end_row();

            ui.label("保护级别");
            egui::ComboBox::from_id_salt("protection")
                .selected_text(protection_label(state.settings.draft.opp_protection))
                .show_ui(ui, |ui| {
                    for level in [ProtectionLevel::Auto, ProtectionLevel::Plain, ProtectionLevel::Encrypt] {
                        ui.selectable_value(&mut state.settings.draft.opp_protection, level, protection_label(level));
                    }
                });
            ui.end_row();

            ui.label("文件名 BOM");
            ui.checkbox(&mut state.settings.draft.opp_name_bom, "写入 UTF-8 BOM");
            ui.end_row();

            ui.label("助手通道分片");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut state.settings.draft.assistant_chunk_size)
                        .range(4096..=1048557)
                        .speed(1024.0),
                );
                ui.label(hint("4096–1048557 字节"));
            });
            ui.end_row();

            ui.label("文本文件名");
            ui.add(egui::TextEdit::singleline(&mut state.settings.draft.push_text_file_name).desired_width(220.0));
            ui.end_row();
        });

    section_title(ui, "接收与通道");
    egui::Grid::new("settings-receive")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("接收目录");
            ui.horizontal(|ui| {
                let response =
                    ui.add(egui::TextEdit::singleline(&mut state.settings.receive_dir_text).desired_width(320.0));
                if response.changed() {
                    state.settings.draft.receive_dir = PathBuf::from(state.settings.receive_dir_text.trim());
                }
                if ui.button("选择").clicked() {
                    state.choose_settings_receive_dir();
                }
            });
            ui.end_row();

            ui.label("每次接收前询问");
            ui.checkbox(&mut state.settings.draft.receive_ask, "启用");
            ui.end_row();

            ui.label("通道模式");
            egui::ComboBox::from_id_salt("mode")
                .selected_text(mode_label(state.settings.draft.mode))
                .show_ui(ui, |ui| {
                    for mode in [TransferMode::Auto, TransferMode::Assistant, TransferMode::Opp] {
                        ui.selectable_value(&mut state.settings.draft.mode, mode, mode_label(mode));
                    }
                });
            ui.end_row();
        });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.add_enabled(dirty, egui::Button::new("保存设置")).clicked() {
            state.save_settings();
        }
        if ui.button("恢复默认值").clicked() {
            state.reset_settings();
        }
        if dirty {
            status_chip(ui, "有未保存的修改", StatusKind::Warning);
        }
    });

    section_title(ui, "OPP 认证密码");
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.settings.auth_input)
                .password(true)
                .hint_text("输入新密码")
                .desired_width(220.0),
        );
        if ui.button("设置密码").clicked() {
            state.set_auth_password();
        }
        if ui.button("清除密码").clicked() {
            state.clear_auth_password();
        }
        status_chip(
            ui,
            if state.settings.auth_set {
                "本次会话已设置（不回显）"
            } else {
                "密码状态不可查询"
            },
            if state.settings.auth_set {
                StatusKind::Success
            } else {
                StatusKind::Neutral
            },
        );
    });
    ui.label(hint("密码只写入系统私密存储或 0600 独立文件；设置新密码会覆盖旧密码"));

    section_title(ui, "平台集成");
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{}：{}",
            state.settings.shell_label,
            if state.settings.shell_installed {
                "已安装"
            } else {
                "未安装"
            }
        ));
        if ui.button("安装").clicked() {
            state.install_shell();
        }
        if ui.button("移除").clicked() {
            state.uninstall_shell();
        }
        let mut autostart = state.settings.autostart_enabled;
        if ui.checkbox(&mut autostart, "开机自启").changed() {
            state.toggle_autostart();
        }
        if ui.button("打开数据目录").clicked() {
            state.open_data_dir();
        }
    });

    section_title(ui, "维护");
    ui.horizontal(|ui| {
        if ui.button("运行后端自检").clicked() {
            state.run_selftest();
        }
        if let Some(report) = &state.settings.selftest_report {
            let passed = report.passed();
            status_chip(
                ui,
                if passed { "自检通过" } else { "自检未通过" },
                if passed {
                    StatusKind::Success
                } else {
                    StatusKind::Danger
                },
            );
        }
    });
    if let Some(report) = &state.settings.selftest_report {
        for check in &report.checks {
            ui.horizontal(|ui| {
                status_chip(
                    ui,
                    if check.ok { "通过" } else { "失败" },
                    if check.ok {
                        StatusKind::Success
                    } else {
                        StatusKind::Danger
                    },
                );
                ui.label(RichText::new(&check.name).strong().size(12.0));
                ui.label(RichText::new(&check.detail).size(12.0).color(theme::TEXT_SECONDARY));
            });
        }
    }

    ui.add_space(6.0);
    ui.label(RichText::new("导入旧版本数据").strong().size(13.0));
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.settings.migration_root)
                .hint_text("旧版本数据目录")
                .desired_width(320.0),
        );
        if ui.button("选择目录").clicked() {
            state.choose_migration_root();
        }
        ui.checkbox(&mut state.settings.migration_dry_run, "仅预演");
        if ui
            .add_enabled(!state.settings.busy, egui::Button::new("开始迁移"))
            .clicked()
        {
            state.run_migration();
        }
    });
    if let Some(report) = &state.settings.migration_report {
        ui.label(
            RichText::new(format!(
                "记录 {} 条 · 配置 {} · 文件 {} 个 · 跳过 {} · 警告 {} 条",
                report.imported_records,
                if report.imported_config {
                    "已导入"
                } else {
                    "未导入"
                },
                report.copied_files,
                report.skipped,
                report.warnings.len()
            ))
            .size(12.0)
            .color(theme::TEXT_SECONDARY),
        );
        for warning in &report.warnings {
            ui.label(RichText::new(warning).size(12.0).color(theme::WARNING));
        }
    }

    ui.add_space(8.0);
    ui.label(
        RichText::new(format!(
            "当前生效：OPP {} 字节，助手 {} 字节，模式 {}",
            state.config.opp_chunk_size,
            state.config.assistant_chunk_size,
            mode_label(state.config.mode)
        ))
        .size(11.0)
        .color(theme::TEXT_SECONDARY),
    );
}

fn hint(text: &str) -> RichText {
    RichText::new(text).size(11.0).color(theme::TEXT_SECONDARY)
}

fn protection_label(level: ProtectionLevel) -> &'static str {
    match level {
        ProtectionLevel::Auto => "自动",
        ProtectionLevel::Plain => "明文",
        ProtectionLevel::Encrypt => "加密",
    }
}

fn mode_label(mode: TransferMode) -> &'static str {
    match mode {
        TransferMode::Auto => "自动",
        TransferMode::Assistant => "助手通道",
        TransferMode::Opp => "OPP",
    }
}
