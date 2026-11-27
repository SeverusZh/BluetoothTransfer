//! Windows 平台适配：WinRT 负责发现、配对、服务广播与 RFCOMM 连接。
//!
//! 本轮只能在 Linux 上做 msvc 目标检查，实际蓝牙行为需在 Windows 真机验收。

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bt_core::{BtError, BtErrorKind, Channel, DeviceId, ProtectionLevel};
use windows::core::{Ref, GUID, HSTRING};
use windows::Devices::Bluetooth::BluetoothDevice;
use windows::Devices::Bluetooth::Rfcomm::{RfcommServiceId, RfcommServiceProvider};
use windows::Devices::Enumeration::{
    DeviceInformation, DeviceInformationUpdate, DevicePairingKinds, DevicePairingRequestedEventArgs,
    DevicePairingResultStatus, DeviceWatcher,
};
use windows::Foundation::TypedEventHandler;
use windows::Networking::Sockets::{SocketProtectionLevel, StreamSocket, StreamSocketListener};
use windows::Storage::Streams::{DataReader, DataWriter, InputStreamOptions};

use crate::{
    AcceptHandler, DiscoveredDevice, Discovery, PairAnswer, PairPrompt, PairPromptKind, Pairing, PromptHandler,
    RfcommConnector, RfcommService, ServiceOptions, TransportResult, TransportStream,
};

struct ScanSession {
    watcher: DeviceWatcher,
    tokens: Vec<i64>,
    cancel: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

struct ServiceSession {
    provider: RfcommServiceProvider,
    listener: StreamSocketListener,
    handler: AcceptHandler,
    uuid: String,
}

struct RfcommStream {
    socket: StreamSocket,
    reader: DataReader,
    writer: DataWriter,
    peer: Option<DeviceId>,
    read_timeout: Option<Duration>,
}

impl RfcommStream {
    fn new(socket: StreamSocket, peer: Option<DeviceId>) -> TransportResult<Self> {
        let input = socket.InputStream().map_err(winrt_error)?;
        let reader = DataReader::CreateDataReader(&input).map_err(winrt_error)?;
        reader
            .SetInputStreamOptions(InputStreamOptions::Partial)
            .map_err(winrt_error)?;
        let output = socket.OutputStream().map_err(winrt_error)?;
        let writer = DataWriter::CreateDataWriter(&output).map_err(winrt_error)?;
        Ok(Self {
            socket,
            reader,
            writer,
            peer,
            read_timeout: None,
        })
    }
}

impl Read for RfcommStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let loaded = self
            .reader
            .LoadAsync(buffer.len() as u32)
            .and_then(|operation| operation.join())
            .map_err(|error| std::io::Error::other(format!("WinRT 读取失败：{error}")))?;
        if loaded == 0 {
            return Ok(0);
        }
        let mut bytes = vec![0u8; loaded as usize];
        self.reader
            .ReadBytes(&mut bytes)
            .map_err(|error| std::io::Error::other(format!("WinRT 读取缓冲失败：{error}")))?;
        buffer[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}

impl Write for RfcommStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.writer
            .WriteBytes(buffer)
            .map_err(|error| std::io::Error::other(format!("WinRT 写入失败：{error}")))?;
        self.writer
            .StoreAsync()
            .and_then(|operation| operation.join())
            .map_err(|error| std::io::Error::other(format!("WinRT 提交写入失败：{error}")))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer
            .StoreAsync()
            .and_then(|operation| operation.join())
            .map(|_| ())
            .map_err(|error| std::io::Error::other(format!("WinRT 刷新失败：{error}")))
    }
}

impl TransportStream for RfcommStream {
    fn set_timeouts(&mut self, read: Option<Duration>, _write: Option<Duration>) -> TransportResult<()> {
        self.read_timeout = read;
        // WinRT StreamSocket 没有读超时 API；引擎依赖轮询取消，连接静默由上层空闲超时兜底。
        Ok(())
    }

    fn peer(&self) -> Option<DeviceId> {
        self.peer
    }

    fn close_stream(&mut self) -> TransportResult<()> {
        let _ = self.reader.Close();
        let _ = self.writer.Close();
        let _ = self.socket.Close();
        Ok(())
    }
}

impl Drop for RfcommStream {
    fn drop(&mut self) {
        let _ = self.close_stream();
    }
}

pub struct WinRtTransport {
    devices: Arc<Mutex<Vec<DiscoveredDevice>>>,
    scan: Mutex<Option<ScanSession>>,
    prompt_handler: Arc<Mutex<Option<PromptHandler>>>,
    service: Mutex<Option<ServiceSession>>,
}

impl std::fmt::Debug for WinRtTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WinRtTransport").finish_non_exhaustive()
    }
}

impl WinRtTransport {
    pub fn new() -> Result<Self, BtError> {
        Ok(Self {
            devices: Arc::new(Mutex::new(Vec::new())),
            scan: Mutex::new(None),
            prompt_handler: Arc::new(Mutex::new(None)),
            service: Mutex::new(None),
        })
    }

    fn snapshot(&self) -> Vec<DiscoveredDevice> {
        self.devices.lock().map(|devices| devices.clone()).unwrap_or_default()
    }

    fn stop_watcher(&self) {
        let session = self.scan.lock().ok().and_then(|mut scan| scan.take());
        if let Some(session) = session {
            session.cancel.store(true, Ordering::SeqCst);
            let _ = session.watcher.Stop();
            for token in &session.tokens {
                let _ = session.watcher.RemoveAdded(*token);
            }
            let _ = session.handle.join();
        }
    }

    fn device_for(&self, device: &DeviceId) -> Result<BluetoothDevice, BtError> {
        let address = address_to_u64(device);
        let found = BluetoothDevice::FromBluetoothAddressAsync(address)
            .map_err(winrt_error)?
            .join()
            .map_err(winrt_error)?;
        Ok(found)
    }

    fn service_id(uuid: &str) -> Result<RfcommServiceId, BtError> {
        let value = parse_uuid(uuid)?;
        RfcommServiceId::FromUuid(GUID::from_u128(value)).map_err(winrt_error)
    }
}

impl Discovery for WinRtTransport {
    fn start_scan(&self, duration: Option<Duration>) -> TransportResult<()> {
        self.stop_watcher();
        let devices = self.devices.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let selector = BluetoothDevice::GetDeviceSelector().map_err(winrt_error)?;
        let watcher = DeviceInformation::CreateWatcherAqsFilter(&selector).map_err(winrt_error)?;
        let added_store = devices.clone();
        let added = watcher
            .Added(&TypedEventHandler::new(move |_sender, info: Ref<DeviceInformation>| {
                if let Some(info) = info.as_ref() {
                    if let Some(device) = device_from_information(info) {
                        if let Ok(mut devices) = added_store.lock() {
                            upsert_device(&mut devices, device);
                        }
                    }
                }
                Ok(())
            }))
            .map_err(winrt_error)?;
        let updated = watcher
            .Updated(&TypedEventHandler::new(
                move |_sender, update: Ref<DeviceInformationUpdate>| {
                    if let Some(update) = update.as_ref() {
                        let _ = update.Id();
                    }
                    Ok(())
                },
            ))
            .map_err(winrt_error)?;
        let removed_store = devices.clone();
        let removed = watcher
            .Removed(&TypedEventHandler::new(
                move |_sender, update: Ref<DeviceInformationUpdate>| {
                    if let Some(update) = update.as_ref() {
                        if let Some(mac) = parse_mac_from_id(&update.Id().unwrap_or_default().to_string()) {
                            if let Ok(mut devices) = removed_store.lock() {
                                devices.retain(|device| device.id != mac);
                            }
                        }
                    }
                    Ok(())
                },
            ))
            .map_err(winrt_error)?;
        watcher.Start().map_err(winrt_error)?;
        let timer_watcher = watcher.clone();
        let timer_cancel = cancel.clone();
        let handle = std::thread::spawn(move || {
            let deadline = duration
                .map(|duration| std::time::Instant::now() + duration)
                .unwrap_or_else(|| std::time::Instant::now() + Duration::from_secs(10));
            while std::time::Instant::now() < deadline && !timer_cancel.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(200));
            }
            let _ = timer_watcher.Stop();
        });
        *self.scan.lock().map_err(|_| BtError::internal("扫描状态被投毒"))? = Some(ScanSession {
            watcher,
            tokens: vec![added, updated, removed],
            cancel,
            handle,
        });
        Ok(())
    }

    fn cancel_scan(&self) -> TransportResult<()> {
        self.stop_watcher();
        Ok(())
    }

    fn devices(&self) -> Vec<DiscoveredDevice> {
        self.snapshot()
    }
}

impl Pairing for WinRtTransport {
    fn pair(&self, device: &DeviceId) -> TransportResult<()> {
        let bluetooth = self.device_for(device)?;
        let information = bluetooth.DeviceInformation().map_err(winrt_error)?;
        let pairing = information.Pairing().map_err(winrt_error)?;
        let custom = pairing.Custom().map_err(winrt_error)?;
        let prompt_handler = self.prompt_handler.clone();
        let target = *device;
        let requested = custom
            .PairingRequested(&TypedEventHandler::new(
                move |_sender, args: Ref<DevicePairingRequestedEventArgs>| {
                    if let Some(args) = args.as_ref() {
                        handle_pairing_request(args, target, &prompt_handler);
                    }
                    Ok(())
                },
            ))
            .map_err(winrt_error)?;
        let kinds = DevicePairingKinds::ConfirmOnly | DevicePairingKinds::ProvidePin | DevicePairingKinds::DisplayPin;
        let result = custom
            .PairAsync(kinds)
            .map_err(winrt_error)?
            .join()
            .map_err(winrt_error)?;
        let _ = custom.RemovePairingRequested(requested);
        match result.Status().map_err(winrt_error)? {
            DevicePairingResultStatus::Paired | DevicePairingResultStatus::AlreadyPaired => Ok(()),
            other => Err(BtError::new(
                BtErrorKind::Permission,
                format!("WinRT 配对未成功：{other:?}"),
            )),
        }
    }

    fn answer(&self, _accept: bool) -> TransportResult<()> {
        Ok(())
    }

    fn unpair(&self, device: &DeviceId) -> TransportResult<()> {
        let bluetooth = self.device_for(device)?;
        let information = bluetooth.DeviceInformation().map_err(winrt_error)?;
        information
            .Pairing()
            .map_err(winrt_error)?
            .UnpairAsync()
            .map_err(winrt_error)?
            .join()
            .map_err(winrt_error)?;
        Ok(())
    }

    fn set_prompt_handler(&self, handler: Option<PromptHandler>) {
        if let Ok(mut slot) = self.prompt_handler.lock() {
            *slot = handler;
        }
    }
}

fn handle_pairing_request(
    args: &DevicePairingRequestedEventArgs,
    device: DeviceId,
    handler: &Arc<Mutex<Option<PromptHandler>>>,
) {
    let ask = |kind: PairPromptKind| {
        handler
            .lock()
            .ok()
            .and_then(|handler| handler.clone())
            .map(|handler| handler(PairPrompt { device, kind }))
            .unwrap_or(PairAnswer::Reject)
    };
    let Ok(pairing_kind) = args.PairingKind() else {
        return;
    };
    match pairing_kind {
        DevicePairingKinds::ConfirmOnly => {
            if ask(PairPromptKind::Confirm).accepted() {
                let _ = args.Accept();
            }
        }
        DevicePairingKinds::DisplayPin => {
            if let Ok(pin) = args.Pin() {
                let _ = ask(PairPromptKind::Passkey(parse_passkey(&pin.to_string())));
            }
            if ask(PairPromptKind::Confirm).accepted() {
                let _ = args.Accept();
            }
        }
        DevicePairingKinds::ProvidePin => match ask(PairPromptKind::Pin) {
            PairAnswer::Pin(pin) if !pin.is_empty() => {
                let _ = args.AcceptWithPin(&HSTRING::from(pin));
            }
            _ => {}
        },
        _ => {
            if ask(PairPromptKind::Confirm).accepted() {
                let _ = args.Accept();
            }
        }
    }
}

impl RfcommConnector for WinRtTransport {
    fn connect_channel(&self, device: &DeviceId, _channel: u8) -> TransportResult<Box<dyn TransportStream>> {
        let _ = device;
        Err(BtError::unsupported("WinRT 不暴露 RFCOMM 通道号，请按服务 UUID 连接"))
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
        let bluetooth = self.device_for(device)?;
        let service_id = Self::service_id(uuid)?;
        let services = bluetooth
            .GetRfcommServicesForIdAsync(&service_id)
            .map_err(winrt_error)?
            .join()
            .map_err(winrt_error)?;
        let services = services.Services().map_err(winrt_error)?;
        if services.Size().unwrap_or(0) == 0 {
            return Err(BtError::not_found(format!("设备 {device} 未声明服务 {uuid}")));
        }
        let service = services.GetAt(0).map_err(winrt_error)?;
        let protection = match options.protection {
            ProtectionLevel::Plain => SocketProtectionLevel::PlainSocket,
            ProtectionLevel::Encrypt => SocketProtectionLevel::BluetoothEncryptionWithAuthentication,
            ProtectionLevel::Auto => match service.ProtectionLevel() {
                Ok(level) if level != SocketProtectionLevel::PlainSocket => level,
                _ => SocketProtectionLevel::PlainSocket,
            },
        };
        let socket = self.open_socket(&service, protection, options.timeout)?;
        Ok(Box::new(RfcommStream::new(socket, Some(*device))?))
    }

    fn find_service(&self, device: &DeviceId, uuid: &str) -> TransportResult<Option<u8>> {
        let bluetooth = self.device_for(device)?;
        let service_id = Self::service_id(uuid)?;
        let services = bluetooth
            .GetRfcommServicesForIdAsync(&service_id)
            .map_err(winrt_error)?
            .join()
            .map_err(winrt_error)?;
        let services = services.Services().map_err(winrt_error)?;
        if services.Size().unwrap_or(0) == 0 {
            Ok(None)
        } else {
            Ok(Some(0))
        }
    }
}

impl WinRtTransport {
    fn open_socket(
        &self,
        service: &windows::Devices::Bluetooth::Rfcomm::RfcommDeviceService,
        protection: SocketProtectionLevel,
        timeout: Option<Duration>,
    ) -> Result<StreamSocket, BtError> {
        let socket = StreamSocket::new().map_err(winrt_error)?;
        let hostname = service.ConnectionHostName().map_err(winrt_error)?;
        let service_name = service.ConnectionServiceName().map_err(winrt_error)?;
        let Some(timeout) = timeout else {
            socket
                .ConnectWithProtectionLevelAsync(&hostname, &service_name, protection)
                .map_err(winrt_error)?
                .join()
                .map_err(winrt_error)?;
            return Ok(socket);
        };
        let worker_socket = socket.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = worker_socket
                .ConnectWithProtectionLevelAsync(&hostname, &service_name, protection)
                .map_err(winrt_error)
                .and_then(|operation| operation.join().map_err(winrt_error));
            let _ = sender.send(result);
        });
        match receiver.recv_timeout(timeout) {
            Ok(Ok(())) => Ok(socket),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(BtError::timeout("WinRT RFCOMM 连接超时")),
        }
    }
}

impl RfcommService for WinRtTransport {
    fn start(&self, uuid: &str, handler: AcceptHandler) -> TransportResult<u8> {
        let service_id = Self::service_id(uuid)?;
        let provider = RfcommServiceProvider::CreateAsync(&service_id)
            .map_err(winrt_error)?
            .join()
            .map_err(winrt_error)?;
        let listener = StreamSocketListener::new().map_err(winrt_error)?;
        let callback = handler.clone();
        listener
            .ConnectionReceived(&TypedEventHandler::new(
                move |_sender, args: Ref<
                    windows::Networking::Sockets::StreamSocketListenerConnectionReceivedEventArgs,
                >| {
                    if let Some(args) = args.as_ref() {
                        if let Ok(socket) = args.Socket() {
                            if let Ok(stream) = RfcommStream::new(socket, None) {
                                callback(Box::new(stream));
                            }
                        }
                    }
                    Ok(())
                },
            ))
            .map_err(winrt_error)?;
        provider.StartAdvertising(&listener).map_err(winrt_error)?;
        *self.service.lock().map_err(|_| BtError::internal("服务状态被投毒"))? = Some(ServiceSession {
            provider,
            listener,
            handler,
            uuid: uuid.to_string(),
        });
        Ok(0)
    }

    fn stop(&self) -> TransportResult<()> {
        let session = self.service.lock().ok().and_then(|mut service| service.take());
        if let Some(session) = session {
            let _ = session.provider.StopAdvertising();
            let _ = session.listener.Close();
            let _ = session.uuid;
            drop(session.handler);
        }
        Ok(())
    }

    fn is_listening(&self) -> bool {
        self.service.lock().map(|service| service.is_some()).unwrap_or(false)
    }

    fn channel(&self) -> Option<u8> {
        if self.is_listening() {
            Some(0)
        } else {
            None
        }
    }
}

fn device_from_information(info: &DeviceInformation) -> Option<DiscoveredDevice> {
    let id = info.Id().ok()?.to_string();
    let mac = parse_mac_from_id(&id)?;
    let name = info
        .Name()
        .ok()
        .map(|name| name.to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| mac.to_string());
    let paired = info
        .Pairing()
        .ok()
        .and_then(|pairing| pairing.IsPaired().ok())
        .unwrap_or(false);
    Some(DiscoveredDevice {
        id: mac,
        name,
        rssi: None,
        paired,
        connected: false,
        channels: vec![Channel::Opp],
    })
}

fn upsert_device(devices: &mut Vec<DiscoveredDevice>, device: DiscoveredDevice) {
    match devices.iter_mut().find(|existing| existing.id == device.id) {
        Some(existing) => *existing = device,
        None => devices.push(device),
    }
}

fn parse_mac_from_id(id: &str) -> Option<DeviceId> {
    let bytes = id.as_bytes();
    let mut start = None;
    for index in 0..bytes.len().saturating_sub(17) {
        let candidate = &id[index..index + 17];
        if is_mac(candidate) {
            start = Some(candidate);
            break;
        }
    }
    let text = start?;
    DeviceId::parse(text).ok()
}

fn is_mac(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 17 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        if index % 3 == 2 {
            if *byte != b':' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn address_to_u64(device: &DeviceId) -> u64 {
    let bytes = device.as_bytes();
    u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], 0, 0])
}

fn parse_uuid(text: &str) -> Result<u128, BtError> {
    let digits: String = text.chars().filter(|ch| *ch != '-').collect();
    if digits.len() != 32 {
        return Err(BtError::invalid(format!("UUID 长度非法 {text}")));
    }
    u128::from_str_radix(&digits, 16).map_err(|_| BtError::invalid(format!("UUID 解析失败 {text}")))
}

fn parse_passkey(pin: &str) -> u32 {
    pin.trim().parse().unwrap_or(0)
}

fn winrt_error(error: windows::core::Error) -> BtError {
    BtError::new(BtErrorKind::Io, format!("WinRT 调用失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_is_parsed_from_winrt_id() {
        let id = parse_mac_from_id("Bluetooth#BluetoothAA:BB:CC:DD:EE:FF-9F3C0E2A").unwrap();
        assert_eq!(id.to_string(), "AA:BB:CC:DD:EE:FF");
        assert!(parse_mac_from_id("Bluetooth#BluetoothNoMac").is_none());
    }

    #[test]
    fn uuid_and_address_helpers_round_trip() {
        const ASSISTANT_UUID: u128 = 0x9F3C0E2A_1B4D_4E6F_8A9B_0C1D2E3F4A5B;
        assert_eq!(
            parse_uuid("9F3C0E2A-1B4D-4E6F-8A9B-0C1D2E3F4A5B").unwrap(),
            ASSISTANT_UUID
        );
        let device = DeviceId::parse("AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(address_to_u64(&device), 0x0000_FFEE_DDCC_BBAA);
    }
}
