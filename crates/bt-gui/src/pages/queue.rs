//! 队列页：逐任务进度、状态、通道与全部控制。

use bt_api::{JobSnapshot, JobState};
use eframe::egui::{self, RichText};

use super::{empty_state, section_title, status_chip, StatusKind};
use crate::format;
use crate::state::UiState;
use crate::theme;

pub fn show(state: &mut UiState, ui: &mut egui::Ui) {
    let total = state.jobs.len();
    let running = state
        .jobs
        .iter()
        .filter(|job| job.state.is_running() || job.state == JobState::Queued)
        .count();
    section_title(ui, "队列");
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("共 {total} 个任务，进行中 {running} 个")).color(theme::TEXT_SECONDARY));
        if ui.button("取消当前任务").clicked() {
            state.cancel_current_job();
        }
        if ui.add_enabled(total > 0, egui::Button::new("取消全部")).clicked() {
            state.cancel_all_jobs();
        }
    });
    ui.separator();
    if state.jobs.is_empty() {
        empty_state(ui, "队列为空", "在发送页选择内容与设备后即可入队");
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let jobs: Vec<JobSnapshot> = state.jobs.clone();
        for job in &jobs {
            job_row(state, ui, job);
        }
    });
}

fn job_row(state: &mut UiState, ui: &mut egui::Ui, job: &JobSnapshot) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&job.item.display_name).strong());
                status_chip(ui, format::channel(job.channel), StatusKind::Neutral);
                status_chip(ui, format::job_state(job.state), state_kind(job.state));
                ui.label(
                    RichText::new(format!("→ {}", job.device_name))
                        .size(12.0)
                        .color(theme::TEXT_SECONDARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    job_buttons(state, ui, job);
                });
            });
            let percent = job.percent();
            let mut progress = egui::ProgressBar::new(percent as f32 / 100.0)
                .desired_height(8.0)
                .show_percentage();
            if job.total_bytes == 0 {
                progress = progress.text(format!("{}（总大小待后端确认）", format::bytes(job.bytes_sent)));
            }
            ui.add(progress);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} / {}   {}   {}",
                        format::bytes(job.bytes_sent),
                        format::bytes(job.total_bytes),
                        format::speed(job.bps),
                        format::eta(job)
                    ))
                    .size(12.0)
                    .color(theme::TEXT_SECONDARY),
                );
                if job.attempts > 1 {
                    ui.label(
                        RichText::new(format!("第 {} 次尝试", job.attempts))
                            .size(11.0)
                            .color(theme::WARNING),
                    );
                }
                if job.resumable {
                    ui.label(RichText::new("支持断点续传").size(11.0).color(theme::TEXT_SECONDARY));
                }
            });
            if let Some(error) = &job.error {
                ui.label(RichText::new(error).color(theme::DANGER).size(12.0));
            }
        });
}

fn job_buttons(state: &mut UiState, ui: &mut egui::Ui, job: &JobSnapshot) {
    match job.state {
        JobState::Queued | JobState::Connecting | JobState::Sending | JobState::Retrying => {
            if ui.button("取消").clicked() {
                state.cancel_job(job.id);
            }
            if ui.button("暂停").clicked() {
                state.pause_job(job.id);
            }
        }
        JobState::Paused => {
            if ui.button("取消").clicked() {
                state.cancel_job(job.id);
            }
            if ui.button("继续").clicked() {
                state.resume_job(job.id);
            }
        }
        JobState::Failed | JobState::Canceled => {
            if ui.button("移除").clicked() {
                state.remove_job(job.id);
            }
            if ui.button("重试").clicked() {
                state.retry_job(job.id);
            }
        }
        JobState::Completed => {
            if ui.button("移除").clicked() {
                state.remove_job(job.id);
            }
        }
    }
}

fn state_kind(state: JobState) -> StatusKind {
    match state {
        JobState::Completed => StatusKind::Success,
        JobState::Failed => StatusKind::Danger,
        JobState::Canceled => StatusKind::Neutral,
        JobState::Paused | JobState::Retrying => StatusKind::Warning,
        _ => StatusKind::Neutral,
    }
}
