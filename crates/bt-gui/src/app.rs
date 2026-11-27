//! 主窗口：面板布局、平台事件、窗口状态持久化与对话框。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bt_api::SharedBackend;
use bt_core::Config;
use eframe::egui::{self, RichText};

use crate::pages;
use crate::state::{NoticeKind, Page, UiState};
use crate::theme;

/// 打开窗口所需的运行期数据。
pub struct GuiRuntime {
    pub backend: SharedBackend,
    pub backend_error: Option<String>,
    pub default_config: Config,
    pub data_dir: Option<PathBuf>,
    pub app_paths: Option<bt_app::AppPaths>,
    pub window_file: Option<PathBuf>,
    pub inbox_root: Option<PathBuf>,
    pub own_request: bt_platform::SendRequest,
    pub minimized: bool,
    pub guard: Option<bt_platform::InstanceGuard>,
    pub window_state: bt_platform::WindowState,
}

pub struct BtApp {
    state: UiState,
    guard: Option<bt_platform::InstanceGuard>,
    tray: Option<Box<dyn bt_platform::TrayHandle>>,
    window_file: Option<PathBuf>,
    window_state: bt_platform::WindowState,
    last_window_save: Instant,
    minimized: bool,
    minimized_handled: bool,
    allow_exit: bool,
    cleaned: bool,
}

impl BtApp {
    pub fn new(cc: &eframe::CreationContext<'_>, runtime: GuiRuntime) -> Self {
        theme::apply_theme(&cc.egui_ctx);
        let font = theme::install_cjk_font(&cc.egui_ctx);
        let mut state = UiState::new(
            runtime.backend,
            runtime.data_dir.clone(),
            runtime.app_paths,
            runtime.default_config,
            runtime.own_request,
        );
        state.set_font_report(font);
        if let Some(error) = runtime.backend_error {
            state.set_backend_error(error);
        }
        let menu_state = bt_platform::TrayMenuState {
            integration_installed: state.settings.shell_installed,
        };
        let tray = match bt_platform::platform_tray(menu_state) {
            Ok(handle) => {
                let note = handle.unavailable_reason();
                let active = note.is_none();
                state.set_tray(active, note);
                Some(handle)
            }
            Err(error) => {
                state.set_tray(
                    false,
                    Some(format!("系统托盘不可用：{}；关闭窗口将直接退出", error.message())),
                );
                None
            }
        };
        if let Some(root) = runtime.inbox_root.clone() {
            let inbox = bt_platform::ExternalInbox::new(root);
            state.spawn_task(move || {
                let result = inbox.clean_residue(bt_platform::inbox::RESIDUE_MAX_AGE);
                Box::new(move |state: &mut UiState| match result {
                    Ok(0) => {}
                    Ok(count) => state.logs.info(format!("已清理 {count} 个过期的外部发送残留")),
                    Err(error) => state.logs.warn(format!("清理外部发送残留失败：{}", error.message())),
                })
            });
        }
        state.drain_inbox_now();
        Self {
            state,
            guard: runtime.guard,
            tray,
            window_file: runtime.window_file,
            window_state: runtime.window_state,
            last_window_save: Instant::now(),
            minimized: runtime.minimized,
            minimized_handled: false,
            allow_exit: false,
            cleaned: false,
        }
    }

    pub fn state(&self) -> &UiState {
        &self.state
    }

    fn poll_platform(&mut self, ctx: &egui::Context) {
        if let Some(guard) = &self.guard {
            while let Ok(message) = guard.messages().try_recv() {
                let request = bt_platform::SendRequest::parse_cli(&message.args);
                self.state.schedule_external(request);
                show_and_focus(ctx);
            }
        }
        if self.state.focus_requested {
            self.state.focus_requested = false;
            show_and_focus(ctx);
        }
        let mut commands = Vec::new();
        if let Some(tray) = &self.tray {
            while let Ok(command) = tray.commands().try_recv() {
                commands.push(command);
            }
            let reason = tray.unavailable_reason();
            match (reason, self.state.tray_active) {
                (Some(reason), true) => {
                    self.state.set_tray(false, Some(reason));
                }
                (None, false) => {
                    self.state.set_tray(true, None);
                }
                _ => {}
            }
        }
        for command in commands {
            match command {
                bt_platform::TrayCommand::Show => show_and_focus(ctx),
                bt_platform::TrayCommand::Hide => {
                    self.save_window_state(ctx);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                bt_platform::TrayCommand::Exit => {
                    self.allow_exit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                bt_platform::TrayCommand::InstallIntegration => self.state.install_shell(),
                bt_platform::TrayCommand::UninstallIntegration => self.state.uninstall_shell(),
            }
        }

        if !self.minimized_handled {
            self.minimized_handled = true;
            if self.minimized && self.state.tray_active {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        if ctx.input(|input| input.viewport().close_requested()) {
            if self.allow_exit {
                return;
            }
            if self.state.tray_active {
                self.save_window_state(ctx);
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.state.info("已隐藏到系统托盘，可从托盘菜单退出");
            } else {
                self.allow_exit = true;
            }
        }

        if self.last_window_save.elapsed() >= Duration::from_secs(2) {
            self.last_window_save = Instant::now();
            self.save_window_state(ctx);
        }
    }

    fn save_window_state(&mut self, ctx: &egui::Context) {
        let Some(path) = &self.window_file else {
            return;
        };
        let info = ctx.input(|input| {
            let viewport = input.viewport();
            (viewport.outer_rect, viewport.inner_rect, viewport.maximized)
        });
        if let Some(outer) = info.0 {
            self.window_state.x = outer.min.x;
            self.window_state.y = outer.min.y;
            self.window_state.width = outer.width();
            self.window_state.height = outer.height();
        } else if let Some(inner) = info.1 {
            self.window_state.width = inner.width();
            self.window_state.height = inner.height();
        }
        if let Some(maximized) = info.2 {
            self.window_state.maximized = maximized;
        }
        let _ = self.window_state.save(path);
    }

    fn request_repaint_if_needed(&self, ctx: &egui::Context) {
        let busy = self.state.scanning
            || self.state.receive.listening
            || self
                .state
                .jobs
                .iter()
                .any(|job| job.state.is_running() || job.state == bt_api::JobState::Queued)
            || self.state.send.busy;
        if busy {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }
}

impl eframe::App for BtApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.state.poll();
        self.poll_platform(ctx);
        // 隐藏到托盘后仍需轮询托盘命令、二次启动消息与后台任务。
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.state.add_paths(dropped);
            show_and_focus(&ctx);
        }
        show_chrome(&mut self.state, ui);
        show_pairing_dialog(&mut self.state, &ctx);
        self.request_repaint_if_needed(&ctx);
    }
}

impl Drop for BtApp {
    fn drop(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;
        self.state.backend().shutdown();
        self.state.cleanup_temp_files();
        self.state.logs.flush();
        self.tray.take();
    }
}

/// 根面板布局；自检复用同一路径，保证无窗口也能覆盖全部页面。
pub fn show_chrome(state: &mut UiState, ui: &mut egui::Ui) {
    egui::Panel::top("app-top").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("蓝牙传输").strong().size(18.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if state.scanning {
                    ui.spinner();
                    ui.label("扫描中");
                }
                if state.receive.listening {
                    ui.label(RichText::new("接收中").color(theme::SUCCESS));
                }
                if state.tray_active {
                    ui.label(RichText::new("托盘已启用").size(11.0).color(theme::TEXT_SECONDARY));
                } else {
                    ui.label(RichText::new("托盘不可用").size(11.0).color(theme::TEXT_SECONDARY));
                }
            });
        });
    });
    if let Some(notice) = state.notice.clone() {
        egui::Panel::top("app-notice").show(ui, |ui| {
            let (fill, text_color) = match notice.kind {
                NoticeKind::Info => (egui::Color32::from_rgb(233, 242, 252), theme::TEXT),
                NoticeKind::Warn => (egui::Color32::from_rgb(255, 244, 206), theme::WARNING),
                NoticeKind::Error => (egui::Color32::from_rgb(253, 231, 233), theme::DANGER),
            };
            egui::Frame::new()
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&notice.text).color(text_color).size(12.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("关闭").clicked() {
                                state.dismiss_notice();
                            }
                        });
                    });
                });
        });
    }
    egui::Panel::bottom("app-status").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&state.status).size(12.0).color(theme::TEXT_SECONDARY));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .size(11.0)
                        .color(theme::TEXT_SECONDARY),
                );
            });
        });
    });
    egui::Panel::left("app-nav")
        .default_size(128.0)
        .resizable(false)
        .show(ui, |ui| {
            ui.add_space(6.0);
            for page in Page::ALL {
                let selected = state.page == page;
                if ui
                    .add_sized(
                        [ui.available_width(), 30.0],
                        egui::Button::new(page.label()).selected(selected),
                    )
                    .clicked()
                {
                    state.page(page);
                }
            }
        });
    egui::CentralPanel::default().show(ui, |ui| {
        pages::show(state.page, state, ui);
    });
}

/// 配对确认、PIN 与匹配口令对话框。
pub fn show_pairing_dialog(state: &mut UiState, ctx: &egui::Context) {
    let Some(pairing) = state.pairing.clone() else {
        return;
    };
    let mut pin = state.pin_input.clone();
    let response = egui::Modal::new(egui::Id::unique("pairing-dialog")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.label(RichText::new("配对请求").strong().size(15.0));
        ui.add_space(4.0);
        ui.label(format!("设备 {}  {}", pairing.device.label(), pairing.device.id));
        ui.add_space(6.0);
        match pairing.kind {
            bt_api::PinPromptKind::Confirm => {
                ui.label("请确认设备两端显示的配对请求一致。");
            }
            bt_api::PinPromptKind::Passkey(code) => {
                ui.label(format!("对端显示的口令：{code:06}"));
                ui.label("核对一致后再允许。");
            }
            bt_api::PinPromptKind::Pin => {
                ui.label("输入对端显示的 PIN：");
                ui.add(egui::TextEdit::singleline(&mut pin).password(true).desired_width(180.0));
                ui.label(
                    RichText::new("PIN 只用于本次配对，不写入日志与记录")
                        .size(11.0)
                        .color(theme::TEXT_SECONDARY),
                );
            }
        }
        ui.add_space(10.0);
        let pin_ready = !matches!(pairing.kind, bt_api::PinPromptKind::Pin) || !pin.trim().is_empty();
        ui.horizontal(|ui| {
            let accept = ui.add_enabled(pin_ready, egui::Button::new("允许")).clicked();
            let reject = ui.button("拒绝").clicked();
            (accept, reject)
        })
        .inner
    });
    state.pin_input = pin;
    let (accept, reject) = response.inner;
    if accept {
        state.answer_pairing(true);
    } else if reject {
        state.answer_pairing(false);
    }
}

/// 自检入口：把根面板渲染到临时 Context，不创建窗口。
pub fn render_headless(state: &mut UiState, ctx: &egui::Context) {
    let input = egui::RawInput::default();
    let output = ctx.run_ui(input, |ui| {
        show_chrome(state, ui);
    });
    output.drop_without_applying_deltas();
}

fn show_and_focus(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
}
