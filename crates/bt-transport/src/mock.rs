//! 可注入脚本的传输层替身，用于无硬件条件下的引擎与界面测试。

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use bt_core::{BtError, DeviceId};

use crate::{
    AcceptHandler, DiscoveredDevice, Discovery, Pairing, RfcommConnector, RfcommService, TransportResult,
    TransportStream,
};

/// 内存双向流：读取取自预置入站缓冲，写入累积到出站缓冲。
#[derive(Debug, Default)]
pub struct MockStream {
    inbound: VecDeque<u8>,
    outbound: Vec<u8>,
    peer: Option<DeviceId>,
    closed: bool,
}

impl MockStream {
    pub fn new(peer: Option<DeviceId>, inbound: Vec<u8>) -> Self {
        Self {
            inbound: inbound.into(),
            outbound: Vec::new(),
            peer,
            closed: false,
        }
    }

    pub fn written(&self) -> &[u8] {
        &self.outbound
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl Read for MockStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.inbound.is_empty() {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "入站缓冲已空"));
        }
        let count = buffer.len().min(self.inbound.len());
        for slot in buffer.iter_mut().take(count) {
            *slot = self.inbound.pop_front().expect("已确认非空");
        }
        Ok(count)
    }
}

impl Write for MockStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.closed {
            return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "流已关闭"));
        }
        self.outbound.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TransportStream for MockStream {
    fn set_timeouts(&mut self, _read: Option<Duration>, _write: Option<Duration>) -> TransportResult<()> {
        Ok(())
    }

    fn peer(&self) -> Option<DeviceId> {
        self.peer
    }

    fn close_stream(&mut self) -> TransportResult<()> {
        self.closed = true;
        Ok(())
    }
}

/// 传输层替身：可预置设备、记录连接请求、可让下一次连接失败。
#[derive(Default)]
pub struct MockTransport {
    devices: Mutex<Vec<DiscoveredDevice>>,
    connects: Mutex<Vec<(DeviceId, u8)>>,
    inbound: Mutex<Vec<u8>>,
    handler: Mutex<Option<AcceptHandler>>,
    service_uuid: Mutex<Option<String>>,
    channel: Mutex<Option<u8>>,
    fail_next_connect: AtomicBool,
    scan_count: AtomicUsize,
}

impl MockTransport {
    pub fn new(devices: Vec<DiscoveredDevice>) -> Self {
        Self {
            devices: Mutex::new(devices),
            ..Self::default()
        }
    }

    pub fn set_devices(&self, devices: Vec<DiscoveredDevice>) {
        if let Ok(mut current) = self.devices.lock() {
            *current = devices;
        }
    }

    pub fn set_inbound(&self, bytes: Vec<u8>) {
        if let Ok(mut current) = self.inbound.lock() {
            *current = bytes;
        }
    }

    pub fn fail_next_connect(&self) {
        self.fail_next_connect.store(true, Ordering::SeqCst);
    }

    pub fn connects(&self) -> Vec<(DeviceId, u8)> {
        self.connects
            .lock()
            .map(|connects| connects.clone())
            .unwrap_or_default()
    }

    pub fn scan_count(&self) -> usize {
        self.scan_count.load(Ordering::SeqCst)
    }

    pub fn service_uuid(&self) -> Option<String> {
        self.service_uuid.lock().ok().and_then(|uuid| uuid.clone())
    }
}

impl Discovery for MockTransport {
    fn start_scan(&self, _duration: Option<Duration>) -> TransportResult<()> {
        self.scan_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn cancel_scan(&self) -> TransportResult<()> {
        Ok(())
    }

    fn devices(&self) -> Vec<DiscoveredDevice> {
        self.devices.lock().map(|devices| devices.clone()).unwrap_or_default()
    }
}

impl Pairing for MockTransport {
    fn pair(&self, _device: &DeviceId) -> TransportResult<()> {
        Ok(())
    }

    fn answer(&self, _accept: bool) -> TransportResult<()> {
        Ok(())
    }

    fn unpair(&self, _device: &DeviceId) -> TransportResult<()> {
        Ok(())
    }
}

impl RfcommConnector for MockTransport {
    fn connect_channel(&self, device: &DeviceId, channel: u8) -> TransportResult<Box<dyn TransportStream>> {
        if self.fail_next_connect.swap(false, Ordering::SeqCst) {
            return Err(BtError::timeout("模拟连接失败"));
        }
        if let Ok(mut connects) = self.connects.lock() {
            connects.push((*device, channel));
        }
        let inbound = self.inbound.lock().map(|bytes| bytes.clone()).unwrap_or_default();
        Ok(Box::new(MockStream::new(Some(*device), inbound)))
    }

    fn connect_service(&self, device: &DeviceId, _uuid: &str) -> TransportResult<Box<dyn TransportStream>> {
        self.connect_channel(device, crate::OPP_FALLBACK_CHANNEL)
    }

    fn find_service(&self, _device: &DeviceId, _uuid: &str) -> TransportResult<Option<u8>> {
        Ok(Some(3))
    }
}

impl RfcommService for MockTransport {
    fn start(&self, uuid: &str, handler: AcceptHandler) -> TransportResult<u8> {
        if let Ok(mut slot) = self.handler.lock() {
            *slot = Some(handler);
        }
        if let Ok(mut slot) = self.service_uuid.lock() {
            *slot = Some(uuid.to_string());
        }
        if let Ok(mut slot) = self.channel.lock() {
            *slot = Some(3);
        }
        Ok(3)
    }

    fn stop(&self) -> TransportResult<()> {
        if let Ok(mut slot) = self.handler.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.channel.lock() {
            *slot = None;
        }
        Ok(())
    }

    fn is_listening(&self) -> bool {
        self.channel.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }

    fn channel(&self) -> Option<u8> {
        self.channel.lock().map(|slot| *slot).unwrap_or(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn device() -> DeviceId {
        DeviceId::parse("AA:BB:CC:DD:EE:03").unwrap()
    }

    #[test]
    fn connect_records_channel_and_returns_reusable_stream() {
        let transport = MockTransport::new(Vec::new());
        transport.set_inbound(b"pong".to_vec());
        let mut stream = transport.connect_channel(&device(), 10).unwrap();
        let mut buffer = [0u8; 4];
        assert_eq!(stream.read(&mut buffer).unwrap(), 4);
        assert_eq!(&buffer, b"pong");
        stream.write_all(b"ping").unwrap();
        assert_eq!(transport.connects(), vec![(device(), 10)]);
        assert_eq!(stream.peer(), Some(device()));
    }

    #[test]
    fn fail_next_connect_surfaces_timeout_once() {
        let transport = MockTransport::new(Vec::new());
        transport.fail_next_connect();
        assert!(transport.connect_channel(&device(), 10).is_err());
        assert!(transport.connect_channel(&device(), 10).is_ok());
    }

    #[test]
    fn service_start_records_uuid_and_channel() {
        let transport = MockTransport::new(Vec::new());
        let handler: AcceptHandler = Arc::new(|_stream: Box<dyn TransportStream>| {});
        assert!(!transport.is_listening());
        let channel = transport.start("uuid", handler).unwrap();
        assert_eq!(channel, 3);
        assert!(transport.is_listening());
        assert_eq!(transport.service_uuid().as_deref(), Some("uuid"));
        assert!(transport.devices().is_empty());
        transport.start_scan(None).unwrap();
        assert_eq!(transport.scan_count(), 1);
    }
}
