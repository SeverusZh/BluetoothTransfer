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

/// 一帧页面渲染后的滚动度量，供无头测试断言滚动容器生效。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PageView {
    pub content_height: f32,
    pub viewport_height: f32,
    pub scroll_offset: f32,
}

/// 渲染页面：纵向滚动容器统一在这里创建，页面内容不再各自嵌套纵向滚动区域。
pub fn show(page: crate::state::Page, state: &mut UiState, ui: &mut egui::Ui) -> PageView {
    let mut area = egui::ScrollArea::vertical()
        .id_salt(("page-scroll", page.key()))
        .auto_shrink([false, false]);
    if page == crate::state::Page::Logs {
        area = area.stick_to_bottom(state.log_follow);
    }
    let output = area.show(ui, |ui| match page {
        crate::state::Page::Devices => devices::show(state, ui),
        crate::state::Page::Send => send::show(state, ui),
        crate::state::Page::Queue => queue::show(state, ui),
        crate::state::Page::Receive => receive::show(state, ui),
        crate::state::Page::Records => records::show(state, ui),
        crate::state::Page::Logs => logs::show(state, ui),
        crate::state::Page::Settings => settings::show(state, ui),
    });
    PageView {
        content_height: output.content_size.y,
        viewport_height: output.inner_rect.height(),
        scroll_offset: output.state.clamped_offset().y,
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use bt_api::{Backend, Device, JobSnapshot, JobState, LogLevel, MockBackend, SendItem};
    use bt_core::{Channel, DeviceId, Direction, Record, RecordKind, RecordStatus, SendSource};

    use super::*;
    use crate::state::{Page, ReceivedEntry, UiState};

    const VIEWPORT: [f32; 2] = [520.0, 220.0];

    fn screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VIEWPORT[0], VIEWPORT[1]))
    }

    fn frame_input(time: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(screen()),
            time: Some(time),
            events,
            ..Default::default()
        }
    }

    fn wheel_event() -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -6.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn pointer_event() -> egui::Event {
        egui::Event::PointerMoved(egui::pos2(VIEWPORT[0] / 2.0, VIEWPORT[1] / 2.0))
    }

    fn seeded_state() -> UiState {
        let backend = Arc::new(MockBackend::default());
        let config = backend.config();
        let mut state = UiState::new(backend, None, None, config, bt_platform::SendRequest::default());
        let device_id = DeviceId::parse("AA:BB:CC:DD:EE:01").expect("设备地址合法");
        state.devices = (0..4)
            .map(|index| {
                let mut device = Device::new(device_id, format!("设备 {index}"));
                device.paired = index % 2 == 0;
                device.connected = index == 0;
                device.rssi = Some(-40 - index as i16);
                device.assistant_state = bt_core::AssistantState::Available;
                device
            })
            .collect();
        state.jobs = (0..6)
            .map(|index| JobSnapshot {
                id: bt_api::JobId(index + 1),
                device_id,
                device_name: "设备 0".to_string(),
                item: SendItem::from_source(SendSource::File(PathBuf::from(format!("/tmp/样本-{index}.bin")))),
                channel: Channel::Opp,
                state: JobState::Sending,
                bytes_sent: 1024,
                total_bytes: 4096,
                bps: 2048,
                eta_seconds: Some(3),
                attempts: 1,
                error: None,
                resumable: true,
                created_at: "2026-11-27T10:00:00Z".to_string(),
                updated_at: "2026-11-27T10:00:05Z".to_string(),
            })
            .collect();
        state.records = (0..8)
            .map(|index| {
                let mut record = Record::running(
                    Direction::Send,
                    Channel::Opp,
                    RecordKind::File,
                    format!("样本-{index}.bin"),
                    "2026-11-27T10:00:00Z",
                );
                record.id = index + 1;
                record.device_name = "设备 0".to_string();
                record.status = RecordStatus::Completed;
                record.byte_size = 1024 * (index as u64 + 1);
                record.transferred = record.byte_size;
                record.finished_at = Some("2026-11-27T10:00:10Z".to_string());
                record
            })
            .collect();
        state.receive.completed = (0..6)
            .map(|index| ReceivedEntry {
                name: format!("接收-{index}.bin"),
                path: Some(PathBuf::from(format!("/tmp/接收-{index}.bin"))),
                size: 2048,
                ok: index % 3 != 0,
                error: None,
                at: "2026-11-27T10:00:00Z".to_string(),
            })
            .collect();
        state.send.items = (0..6)
            .map(|index| SendItem::from_source(SendSource::File(PathBuf::from(format!("/tmp/待发-{index}.bin")))))
            .collect();
        for index in 0..24 {
            state.logs.push(
                LogLevel::Info,
                format!("日志条目 {index}"),
                Some("2026-11-27T10:00:00Z".to_string()),
            );
        }
        state
    }

    fn render(state: &mut UiState, page: Page, ctx: &egui::Context, input: egui::RawInput) -> PageView {
        let mut view = PageView::default();
        let output = ctx.run_ui(input, |ui| {
            state.page = page;
            view = show(page, state, ui);
        });
        output.drop_without_applying_deltas();
        view
    }

    fn settle(ctx: &egui::Context, state: &mut UiState, page: Page, start: f64) -> f64 {
        let mut time = start;
        render(state, page, ctx, frame_input(time, Vec::new()));
        time += 0.016;
        time
    }

    #[test]
    fn every_page_exceeds_small_viewport() {
        let ctx = egui::Context::default();
        crate::theme::apply_theme(&ctx);
        let mut state = seeded_state();
        let mut time = 0.0;
        for page in Page::ALL {
            time = settle(&ctx, &mut state, page, time);
            let view = render(&mut state, page, &ctx, frame_input(time, Vec::new()));
            time += 0.016;
            assert!(
                view.viewport_height > 100.0,
                "页面 {} 视口高度 {} 异常，滚动容器没有占满可用区域",
                page.label(),
                view.viewport_height
            );
            assert!(
                view.content_height > view.viewport_height,
                "页面 {} 内容高度 {} 未超过视口 {}",
                page.label(),
                view.content_height,
                view.viewport_height
            );
        }
    }

    #[test]
    fn wheel_scrolls_settings_page() {
        let ctx = egui::Context::default();
        crate::theme::apply_theme(&ctx);
        let mut state = seeded_state();
        let time = settle(&ctx, &mut state, Page::Settings, 0.0);
        let view = render(
            &mut state,
            Page::Settings,
            &ctx,
            frame_input(time, vec![pointer_event(), wheel_event()]),
        );
        assert!(
            view.scroll_offset > 0.0,
            "设置页滚轮未生效，滚动偏移 {}",
            view.scroll_offset
        );
    }

    #[test]
    fn wheel_scrolls_every_page() {
        let ctx = egui::Context::default();
        crate::theme::apply_theme(&ctx);
        let mut state = seeded_state();
        let mut time = 0.0;
        for page in Page::ALL {
            time = settle(&ctx, &mut state, page, time);
            let view = render(
                &mut state,
                page,
                &ctx,
                frame_input(time, vec![pointer_event(), wheel_event()]),
            );
            time += 0.016;
            assert!(
                view.scroll_offset > 0.0,
                "页面 {} 滚轮未生效，滚动偏移 {}",
                page.label(),
                view.scroll_offset
            );
        }
    }
}
