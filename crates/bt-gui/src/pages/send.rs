//! 发送页：目标、通道、文本、文件、文件夹、剪贴板与拖放。

use bt_api::SendItem;
use bt_core::SendSource;
use eframe::egui::{self, RichText};

use super::{empty_state, section_title};
use crate::format;
use crate::state::{ChannelChoice, UiState};
use crate::theme;

pub fn show(state: &mut UiState, ui: &mut egui::Ui) {
    section_title(ui, "目标与通道");
    ui.horizontal(|ui| {
        ui.label("设备");
        let selected = state
            .send
            .selected_device
            .and_then(|id| state.devices.iter().find(|device| device.id == id).cloned());
        let text = selected
            .as_ref()
            .map(|device| format!("{}  {}", device.label(), device.id))
            .unwrap_or_else(|| "选择目标设备".to_string());
        egui::ComboBox::from_id_salt("send-device")
            .selected_text(text)
            .width(280.0)
            .show_ui(ui, |ui| {
                for device in &state.devices {
                    let label = format!("{}  {}", device.label(), device.id);
                    if ui
                        .selectable_label(state.send.selected_device == Some(device.id), label)
                        .clicked()
                    {
                        state.send.selected_device = Some(device.id);
                    }
                }
                if state.devices.is_empty() {
                    ui.label("暂无可选设备，请先在设备页扫描");
                }
            });
        ui.label("通道");
        egui::ComboBox::from_id_salt("send-channel")
            .selected_text(state.send.channel.label())
            .show_ui(ui, |ui| {
                for choice in [ChannelChoice::Auto, ChannelChoice::Assistant, ChannelChoice::Opp] {
                    ui.selectable_value(&mut state.send.channel, choice, choice.label());
                }
            });
    });
    if let Some(device) = state.send.selected_device {
        if !state.devices.iter().any(|entry| entry.id == device) {
            state.send.selected_device = None;
        }
    }

    section_title(ui, "文本");
    ui.add(
        egui::TextEdit::multiline(&mut state.send.text)
            .desired_rows(4)
            .desired_width(f32::INFINITY)
            .hint_text("输入要发送的文本"),
    );
    ui.horizontal(|ui| {
        if ui.button("发送文本").clicked() {
            state.send_text();
        }
        if ui.button("读取剪贴板文本").clicked() {
            state.read_clipboard_text();
        }
        ui.label(
            RichText::new(format!("{} 字符", state.send.text.chars().count()))
                .size(11.0)
                .color(theme::TEXT_SECONDARY),
        );
    });

    section_title(ui, "文件与文件夹");
    ui.horizontal_wrapped(|ui| {
        if ui.button("选择文件").clicked() {
            state.choose_send_files();
        }
        if ui.button("选择文件夹").clicked() {
            state.choose_send_folder();
        }
        if ui.button("发送剪贴板图片").clicked() {
            state.read_clipboard_image();
        }
        ui.checkbox(&mut state.send.zip_single, "单文件 zip")
            .on_hover_text("选择单个文件时先打包为 zip 再入队");
    });

    ui.add_space(4.0);
    let known_bytes: u64 = state.send.items.iter().filter_map(|item| item.total_bytes).sum();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("待发送 {} 项", state.send.items.len())).strong());
        if known_bytes > 0 {
            ui.label(
                RichText::new(format!("已知大小 {}", format::bytes(known_bytes)))
                    .color(theme::TEXT_SECONDARY)
                    .size(12.0),
            );
        }
        if ui.button("清空").clicked() {
            state.clear_send_items();
        }
        let send = ui.add_enabled(!state.send.items.is_empty(), egui::Button::new("发送待发送项"));
        if send.clicked() {
            state.send_pending();
        }
        if state.send.busy {
            ui.spinner();
            ui.label("正在打包");
        }
    });

    if state.send.items.is_empty() {
        empty_state(ui, "待发送列表为空", "拖入文件或文件夹，或用上方按钮选择内容");
    } else {
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut remove = None;
                for (index, item) in state.send.items.iter().enumerate() {
                    egui::Frame::new()
                        .fill(theme::SURFACE)
                        .stroke(egui::Stroke::new(1.0, theme::BORDER))
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(kind_label(item));
                                ui.label(RichText::new(&item.display_name).strong());
                                if let Some(bytes) = item.total_bytes {
                                    ui.label(
                                        RichText::new(format::bytes(bytes))
                                            .size(11.0)
                                            .color(theme::TEXT_SECONDARY),
                                    );
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.button("移除").clicked() {
                                        remove = Some(index);
                                    }
                                });
                            });
                        });
                }
                if let Some(index) = remove {
                    state.remove_send_item(index);
                }
            });
    }

    if !state.send.warnings.is_empty() {
        ui.add_space(4.0);
        for warning in &state.send.warnings {
            ui.label(RichText::new(warning).color(theme::WARNING).size(12.0));
        }
    }
}

fn kind_label(item: &SendItem) -> &'static str {
    match item.source {
        SendSource::File(_) => "文件",
        SendSource::Files(_) => "多文件",
        SendSource::Folder(_) => "文件夹",
        SendSource::Text(_) => "文本",
    }
}
