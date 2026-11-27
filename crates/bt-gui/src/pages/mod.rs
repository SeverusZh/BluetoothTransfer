//! 页面渲染：每个页面只读取 UiState 快照并调用动作方法。

pub mod devices;
pub mod logs;
pub mod queue;
pub mod receive;
pub mod records;
pub mod send;
pub mod settings;

use eframe::egui::{self, Color32, RichText};

use crate::state::UiState;
use crate::theme;

pub fn show(page: crate::state::Page, state: &mut UiState, ui: &mut egui::Ui) {
    match page {
        crate::state::Page::Devices => devices::show(state, ui),
        crate::state::Page::Send => send::show(state, ui),
        crate::state::Page::Queue => queue::show(state, ui),
        crate::state::Page::Receive => receive::show(state, ui),
        crate::state::Page::Records => records::show(state, ui),
        crate::state::Page::Logs => logs::show(state, ui),
        crate::state::Page::Settings => settings::show(state, ui),
    }
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).strong().size(15.0));
}

pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).color(Color32::WHITE));
        });
}

pub fn status_chip(ui: &mut egui::Ui, text: &str, kind: StatusKind) {
    let color = match kind {
        StatusKind::Success => theme::SUCCESS,
        StatusKind::Warning => theme::WARNING,
        StatusKind::Danger => theme::DANGER,
        StatusKind::Neutral => theme::TEXT_SECONDARY,
    };
    egui::Frame::new()
        .fill(Color32::from_rgb(245, 247, 250))
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.0).color(color));
        });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Success,
    Warning,
    Danger,
    Neutral,
}

pub fn empty_state(ui: &mut egui::Ui, title: &str, hint: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).color(theme::TEXT_SECONDARY).size(14.0));
        ui.add_space(4.0);
        ui.label(RichText::new(hint).color(theme::TEXT_SECONDARY).size(12.0));
    });
    ui.add_space(24.0);
}

pub fn path_label(ui: &mut egui::Ui, path: &std::path::Path) {
    ui.label(
        RichText::new(path.display().to_string())
            .size(12.0)
            .color(theme::TEXT_SECONDARY),
    );
}
