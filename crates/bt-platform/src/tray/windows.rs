//! Windows 托盘：独立消息线程上的 Shell_NotifyIcon 与弹出菜单。

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;

use bt_core::BtError;
use crossbeam_channel::{Receiver, Sender};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, PostMessageW, PostThreadMessageW, RegisterClassW, SetForegroundWindow,
    TrackPopupMenu, TranslateMessage, HICON, ICONINFO, MF_SEPARATOR, MF_STRING, MSG, TPM_NONOTIFY, TPM_RETURNCMD,
    WM_APP, WM_CLOSE, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_QUIT, WM_RBUTTONUP, WNDCLASSW,
};

use super::{icon_rgba, TrayCommand, TrayHandle};

const TRAY_ID: u32 = 1;
const CALLBACK_MESSAGE: u32 = WM_APP + 1;
const TIP: &str = "蓝牙传输";

static COMMANDS: OnceLock<Sender<TrayCommand>> = OnceLock::new();
static ICON: AtomicIsize = AtomicIsize::new(0);

pub struct WindowsTrayHandle {
    commands: Receiver<TrayCommand>,
    hwnd: isize,
    thread_id: u32,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TrayHandle for WindowsTrayHandle {
    fn commands(&self) -> Receiver<TrayCommand> {
        self.commands.clone()
    }
}

impl Drop for WindowsTrayHandle {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0);
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let icon = ICON.swap(0, Ordering::SeqCst);
        if icon != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(icon as HICON);
            }
        }
    }
}

pub fn start(_state: super::TrayMenuState) -> Result<Box<dyn TrayHandle>, BtError> {
    let (sender, receiver) = crossbeam_channel::unbounded();
    if COMMANDS.set(sender).is_err() {
        return Err(BtError::busy("系统托盘已经启动"));
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("bt-tray".to_string())
        .spawn(move || run_message_loop(ready_tx))
        .map_err(|error| BtError::internal(format!("启动托盘线程失败：{error}")))?;
    let (hwnd, thread_id) = match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(value)) => value,
        Ok(Err(message)) => {
            let _ = thread.join();
            return Err(BtError::internal(message));
        }
        Err(_) => {
            return Err(BtError::internal("托盘线程在 5 秒内未完成初始化"));
        }
    };
    Ok(Box::new(WindowsTrayHandle {
        commands: receiver,
        hwnd,
        thread_id,
        thread: Some(thread),
    }))
}

fn run_message_loop(ready: std::sync::mpsc::SyncSender<Result<(isize, u32), String>>) {
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class_name = wide(&format!("BluetoothTransferTray-{}", std::process::id()));
        let window_title = wide("蓝牙传输");
        let window_class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&window_class) == 0 {
            let _ = ready.send(Err(format!("注册托盘窗口类失败：{}", windows_error())));
            return;
        }
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            let _ = ready.send(Err(format!("创建托盘窗口失败：{}", windows_error())));
            return;
        }
        let icon = match create_icon() {
            Ok(icon) => icon,
            Err(message) => {
                let _ = ready.send(Err(message));
                DestroyWindow(hwnd);
                return;
            }
        };
        ICON.store(icon as isize, Ordering::SeqCst);
        let mut data = zeroed_notify_data();
        data.hWnd = hwnd;
        data.uID = TRAY_ID;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = CALLBACK_MESSAGE;
        data.hIcon = icon;
        let tip: Vec<u16> = TIP.encode_utf16().collect();
        for (slot, value) in data.szTip.iter_mut().zip(tip) {
            *slot = value;
        }
        if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
            let _ = ready.send(Err(format!("添加托盘图标失败：{}", windows_error())));
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(icon);
            DestroyWindow(hwnd);
            return;
        }
        let thread_id = windows_sys::Win32::System::Threading::GetCurrentThreadId();
        if ready.send(Ok((hwnd as isize, thread_id))).is_err() {
            Shell_NotifyIconW(NIM_DELETE, &data);
            DestroyWindow(hwnd);
            return;
        }
        let mut message = zeroed_message();
        loop {
            let result = GetMessageW(&mut message, std::ptr::null_mut(), 0, 0);
            if result <= 0 {
                break;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        CALLBACK_MESSAGE => {
            let event = lparam as u32;
            if event == WM_LBUTTONUP || event == WM_LBUTTONDBLCLK {
                send_command(TrayCommand::Show);
            } else if event == WM_RBUTTONUP {
                show_menu(hwnd);
            }
            0
        }
        WM_DESTROY => {
            let mut data = zeroed_notify_data();
            data.hWnd = hwnd;
            data.uID = TRAY_ID;
            Shell_NotifyIconW(NIM_DELETE, &data);
            windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    AppendMenuW(menu, MF_STRING, 1, wide("显示窗口").as_ptr());
    AppendMenuW(menu, MF_STRING, 2, wide("隐藏窗口").as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
    AppendMenuW(menu, MF_STRING, 3, wide("安装右键菜单").as_ptr());
    AppendMenuW(menu, MF_STRING, 4, wide("移除右键菜单").as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
    AppendMenuW(menu, MF_STRING, 5, wide("退出").as_ptr());
    let mut point = POINT { x: 0, y: 0 };
    GetCursorPos(&mut point);
    SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY,
        point.x,
        point.y,
        0,
        hwnd,
        std::ptr::null(),
    );
    PostMessageW(hwnd, 0x0000, 0, 0);
    DestroyMenu(menu);
    match command {
        1 => send_command(TrayCommand::Show),
        2 => send_command(TrayCommand::Hide),
        3 => send_command(TrayCommand::InstallIntegration),
        4 => send_command(TrayCommand::UninstallIntegration),
        5 => send_command(TrayCommand::Exit),
        _ => {}
    }
}

fn send_command(command: TrayCommand) {
    if let Some(sender) = COMMANDS.get() {
        let _ = sender.send(command);
    }
}

unsafe fn create_icon() -> Result<HICON, String> {
    let (rgba, width, height) = icon_rgba();
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [windows_sys::Win32::Graphics::Gdi::RGBQUAD {
            rgbBlue: 0,
            rgbGreen: 0,
            rgbRed: 0,
            rgbReserved: 0,
        }],
    };
    let dc = GetDC(std::ptr::null_mut());
    let mut pixels: *mut std::ffi::c_void = std::ptr::null_mut();
    let color = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut pixels, std::ptr::null_mut(), 0);
    ReleaseDC(std::ptr::null_mut(), dc);
    if color.is_null() || pixels.is_null() {
        return Err(format!("创建托盘位图失败：{}", windows_error()));
    }
    let target = std::slice::from_raw_parts_mut(pixels.cast::<u8>(), rgba.len());
    for (source, target) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(target.as_chunks_mut::<4>().0.iter_mut())
    {
        target[0] = source[2];
        target[1] = source[1];
        target[2] = source[0];
        target[3] = source[3];
    }
    let mask = CreateBitmap(width as i32, height as i32, 1, 1, std::ptr::null());
    let icon_info = ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    let icon = CreateIconIndirect(&icon_info);
    DeleteObject(color);
    DeleteObject(mask);
    if icon.is_null() {
        return Err(format!("创建托盘图标失败：{}", windows_error()));
    }
    Ok(icon)
}

unsafe fn zeroed_notify_data() -> NOTIFYICONDATAW {
    std::mem::zeroed()
}

unsafe fn zeroed_message() -> MSG {
    std::mem::zeroed()
}

fn windows_error() -> u32 {
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
