//! 单实例守卫：首个实例持有平台命名对象，二次启动把参数转发给首个实例。
//!
//! Linux 用抽象命名空间 socket 判定并传输消息，锁文件只用于诊断；
//! Windows 用命名互斥量判定，命名管道传输消息。转发失败返回 false，由调用方决定是否退化为普通启动。

use std::path::PathBuf;
use std::time::Duration;

use bt_core::BtError;
use crossbeam_channel::Receiver;
use serde::{Deserialize, Serialize};

/// 二次启动允许等待首个实例就绪的时间。
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(2);
const SIGNAL_RETRY: Duration = Duration::from_millis(50);

/// 首个实例收到的启动参数。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceMessage {
    pub args: Vec<String>,
}

impl InstanceMessage {
    pub fn new(args: Vec<String>) -> Self {
        Self { args }
    }

    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{\"args\":[]}".to_string())
    }

    pub fn decode(payload: &str) -> Self {
        serde_json::from_str(payload).unwrap_or_else(|_| Self {
            args: payload
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| line.to_string())
                .collect(),
        })
    }

    pub fn to_paths(&self) -> Vec<PathBuf> {
        self.args.iter().map(PathBuf::from).collect()
    }
}

/// 单实例守卫；存活期间持有平台锁，Drop 时释放并停止接收线程。
pub struct InstanceGuard {
    name: String,
    primary: bool,
    messages: Receiver<InstanceMessage>,
    _platform: platform::Guard,
}

impl InstanceGuard {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 本进程是否为首个实例。
    pub fn is_primary(&self) -> bool {
        self.primary
    }

    /// 首个实例收到的二次启动消息；非首个实例永远为空。
    pub fn messages(&self) -> Receiver<InstanceMessage> {
        self.messages.clone()
    }
}

impl std::fmt::Debug for InstanceGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InstanceGuard")
            .field("name", &self.name)
            .field("primary", &self.primary)
            .finish_non_exhaustive()
    }
}

/// 获取单实例锁；返回 None 表示已有实例在运行，参数应通过 [`signal_existing`] 转交。
pub fn acquire_instance(name: &str) -> Result<Option<InstanceGuard>, BtError> {
    let name = validate_name(name)?;
    let (sender, receiver) = crossbeam_channel::unbounded();
    match platform::acquire(&name, sender)? {
        Some(platform) => Ok(Some(InstanceGuard {
            name,
            primary: true,
            messages: receiver,
            _platform: platform,
        })),
        None => Ok(None),
    }
}

/// 通知已运行的首个实例并转发参数；返回 false 表示等待超时仍未找到可用的首个实例。
pub fn signal_existing(name: &str, message: &InstanceMessage) -> Result<bool, BtError> {
    let name = validate_name(name)?;
    platform::signal(&name, message, SIGNAL_TIMEOUT, SIGNAL_RETRY)
}

fn validate_name(name: &str) -> Result<String, BtError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(BtError::invalid("单实例名不能为空"));
    }
    if trimmed.contains(['\\', '/', '\0']) {
        return Err(BtError::invalid("单实例名不能包含路径分隔符"));
    }
    Ok(trimmed.to_string())
}

#[cfg(target_os = "linux")]
mod platform {
    use std::io::{Read, Write};
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use bt_core::BtError;
    use crossbeam_channel::Sender;

    use super::InstanceMessage;
    use crate::paths;

    pub struct Guard {
        thread: Option<JoinHandle<()>>,
        stop: Arc<AtomicBool>,
        lock_file: Option<PathBuf>,
        address: String,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Ok(stream) = UnixStream::connect_addr(&address_for(&self.address)) {
                drop(stream);
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            if let Some(path) = self.lock_file.take() {
                remove_own_lock(&path);
            }
        }
    }

    pub fn acquire(name: &str, sender: Sender<InstanceMessage>) -> Result<Option<Guard>, BtError> {
        let address = address_name(name);
        let listener = match UnixListener::bind_addr(&address_for(&address)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => return Ok(None),
            Err(error) => return Err(BtError::internal(format!("创建单实例 socket 失败：{error}"))),
        };
        listener
            .set_nonblocking(false)
            .map_err(|error| BtError::internal(format!("配置单实例 socket 失败：{error}")))?;
        let lock_file = write_lock_file(name)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("bt-instance".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    if thread_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(mut stream) = stream else {
                        continue;
                    };
                    let mut payload = String::new();
                    let _ = stream.read_to_string(&mut payload);
                    if !payload.trim().is_empty() {
                        let message = InstanceMessage::decode(&payload);
                        if sender.send(message).is_err() {
                            break;
                        }
                    }
                }
            })
            .map_err(|error| BtError::internal(format!("启动单实例接收线程失败：{error}")))?;
        Ok(Some(Guard {
            thread: Some(thread),
            stop,
            lock_file,
            address,
        }))
    }

    pub fn signal(name: &str, message: &InstanceMessage, timeout: Duration, retry: Duration) -> Result<bool, BtError> {
        let address = address_name(name);
        let deadline = Instant::now() + timeout;
        let payload = message.encode();
        loop {
            match UnixStream::connect_addr(&address_for(&address)) {
                Ok(mut stream) => {
                    stream
                        .write_all(payload.as_bytes())
                        .map_err(|error| BtError::internal(format!("转发启动参数失败：{error}")))?;
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    return Ok(true);
                }
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                    return Ok(false);
                }
                Err(_) if Instant::now() < deadline => std::thread::sleep(retry),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if Instant::now() >= deadline {
                        return Ok(false);
                    }
                    std::thread::sleep(retry);
                }
                Err(_) => return Ok(false),
            }
        }
    }

    fn address_name(name: &str) -> String {
        format!("bt-{}-{name}", current_uid())
    }

    fn address_for(name: &str) -> SocketAddr {
        SocketAddr::from_abstract_name(name.as_bytes()).expect("抽象 socket 名只含合法字节")
    }

    fn current_uid() -> u32 {
        // 抽象 socket 已按网络命名空间隔离，用进程属主区分同一机器上的不同用户。
        unsafe { libc_geteuid() }
    }

    extern "C" {
        #[link_name = "geteuid"]
        fn libc_geteuid() -> u32;
    }

    fn write_lock_file(name: &str) -> Result<Option<PathBuf>, BtError> {
        // 锁文件只用于诊断；运行目录不可写时不影响单实例判定。
        let Some(dir) = paths::runtime_dir() else {
            return Ok(None);
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return Ok(None);
        }
        let path = dir.join(format!("bluetooth-transfer-{name}.lock"));
        let body = format!("{}", std::process::id());
        if std::fs::write(&path, body).is_err() {
            return Ok(None);
        }
        Ok(Some(path))
    }

    fn remove_own_lock(path: &std::path::Path) {
        let own = std::process::id().to_string();
        if std::fs::read_to_string(path).is_ok_and(|body| body.trim() == own) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::time::{Duration, Instant};

    use bt_core::BtError;
    use crossbeam_channel::Sender;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE,
        INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

    use super::InstanceMessage;

    const PIPE_BUFFER: u32 = 64 * 1024;

    pub struct Guard {
        mutex: HANDLE,
        pipe: HANDLE,
        thread: Option<std::thread::JoinHandle<()>>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        pipe_name: Vec<u16>,
    }

    unsafe impl Send for Guard {}

    impl Drop for Guard {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = signal_raw(&self.pipe_name, "");
            unsafe {
                if !self.pipe.is_null() {
                    CloseHandle(self.pipe);
                }
                if !self.mutex.is_null() {
                    ReleaseMutex(self.mutex);
                    CloseHandle(self.mutex);
                }
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    pub fn acquire(name: &str, sender: Sender<InstanceMessage>) -> Result<Option<Guard>, BtError> {
        let mutex_name = wide(&format!("Local\\BluetoothTransfer.{name}.Mutex"));
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
        if mutex.is_null() {
            return Err(BtError::internal(format!("创建单实例互斥量失败：{}", last_error())));
        }
        let wait = unsafe { WaitForSingleObject(mutex, 0) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_ABANDONED {
            unsafe {
                CloseHandle(mutex);
            }
            return Ok(None);
        }
        let pipe_name_text = format!("\\\\.\\pipe\\BluetoothTransfer.{name}");
        let pipe_name = wide(&pipe_name_text);
        let pipe = unsafe {
            CreateNamedPipeW(
                pipe_name.as_ptr(),
                0x0000_0001,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                PIPE_BUFFER,
                PIPE_BUFFER,
                0,
                std::ptr::null(),
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            unsafe {
                ReleaseMutex(mutex);
                CloseHandle(mutex);
            }
            return Err(BtError::internal(format!("创建单实例管道失败：{}", last_error())));
        }
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = std::sync::Arc::clone(&stop);
        let pipe_address = pipe as usize;
        let thread = std::thread::Builder::new()
            .name("bt-instance".to_string())
            .spawn(move || {
                let pipe = pipe_address as HANDLE;
                loop {
                    if thread_stop.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) };
                    if connected == 0 {
                        let error = unsafe { GetLastError() };
                        if error != ERROR_PIPE_CONNECTED && error != ERROR_IO_PENDING {
                            continue;
                        }
                    }
                    let mut buffer = vec![0u8; PIPE_BUFFER as usize];
                    let mut read = 0u32;
                    let ok = unsafe {
                        ReadFile(
                            pipe,
                            buffer.as_mut_ptr().cast(),
                            buffer.len() as u32,
                            &mut read,
                            std::ptr::null_mut(),
                        )
                    };
                    if ok != 0 && read > 0 {
                        let payload = String::from_utf8_lossy(&buffer[..read as usize]).into_owned();
                        if !payload.trim().is_empty() {
                            let message = InstanceMessage::decode(&payload);
                            if sender.send(message).is_err() {
                                break;
                            }
                        }
                    }
                    unsafe {
                        DisconnectNamedPipe(pipe);
                    }
                }
            })
            .map_err(|error| BtError::internal(format!("启动单实例接收线程失败：{error}")))?;
        Ok(Some(Guard {
            mutex,
            pipe,
            thread: Some(thread),
            stop,
            pipe_name,
        }))
    }

    pub fn signal(name: &str, message: &InstanceMessage, timeout: Duration, retry: Duration) -> Result<bool, BtError> {
        let pipe_name = wide(&format!("\\\\.\\pipe\\BluetoothTransfer.{name}"));
        let deadline = Instant::now() + timeout;
        loop {
            if signal_raw(&pipe_name, &message.encode())? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(retry);
        }
    }

    fn signal_raw(pipe_name: &[u16], payload: &str) -> Result<bool, BtError> {
        let handle = unsafe {
            CreateFileW(
                pipe_name.as_ptr(),
                FILE_GENERIC_WRITE,
                FILE_SHARE_NONE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Ok(false);
        }
        let mut written = 0u32;
        let ok = unsafe {
            WriteFile(
                handle,
                payload.as_ptr().cast(),
                payload.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        unsafe {
            CloseHandle(handle);
        }
        if ok == 0 {
            return Ok(false);
        }
        let _ = written;
        let _ = ERROR_BROKEN_PIPE;
        Ok(true)
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error() -> u32 {
        unsafe { GetLastError() }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use std::time::Duration;

    use bt_core::BtError;
    use crossbeam_channel::Sender;

    use super::InstanceMessage;

    pub struct Guard;

    pub fn acquire(_name: &str, _sender: Sender<InstanceMessage>) -> Result<Option<Guard>, BtError> {
        Err(BtError::unsupported("单实例锁在当前平台不可用"))
    }

    pub fn signal(
        _name: &str,
        _message: &InstanceMessage,
        _timeout: Duration,
        _retry: Duration,
    ) -> Result<bool, BtError> {
        Err(BtError::unsupported("单实例转发在当前平台不可用"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    fn unique_name() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, Ordering::SeqCst);
        format!("test-{}-{}-{sequence}", std::process::id(), bt_core::unix_now())
    }

    #[test]
    fn message_round_trips() {
        let message = InstanceMessage::new(vec!["--send".into(), "/tmp/带空格 的.txt".into()]);
        assert_eq!(InstanceMessage::decode(&message.encode()), message);
        assert_eq!(InstanceMessage::decode(""), InstanceMessage::default());
        assert!(validate_name(" ").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn second_instance_forwards_message_to_primary() {
        let name = unique_name();
        let guard = acquire_instance(&name).unwrap().expect("首个实例应成为主实例");
        assert!(guard.is_primary());
        assert!(acquire_instance(&name).unwrap().is_none());
        let message = InstanceMessage::new(vec!["--send".into(), "/tmp/a.txt".into()]);
        assert!(signal_existing(&name, &message).unwrap());
        let received = guard
            .messages()
            .recv_timeout(Duration::from_secs(3))
            .expect("首个实例应收到转发消息");
        assert_eq!(received, message);
        drop(guard);
        assert!(acquire_instance(&name).unwrap().is_some());
    }
}
