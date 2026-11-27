//! Linux 平台适配：BlueZ D-Bus 负责发现与配对，AF_BLUETOOTH RFCOMM 负责连接与服务广播。
//!
//! 运行期依赖系统的 BlueZ 与系统 D-Bus；本机无蓝牙硬件时只在编译层验证。

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bt_core::{BtError, BtErrorKind, Channel, DeviceId, ProtectionLevel};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Str};

use crate::{
    AcceptHandler, DiscoveredDevice, Discovery, PairAnswer, PairPrompt, PairPromptKind, Pairing, PromptHandler,
    RfcommConnector, RfcommService, ServiceOptions, TransportResult, TransportStream,
};

mod sdp;

const BTPROTO_RFCOMM: i32 = 3;
const BTPROTO_L2CAP: i32 = 0;
const SOL_BLUETOOTH: i32 = 274;
const BT_SECURITY: i32 = 4;
const BT_SECURITY_LOW: u8 = 0;
const BT_SECURITY_MEDIUM: u8 = 1;
const BT_SECURITY_HIGH: u8 = 2;

#[repr(C)]
struct BtSecurity {
    level: u8,
    key_size: u8,
}
const RFCOMM_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const SDP_TIMEOUT: Duration = Duration::from_secs(5);
const AGENT_PATH: &str = "/org/bluetoothtransfer/agent";

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
const PROFILE_PATH: &str = "/org/bluetoothtransfer/profile";

#[repr(C)]
struct SockaddrRc {
    rc_family: u16,
    rc_bdaddr: [u8; 6],
    rc_channel: u8,
}

#[repr(C)]
struct SockaddrL2 {
    l2_family: u16,
    l2_psm: u16,
    l2_bdaddr: [u8; 6],
    l2_cid: u16,
    l2_bdaddr_type: u8,
}

fn bdaddr(device: &DeviceId) -> [u8; 6] {
    let mut bytes = *device.as_bytes();
    bytes.reverse();
    bytes
}

fn timeval(duration: Option<Duration>) -> libc::timeval {
    match duration {
        Some(duration) => libc::timeval {
            tv_sec: duration.as_secs() as libc::time_t,
            tv_usec: duration.subsec_micros() as libc::suseconds_t,
        },
        None => libc::timeval { tv_sec: 0, tv_usec: 0 },
    }
}

fn set_fd_timeouts(fd: std::os::fd::RawFd, read: Option<Duration>, write: Option<Duration>) -> std::io::Result<()> {
    for (option, duration) in [(libc::SO_RCVTIMEO, read), (libc::SO_SNDTIMEO, write)] {
        let value = timeval(duration);
        // 安全性：fd 来自有效的 socket，timeval 指针与长度都指向本栈变量。
        let result = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                &value as *const libc::timeval as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

struct FdStream {
    file: File,
    peer: Option<DeviceId>,
}

impl FdStream {
    fn from_fd(fd: OwnedFd, peer: Option<DeviceId>) -> Self {
        Self {
            file: File::from(fd),
            peer,
        }
    }
}

impl Read for FdStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buffer)
    }
}

impl Write for FdStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.file.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl TransportStream for FdStream {
    fn set_timeouts(&mut self, read: Option<Duration>, write: Option<Duration>) -> TransportResult<()> {
        set_fd_timeouts(self.file.as_raw_fd(), read, write)?;
        Ok(())
    }

    fn peer(&self) -> Option<DeviceId> {
        self.peer
    }

    fn close_stream(&mut self) -> TransportResult<()> {
        // 安全性：文件描述符在本结构体内存活，shutdown 只发送关闭信号。
        unsafe {
            libc::shutdown(self.file.as_raw_fd(), libc::SHUT_RDWR);
        }
        Ok(())
    }
}

struct Agent {
    prompt_handler: Arc<Mutex<Option<PromptHandler>>>,
}

impl Agent {
    fn ask(&self, device: &str, kind: PairPromptKind) -> PairAnswer {
        let Some(handler) = self.prompt_handler.lock().ok().and_then(|handler| handler.clone()) else {
            return PairAnswer::Reject;
        };
        handler(PairPrompt {
            device: device_id_from_path(device),
            kind,
        })
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}

    fn request_confirmation(&self, device: OwnedObjectPath, _passkey: u32) -> zbus::fdo::Result<()> {
        if self.ask(device.as_str(), PairPromptKind::Confirm).accepted() {
            Ok(())
        } else {
            Err(zbus::fdo::Error::AuthFailed("用户拒绝配对".to_string()))
        }
    }

    fn request_authorization(&self, device: OwnedObjectPath) -> zbus::fdo::Result<()> {
        if self.ask(device.as_str(), PairPromptKind::Confirm).accepted() {
            Ok(())
        } else {
            Err(zbus::fdo::Error::AuthFailed("用户拒绝授权".to_string()))
        }
    }

    fn authorize_service(&self, device: OwnedObjectPath, _uuid: String) -> zbus::fdo::Result<()> {
        if self.ask(device.as_str(), PairPromptKind::Confirm).accepted() {
            Ok(())
        } else {
            Err(zbus::fdo::Error::AuthFailed("用户拒绝服务授权".to_string()))
        }
    }

    fn request_pin_code(&self, device: OwnedObjectPath) -> zbus::fdo::Result<String> {
        match self.ask(device.as_str(), PairPromptKind::Pin) {
            PairAnswer::Pin(pin) if !pin.is_empty() => Ok(pin),
            _ => Err(zbus::fdo::Error::AuthFailed("用户拒绝或未提供 PIN".to_string())),
        }
    }

    fn display_passkey(&self, device: OwnedObjectPath, passkey: u32, _entered: u16) {
        let _ = self.ask(device.as_str(), PairPromptKind::Passkey(passkey));
    }

    fn cancel(&self) {}
}

struct ProfileIface {
    handler: Arc<Mutex<Option<AcceptHandler>>>,
}

#[zbus::interface(name = "org.bluez.Profile1")]
impl ProfileIface {
    fn new_connection(
        &self,
        device: OwnedObjectPath,
        fd: zbus::zvariant::OwnedFd,
        _properties: HashMap<String, OwnedValue>,
    ) {
        let peer = Some(device_id_from_path(device.as_str()));
        let fd: OwnedFd = fd.into();
        let stream = Box::new(FdStream::from_fd(fd, peer));
        let handler = self.handler.lock().ok().and_then(|handler| handler.clone());
        if let Some(handler) = handler {
            std::thread::spawn(move || handler(stream));
        }
    }

    fn request_disconnection(&self, _device: OwnedObjectPath) {}

    fn release(&self) {
        if let Ok(mut handler) = self.handler.lock() {
            *handler = None;
        }
    }
}

pub struct BlueZTransport {
    connection: Connection,
    adapter: String,
    scanning: Arc<AtomicBool>,
    scan_generation: Arc<AtomicU64>,
    prompt_handler: Arc<Mutex<Option<PromptHandler>>>,
    profile_handler: Mutex<Option<Arc<Mutex<Option<AcceptHandler>>>>>,
    profile_uuid: Mutex<Option<String>>,
}

impl std::fmt::Debug for BlueZTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BlueZTransport")
            .field("adapter", &self.adapter)
            .finish_non_exhaustive()
    }
}

impl BlueZTransport {
    pub fn connect() -> Result<Self, BtError> {
        let connection = Connection::system()
            .map_err(|error| BtError::unsupported(format!("无法连接系统 D-Bus：{error}（请确认 BlueZ 服务已启动）")))?;
        let adapter = find_adapter(&connection)?;
        let prompt_handler: Arc<Mutex<Option<PromptHandler>>> = Arc::new(Mutex::new(None));
        let _ = connection.object_server().at(
            AGENT_PATH,
            Agent {
                prompt_handler: prompt_handler.clone(),
            },
        );
        let manager =
            Proxy::new(&connection, "org.bluez", "/org/bluez", "org.bluez.AgentManager1").map_err(dbus_error)?;
        let agent_path =
            ObjectPath::try_from(AGENT_PATH).map_err(|error| BtError::internal(format!("非法对象路径：{error}")))?;
        let _: Result<(), zbus::Error> = manager.call("RegisterAgent", &(agent_path, "DisplayYesNo"));
        let default_path =
            ObjectPath::try_from(AGENT_PATH).map_err(|error| BtError::internal(format!("非法对象路径：{error}")))?;
        let _: Result<(), zbus::Error> = manager.call("RequestDefaultAgent", &(default_path,));
        Ok(Self {
            connection,
            adapter,
            scanning: Arc::new(AtomicBool::new(false)),
            scan_generation: Arc::new(AtomicU64::new(1)),
            prompt_handler,
            profile_handler: Mutex::new(None),
            profile_uuid: Mutex::new(None),
        })
    }

    fn managed_objects(&self) -> Result<ManagedObjects, BtError> {
        let proxy =
            Proxy::new(&self.connection, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager").map_err(dbus_error)?;
        let objects = proxy
            .call::<_, _, ManagedObjects>("GetManagedObjects", &())
            .map_err(dbus_error)?;
        Ok(objects)
    }

    fn device_path(&self, device: &DeviceId) -> Result<String, BtError> {
        let address = device.to_string();
        let objects = self.managed_objects()?;
        for (path, interfaces) in objects {
            let Some(properties) = interfaces.get("org.bluez.Device1") else {
                continue;
            };
            if owned_string(properties, "Address").as_deref() == Some(address.as_str()) {
                return Ok(path.as_str().to_string());
            }
        }
        Err(BtError::not_found(format!(
            "BlueZ 中未找到设备 {address}，请先扫描或配对"
        )))
    }

    fn adapter_proxy(&self) -> Result<Proxy<'_>, BtError> {
        Proxy::new(
            &self.connection,
            "org.bluez",
            self.adapter.as_str(),
            "org.bluez.Adapter1",
        )
        .map_err(dbus_error)
    }

    fn stop_discovery(&self) {
        if let Ok(proxy) = self.adapter_proxy() {
            let _: Result<(), zbus::Error> = proxy.call("StopDiscovery", &());
        }
        self.scanning.store(false, Ordering::SeqCst);
    }
}

impl Discovery for BlueZTransport {
    fn start_scan(&self, duration: Option<Duration>) -> TransportResult<()> {
        let proxy = self.adapter_proxy()?;
        proxy.call::<_, _, ()>("StartDiscovery", &()).map_err(dbus_error)?;
        self.scanning.store(true, Ordering::SeqCst);
        if let Some(duration) = duration {
            let generation = self.scan_generation.fetch_add(1, Ordering::SeqCst) + 1;
            let connection = self.connection.clone();
            let adapter = self.adapter.clone();
            let scanning = self.scanning.clone();
            let expected = self.scan_generation.clone();
            std::thread::spawn(move || {
                std::thread::sleep(duration);
                if expected.load(Ordering::SeqCst) == generation && scanning.load(Ordering::SeqCst) {
                    if let Ok(proxy) = Proxy::new(&connection, "org.bluez", adapter.as_str(), "org.bluez.Adapter1") {
                        let _: Result<(), zbus::Error> = proxy.call("StopDiscovery", &());
                    }
                    scanning.store(false, Ordering::SeqCst);
                }
            });
        }
        Ok(())
    }

    fn cancel_scan(&self) -> TransportResult<()> {
        self.scan_generation.fetch_add(1, Ordering::SeqCst);
        self.stop_discovery();
        Ok(())
    }

    fn devices(&self) -> Vec<DiscoveredDevice> {
        let Ok(objects) = self.managed_objects() else {
            return Vec::new();
        };
        let mut devices = Vec::new();
        for (path, interfaces) in objects {
            if !path.as_str().starts_with(self.adapter.as_str()) {
                continue;
            }
            let Some(properties) = interfaces.get("org.bluez.Device1") else {
                continue;
            };
            let Some(address) = owned_string(properties, "Address") else {
                continue;
            };
            let Ok(id) = DeviceId::parse(&address) else {
                continue;
            };
            let name = owned_string(properties, "Alias")
                .or_else(|| owned_string(properties, "Name"))
                .unwrap_or_else(|| address.clone());
            devices.push(DiscoveredDevice {
                id,
                name,
                rssi: owned_i16(properties, "RSSI"),
                paired: owned_bool(properties, "Paired").unwrap_or(false),
                connected: owned_bool(properties, "Connected").unwrap_or(false),
                channels: vec![Channel::Opp],
            });
        }
        devices.sort_by(|left, right| left.name.cmp(&right.name));
        devices
    }
}

impl Pairing for BlueZTransport {
    fn pair(&self, device: &DeviceId) -> TransportResult<()> {
        let path = self.device_path(device)?;
        let proxy =
            Proxy::new(&self.connection, "org.bluez", path.as_str(), "org.bluez.Device1").map_err(dbus_error)?;
        proxy.call::<_, _, ()>("Pair", &()).map_err(dbus_error)?;
        let _: Result<(), zbus::fdo::Error> = proxy.set_property("Trusted", true);
        Ok(())
    }

    fn answer(&self, _accept: bool) -> TransportResult<()> {
        Ok(())
    }

    fn unpair(&self, device: &DeviceId) -> TransportResult<()> {
        let path = self.device_path(device)?;
        let proxy = self.adapter_proxy()?;
        let object =
            ObjectPath::try_from(path.as_str()).map_err(|error| BtError::internal(format!("非法对象路径：{error}")))?;
        proxy.call::<_, _, ()>("RemoveDevice", &(object,)).map_err(dbus_error)
    }

    fn set_prompt_handler(&self, handler: Option<PromptHandler>) {
        if let Ok(mut slot) = self.prompt_handler.lock() {
            *slot = handler;
        }
    }
}

impl RfcommConnector for BlueZTransport {
    fn connect_channel(&self, device: &DeviceId, channel: u8) -> TransportResult<Box<dyn TransportStream>> {
        let stream = rfcomm_connect(device, channel)?;
        Ok(Box::new(stream))
    }

    fn connect_service(&self, device: &DeviceId, uuid: &str) -> TransportResult<Box<dyn TransportStream>> {
        self.connect_service_with_options(device, uuid, ServiceOptions::default())
    }

    fn connect_service_with_options(
        &self,
        device: &DeviceId,
        uuid: &str,
        options: ServiceOptions,
    ) -> TransportResult<Box<dyn TransportStream>> {
        let channel = self
            .find_service(device, uuid)?
            .ok_or_else(|| BtError::not_found(format!("设备 {device} 未声明服务 {uuid}")))?;
        let stream = rfcomm_connect_with(device, channel, options.protection, options.timeout)?;
        Ok(Box::new(stream))
    }

    fn find_service(&self, device: &DeviceId, uuid: &str) -> TransportResult<Option<u8>> {
        query_rfcomm_channel(device, uuid)
    }
}

impl RfcommService for BlueZTransport {
    fn start(&self, uuid: &str, handler: AcceptHandler) -> TransportResult<u8> {
        let handler_slot = Arc::new(Mutex::new(Some(handler)));
        let iface = ProfileIface {
            handler: handler_slot.clone(),
        };
        self.connection
            .object_server()
            .at(PROFILE_PATH, iface)
            .map_err(dbus_error)?;
        let manager =
            Proxy::new(&self.connection, "org.bluez", "/org/bluez", "org.bluez.ProfileManager1").map_err(dbus_error)?;
        let mut options: HashMap<String, OwnedValue> = HashMap::new();
        options.insert("Name".to_string(), OwnedValue::from(Str::from("BluetoothTransfer")));
        options.insert("Role".to_string(), OwnedValue::from(Str::from("server")));
        options.insert("RequireAuthentication".to_string(), OwnedValue::from(false));
        options.insert("RequireAuthorization".to_string(), OwnedValue::from(false));
        let profile_path =
            ObjectPath::try_from(PROFILE_PATH).map_err(|error| BtError::internal(format!("非法对象路径：{error}")))?;
        manager
            .call::<_, _, ()>("RegisterProfile", &(profile_path, uuid, options))
            .map_err(dbus_error)?;
        if let Ok(mut slot) = self.profile_uuid.lock() {
            *slot = Some(uuid.to_string());
        }
        if let Ok(mut slot) = self.profile_handler.lock() {
            *slot = Some(handler_slot);
        }
        Ok(0)
    }

    fn stop(&self) -> TransportResult<()> {
        let uuid = self.profile_uuid.lock().ok().and_then(|mut slot| slot.take());
        if let Some(uuid) = uuid {
            if let Ok(manager) = Proxy::new(&self.connection, "org.bluez", "/org/bluez", "org.bluez.ProfileManager1") {
                if let Ok(profile_path) = ObjectPath::try_from(PROFILE_PATH) {
                    let _: Result<(), zbus::Error> = manager.call("UnregisterProfile", &(profile_path,));
                }
            }
            let _ = uuid;
        }
        if let Ok(mut slot) = self.profile_handler.lock() {
            if let Some(handler) = slot.take() {
                if let Ok(mut inner) = handler.lock() {
                    *inner = None;
                }
            }
        }
        let _ = self.connection.object_server().remove::<ProfileIface, _>(PROFILE_PATH);
        Ok(())
    }

    fn is_listening(&self) -> bool {
        self.profile_uuid.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }

    fn channel(&self) -> Option<u8> {
        if self.is_listening() {
            Some(0)
        } else {
            None
        }
    }
}

fn find_adapter(connection: &Connection) -> Result<String, BtError> {
    let proxy = Proxy::new(connection, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager").map_err(dbus_error)?;
    let objects = proxy
        .call::<_, _, ManagedObjects>("GetManagedObjects", &())
        .map_err(dbus_error)?;
    let mut adapters: Vec<String> = objects
        .into_iter()
        .filter(|(_, interfaces)| interfaces.contains_key("org.bluez.Adapter1"))
        .map(|(path, _)| path.as_str().to_string())
        .collect();
    adapters.sort();
    adapters
        .into_iter()
        .next()
        .ok_or_else(|| BtError::unsupported("BlueZ 没有可用蓝牙适配器（请确认适配器已启用）"))
}

fn connect_with_timeout(fd: i32, address: &SockaddrRc, timeout: Duration) -> Result<i32, BtError> {
    // 安全性：fd 为有效 socket，address 为栈上 C 布局结构。
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 {
            return Err(io_error("读取 socket 标志失败"));
        }
        if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io_error("设置非阻塞失败"));
        }
        let result = libc::connect(
            fd,
            address as *const SockaddrRc as *const libc::sockaddr,
            std::mem::size_of::<SockaddrRc>() as libc::socklen_t,
        );
        if result == 0 {
            libc::fcntl(fd, libc::F_SETFL, flags);
            return Ok(0);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            libc::fcntl(fd, libc::F_SETFL, flags);
            return Ok(result);
        }
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let milliseconds = timeout.as_millis().min(u32::MAX as u128) as i32;
        let ready = libc::poll(&mut descriptor, 1, milliseconds);
        libc::fcntl(fd, libc::F_SETFL, flags);
        if ready == 0 {
            return Err(BtError::timeout("RFCOMM 连接超时"));
        }
        if ready < 0 {
            return Err(io_error("等待 RFCOMM 连接失败"));
        }
        let mut socket_error: i32 = 0;
        let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
        if libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            &mut socket_error as *mut i32 as *mut libc::c_void,
            &mut length,
        ) < 0
        {
            return Err(io_error("读取 RFCOMM 连接状态失败"));
        }
        if socket_error != 0 {
            return Ok(-1);
        }
        Ok(0)
    }
}

fn rfcomm_connect(device: &DeviceId, channel: u8) -> Result<FdStream, BtError> {
    rfcomm_connect_with(device, channel, ProtectionLevel::Auto, None)
}

fn rfcomm_connect_with(
    device: &DeviceId,
    channel: u8,
    protection: ProtectionLevel,
    timeout: Option<Duration>,
) -> Result<FdStream, BtError> {
    // 安全性：socket 系统调用无指针参数，返回值在下方检查。
    let fd = unsafe {
        libc::socket(
            libc::AF_BLUETOOTH,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            BTPROTO_RFCOMM,
        )
    };
    if fd < 0 {
        return Err(io_error("创建 RFCOMM socket 失败"));
    }
    if protection != ProtectionLevel::Auto {
        let level = match protection {
            ProtectionLevel::Plain => BT_SECURITY_LOW,
            ProtectionLevel::Encrypt => BT_SECURITY_HIGH,
            ProtectionLevel::Auto => BT_SECURITY_MEDIUM,
        };
        let security = BtSecurity { level, key_size: 0 };
        // 安全性：fd 是有效 socket，security 为栈上两字节配置结构。
        let result = unsafe {
            libc::setsockopt(
                fd,
                SOL_BLUETOOTH,
                BT_SECURITY,
                &security as *const BtSecurity as *const libc::c_void,
                std::mem::size_of::<BtSecurity>() as libc::socklen_t,
            )
        };
        if result != 0 {
            let error = io_error("设置 RFCOMM 保护级别失败");
            // 安全性：失败路径上 fd 尚未移交，立即关闭避免泄漏。
            unsafe {
                libc::close(fd);
            }
            return Err(error);
        }
    }
    let address = SockaddrRc {
        rc_family: libc::AF_BLUETOOTH as u16,
        rc_bdaddr: bdaddr(device),
        rc_channel: channel,
    };
    let result = if let Some(timeout) = timeout {
        connect_with_timeout(fd, &address, timeout)?
    } else {
        // 安全性：address 为按 C ABI 布局的栈变量，长度与类型一致。
        unsafe {
            libc::connect(
                fd,
                &address as *const SockaddrRc as *const libc::sockaddr,
                std::mem::size_of::<SockaddrRc>() as libc::socklen_t,
            )
        }
    };
    if result != 0 {
        let error = io_error("RFCOMM 连接失败");
        // 安全性：失败路径上 fd 尚未移交，立即关闭避免泄漏。
        unsafe {
            libc::close(fd);
        }
        return Err(error);
    }
    // 安全性：fd 是刚创建且连接成功的 socket，所有权唯一移交 OwnedFd。
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let stream = FdStream::from_fd(owned, Some(*device));
    set_fd_timeouts(fd, Some(RFCOMM_CONNECT_TIMEOUT), Some(RFCOMM_CONNECT_TIMEOUT))?;
    Ok(stream)
}

fn query_rfcomm_channel(device: &DeviceId, uuid: &str) -> Result<Option<u8>, BtError> {
    // 安全性：socket 系统调用无指针参数，返回值在下方检查。
    let fd = unsafe {
        libc::socket(
            libc::AF_BLUETOOTH,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            BTPROTO_L2CAP,
        )
    };
    if fd < 0 {
        return Err(io_error("创建 L2CAP socket 失败"));
    }
    let address = SockaddrL2 {
        l2_family: libc::AF_BLUETOOTH as u16,
        l2_psm: sdp::SDP_PSM.to_le(),
        l2_bdaddr: bdaddr(device),
        l2_cid: 0,
        l2_bdaddr_type: 0,
    };
    // 安全性：address 为按 C ABI 布局的栈变量，长度与类型一致。
    let result = unsafe {
        libc::connect(
            fd,
            &address as *const SockaddrL2 as *const libc::sockaddr,
            std::mem::size_of::<SockaddrL2>() as libc::socklen_t,
        )
    };
    if result != 0 {
        let error = io_error("连接 SDP 服务失败");
        // 安全性：失败路径上 fd 尚未移交，立即关闭避免泄漏。
        unsafe {
            libc::close(fd);
        }
        return Err(error);
    }
    // 安全性：fd 是刚创建且连接成功的 socket，所有权唯一移交 File。
    let mut file = unsafe { File::from_raw_fd(fd) };
    set_fd_timeouts(file.as_raw_fd(), Some(SDP_TIMEOUT), Some(SDP_TIMEOUT))?;
    let transaction = transaction_id();
    let request = sdp::service_search_attribute_request(uuid, transaction)?;
    file.write_all(&request)?;
    let mut response = vec![0u8; 4096];
    let mut filled = 0usize;
    loop {
        let read = file.read(&mut response[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
        if filled >= 5 {
            let parameter_length = u16::from_be_bytes([response[2], response[3]]) as usize;
            if filled >= 4 + parameter_length {
                break;
            }
        }
        if filled == response.len() {
            break;
        }
    }
    if filled < 5 {
        return Err(BtError::protocol("SDP 响应为空"));
    }
    sdp::parse_rfcomm_channel(&response[..filled], transaction)
}

fn transaction_id() -> u16 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.subsec_nanos() as u16)
        .unwrap_or(1)
}

fn device_id_from_path(path: &str) -> DeviceId {
    let fragment = path.rsplit('/').next().unwrap_or(path);
    let hex = fragment.strip_prefix("dev_").unwrap_or(fragment);
    let digits: String = hex.chars().filter(|ch| *ch != '_').collect();
    DeviceId::parse(&digits).unwrap_or_else(|_| DeviceId::from_bytes([0u8; 6]))
}

fn owned_string(map: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    map.get(key).and_then(|value| value.downcast_ref::<String>().ok())
}

fn owned_bool(map: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    map.get(key).and_then(|value| value.downcast_ref::<bool>().ok())
}

fn owned_i16(map: &HashMap<String, OwnedValue>, key: &str) -> Option<i16> {
    map.get(key).and_then(|value| value.downcast_ref::<i16>().ok())
}

fn dbus_error(error: zbus::Error) -> BtError {
    BtError::new(BtErrorKind::Io, format!("BlueZ D-Bus 调用失败：{error}"))
}

fn io_error(context: &str) -> BtError {
    BtError::new(
        BtErrorKind::Io,
        format!("{context}：{}", std::io::Error::last_os_error()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_address_is_reversed_for_sockaddr() {
        let device = DeviceId::parse("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(bdaddr(&device), [0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa]);
    }

    #[test]
    fn device_path_is_parsed() {
        let id = device_id_from_path("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_01");
        assert_eq!(id.to_string(), "AA:BB:CC:DD:EE:01");
        assert_eq!(device_id_from_path("garbage"), DeviceId::from_bytes([0u8; 6]));
    }

    #[test]
    fn property_helpers_tolerate_missing_values() {
        let map = HashMap::new();
        assert!(owned_string(&map, "Name").is_none());
        assert!(owned_bool(&map, "Paired").is_none());
        assert!(owned_i16(&map, "RSSI").is_none());
    }
}
