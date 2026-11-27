//! 蓝牙传输抽象层。
//!
//! Linux 走 BlueZ D-Bus 加 AF_BLUETOOTH RFCOMM，Windows 走 WinRT RFCOMM 与配对代理；
//! 两侧共用本 crate 的 trait，引擎与上层不感知平台差异。

pub mod mock;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "windows")]
pub mod windows;

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use bt_core::{BtError, Channel, DeviceId};

pub type TransportResult<T> = Result<T, BtError>;

/// 服务端接受连接后的回调类型。
pub type AcceptHandler = Arc<dyn Fn(Box<dyn TransportStream>) + Send + Sync>;

/// 扫描结果条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDevice {
    pub id: DeviceId,
    pub name: String,
    pub rssi: Option<i16>,
    pub paired: bool,
    pub connected: bool,
    pub channels: Vec<Channel>,
}

/// 已建立的 RFCOMM 字节流，带超时与关闭语义。
pub trait TransportStream: Read + Write + Send {
    fn set_timeouts(&mut self, read: Option<Duration>, write: Option<Duration>) -> TransportResult<()>;

    fn peer(&self) -> Option<DeviceId>;

    fn close_stream(&mut self) -> TransportResult<()>;
}

/// 设备发现。
pub trait Discovery: Send + Sync {
    fn start_scan(&self, duration: Option<Duration>) -> TransportResult<()>;

    fn cancel_scan(&self) -> TransportResult<()>;

    fn devices(&self) -> Vec<DiscoveredDevice>;
}

/// 配对与配对交互应答。
pub trait Pairing: Send + Sync {
    fn pair(&self, device: &DeviceId) -> TransportResult<()>;

    /// 应答当前挂起的配对交互。
    fn answer(&self, accept: bool) -> TransportResult<()>;

    fn unpair(&self, device: &DeviceId) -> TransportResult<()>;
}

/// RFCOMM 客户端连接。
pub trait RfcommConnector: Send + Sync {
    fn connect_channel(&self, device: &DeviceId, channel: u8) -> TransportResult<Box<dyn TransportStream>>;

    /// 按 SDP 服务 UUID 连接，跨平台推荐路径。
    fn connect_service(&self, device: &DeviceId, uuid: &str) -> TransportResult<Box<dyn TransportStream>>;

    /// 查询服务对应的 RFCOMM 通道号。
    fn find_service(&self, device: &DeviceId, uuid: &str) -> TransportResult<Option<u8>>;
}

/// RFCOMM 服务端广播。
pub trait RfcommService: Send + Sync {
    /// 开始监听并返回实际通道号。
    fn start(&self, uuid: &str, handler: AcceptHandler) -> TransportResult<u8>;

    fn stop(&self) -> TransportResult<()>;

    fn is_listening(&self) -> bool;

    fn channel(&self) -> Option<u8>;
}

/// 平台适配器需要同时具备四类能力。
pub trait BluetoothTransport: Discovery + Pairing + RfcommConnector + RfcommService {}

impl<T> BluetoothTransport for T where T: Discovery + Pairing + RfcommConnector + RfcommService {}

/// 标准 OPP 服务的常用 RFCOMM 通道号，仅作 SDP 不可用时的回退。
pub const OPP_FALLBACK_CHANNEL: u8 = 10;
