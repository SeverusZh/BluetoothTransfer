//! Linux 平台适配器骨架：BlueZ D-Bus 负责发现与配对，AF_BLUETOOTH RFCOMM 负责连接与监听。

use std::time::Duration;

use bt_core::{BtError, DeviceId};

use crate::{
    AcceptHandler, DiscoveredDevice, Discovery, Pairing, RfcommConnector, RfcommService, TransportResult,
    TransportStream,
};

#[derive(Debug, Default)]
pub struct BlueZTransport;

impl BlueZTransport {
    pub fn new() -> Self {
        Self
    }
}

impl Discovery for BlueZTransport {
    fn start_scan(&self, _duration: Option<Duration>) -> TransportResult<()> {
        Err(BtError::not_implemented("BlueZ 设备发现待实现"))
    }

    fn cancel_scan(&self) -> TransportResult<()> {
        Err(BtError::not_implemented("BlueZ 取消扫描待实现"))
    }

    fn devices(&self) -> Vec<DiscoveredDevice> {
        Vec::new()
    }
}

impl Pairing for BlueZTransport {
    fn pair(&self, _device: &DeviceId) -> TransportResult<()> {
        Err(BtError::not_implemented("BlueZ 配对待实现"))
    }

    fn answer(&self, _accept: bool) -> TransportResult<()> {
        Err(BtError::not_implemented("BlueZ 配对应答待实现"))
    }

    fn unpair(&self, _device: &DeviceId) -> TransportResult<()> {
        Err(BtError::not_implemented("BlueZ 取消配对待实现"))
    }
}

impl RfcommConnector for BlueZTransport {
    fn connect_channel(&self, _device: &DeviceId, _channel: u8) -> TransportResult<Box<dyn TransportStream>> {
        Err(BtError::not_implemented("BlueZ RFCOMM 直连待实现"))
    }

    fn connect_service(&self, _device: &DeviceId, _uuid: &str) -> TransportResult<Box<dyn TransportStream>> {
        Err(BtError::not_implemented("BlueZ SDP 连接待实现"))
    }

    fn find_service(&self, _device: &DeviceId, _uuid: &str) -> TransportResult<Option<u8>> {
        Err(BtError::not_implemented("BlueZ SDP 查询待实现"))
    }
}

impl RfcommService for BlueZTransport {
    fn start(&self, _uuid: &str, _handler: AcceptHandler) -> TransportResult<u8> {
        Err(BtError::not_implemented("BlueZ RFCOMM 服务待实现"))
    }

    fn stop(&self) -> TransportResult<()> {
        Err(BtError::not_implemented("BlueZ RFCOMM 服务停止待实现"))
    }

    fn is_listening(&self) -> bool {
        false
    }

    fn channel(&self) -> Option<u8> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unimplemented_paths_report_not_implemented() {
        let transport = BlueZTransport::new();
        let error = transport.start_scan(None).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::NotImplemented);

        assert!(transport.devices().is_empty());
        assert!(!transport.is_listening());
    }
}
