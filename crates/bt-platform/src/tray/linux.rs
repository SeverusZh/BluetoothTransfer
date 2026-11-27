//! Linux 托盘：StatusNotifierItem 与 DBusMenu，由 ksni 提供协议实现。

use std::sync::{Arc, Mutex};

use bt_core::BtError;
use crossbeam_channel::{Receiver, Sender};
use ksni::blocking::TrayMethods;
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, Tray};

use super::{icon_rgba, TrayCommand, TrayHandle, TrayMenuState};

struct LinuxTray {
    commands: Sender<TrayCommand>,
    offline: Arc<Mutex<Option<String>>>,
}

impl Tray for LinuxTray {
    fn id(&self) -> String {
        "bluetooth-transfer".to_string()
    }

    fn title(&self) -> String {
        "蓝牙传输".to_string()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let (rgba, width, height) = icon_rgba();
        let mut data = Vec::with_capacity(rgba.len());
        for pixel in rgba.as_chunks::<4>().0 {
            // SNI 要求 ARGB32 网络字节序。
            data.extend_from_slice(&[pixel[3], pixel[0], pixel[1], pixel[2]]);
        }
        vec![Icon {
            width: width as i32,
            height: height as i32,
            data,
        }]
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            item("显示窗口", TrayCommand::Show, &self.commands),
            item("隐藏窗口", TrayCommand::Hide, &self.commands),
            MenuItem::Separator,
            item("安装右键菜单", TrayCommand::InstallIntegration, &self.commands),
            item("移除右键菜单", TrayCommand::UninstallIntegration, &self.commands),
            MenuItem::Separator,
            item("退出", TrayCommand::Exit, &self.commands),
        ]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.commands.send(TrayCommand::Show);
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        if let Ok(mut slot) = self.offline.lock() {
            *slot = Some(format!("系统未提供托盘宿主：{reason:?}"));
        }
        true
    }

    fn watcher_online(&self) {
        if let Ok(mut slot) = self.offline.lock() {
            *slot = None;
        }
    }
}

fn item(label: &str, command: TrayCommand, sender: &Sender<TrayCommand>) -> MenuItem<LinuxTray> {
    let sender = sender.clone();
    MenuItem::Standard(StandardItem {
        label: label.to_string(),
        activate: Box::new(move |_tray| {
            let _ = sender.send(command);
        }),
        ..Default::default()
    })
}

pub struct LinuxTrayHandle {
    commands: Receiver<TrayCommand>,
    offline: Arc<Mutex<Option<String>>>,
    handle: ksni::blocking::Handle<LinuxTray>,
}

impl TrayHandle for LinuxTrayHandle {
    fn commands(&self) -> Receiver<TrayCommand> {
        self.commands.clone()
    }

    fn unavailable_reason(&self) -> Option<String> {
        self.offline.lock().ok().and_then(|slot| slot.clone())
    }
}

impl Drop for LinuxTrayHandle {
    fn drop(&mut self) {
        self.handle.shutdown().wait();
    }
}

pub fn start(_state: TrayMenuState) -> Result<Box<dyn TrayHandle>, BtError> {
    let (sender, receiver) = crossbeam_channel::unbounded();
    let offline = Arc::new(Mutex::new(None));
    let tray = LinuxTray {
        commands: sender,
        offline: Arc::clone(&offline),
    };
    let handle = tray.assume_sni_available(true).spawn().map_err(|error| {
        BtError::new(
            bt_core::BtErrorKind::Unsupported,
            format!("系统托盘无法启动：{error}；请确认桌面会话的 D-Bus 与托盘宿主可用"),
        )
    })?;
    Ok(Box::new(LinuxTrayHandle {
        commands: receiver,
        offline,
        handle,
    }))
}
