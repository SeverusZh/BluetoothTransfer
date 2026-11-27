use std::time::{Duration, Instant};

use bt_core::BtError;
use bt_transport::TransportStream;

use crate::cancel::CancelFlag;
use crate::client::ChunkSource;
use crate::md5::md5;

/// OBEX 请求操作码，bit7 置位表示最终包。
pub mod opcode {
    pub const CONNECT: u8 = 0x80;
    pub const DISCONNECT: u8 = 0x81;
    pub const PUT: u8 = 0x02;
    pub const PUT_FINAL: u8 = 0x82;
    pub const ABORT: u8 = 0x7f;
}

pub mod response {
    pub const OK: u8 = 0x20;
    pub const SUCCESS_FINAL: u8 = 0xa0;
    pub const CONTINUE: u8 = 0x90;
    pub const BAD_REQUEST: u8 = 0x40;
    pub const UNAUTHORIZED: u8 = 0x41;
    pub const FORBIDDEN: u8 = 0x43;
    pub const NOT_FOUND: u8 = 0x44;
    pub const NOT_ACCEPTABLE: u8 = 0x46;
    pub const REQUEST_TIMEOUT: u8 = 0x48;
    pub const CONFLICT: u8 = 0x49;
    pub const ENTITY_TOO_LARGE: u8 = 0x4d;
    pub const UNSUPPORTED_MEDIA_TYPE: u8 = 0x4f;
    pub const HTTP_FORBIDDEN: u8 = 0xc3;
    pub const SERVICE_UNAVAILABLE: u8 = 0xd3;
    pub const INTERNAL_SERVER_ERROR: u8 = 0xe0;

    pub fn is_success(code: u8) -> bool {
        matches!(code, OK | SUCCESS_FINAL | CONTINUE)
    }

    pub fn describe(code: u8) -> String {
        match code {
            OK => "OK (0x20)".into(),
            SUCCESS_FINAL => "Success Final (0xA0)".into(),
            CONTINUE => "Continue (0x90)".into(),
            BAD_REQUEST => "Bad Request (0x40)".into(),
            UNAUTHORIZED => "Unauthorized (0x41)".into(),
            FORBIDDEN => "Forbidden (0x43)".into(),
            NOT_FOUND => "Not Found (0x44)".into(),
            NOT_ACCEPTABLE => "Not Acceptable (0x46)".into(),
            REQUEST_TIMEOUT => "Request Time Out (0x48)".into(),
            CONFLICT => "Conflict (0x49)".into(),
            ENTITY_TOO_LARGE => "Requested Entity Too Large (0x4D)".into(),
            UNSUPPORTED_MEDIA_TYPE => "Unsupported Media Type (0x4F)".into(),
            HTTP_FORBIDDEN => "Forbidden (0xC3)".into(),
            SERVICE_UNAVAILABLE => "Service Unavailable (0xD3)".into(),
            INTERNAL_SERVER_ERROR => "Internal Server Error (0xE0)".into(),
            other => format!("0x{other:02X}"),
        }
    }
}

pub mod header_id {
    pub const NAME: u8 = 0x01;
    pub const DESCRIPTION: u8 = 0x05;
    pub const TYPE: u8 = 0x42;
    pub const TARGET: u8 = 0x46;
    pub const BODY: u8 = 0x48;
    pub const END_OF_BODY: u8 = 0x49;
    pub const WHO: u8 = 0x4a;
    pub const AUTH_CHALLENGE: u8 = 0x4c;
    pub const AUTH_RESPONSE: u8 = 0x4d;
    pub const OBJECT_CLASS: u8 = 0x4f;
    pub const LENGTH: u8 = 0xc3;
    pub const CONNECTION_ID: u8 = 0xcb;
}

/// OBEX 头：编码规则由头 ID 高两位决定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObexHeader {
    pub id: u8,
    pub value: Vec<u8>,
}

impl ObexHeader {
    pub fn new(id: u8, value: impl Into<Vec<u8>>) -> Self {
        Self {
            id,
            value: value.into(),
        }
    }

    /// Name 头：UTF-16BE，默认无 BOM，末尾 0x0000，超长按字符边界截断。
    pub fn name(name: &str, use_bom: bool) -> Self {
        let name = if name.is_empty() { "file" } else { name };
        let bom = if use_bom { 2 } else { 0 };
        let max_units = (250 - bom - 2) / 2;
        let mut units: Vec<u16> = name.encode_utf16().collect();
        if units.len() > max_units {
            units.truncate(max_units);
            if units.last().is_some_and(|unit| (0xd800..=0xdbff).contains(unit)) {
                units.pop();
            }
        }
        let mut value = Vec::with_capacity(bom + units.len() * 2 + 2);
        if use_bom {
            value.push(0xfe);
            value.push(0xff);
        }
        for unit in units {
            value.extend_from_slice(&unit.to_be_bytes());
        }
        value.push(0);
        value.push(0);
        Self::new(header_id::NAME, value)
    }

    pub fn type_header(mime: &str) -> Self {
        Self::new(header_id::TYPE, mime.as_bytes().to_vec())
    }

    pub fn length(size: u32) -> Self {
        Self::new(header_id::LENGTH, size.to_be_bytes().to_vec())
    }

    pub fn body(data: impl Into<Vec<u8>>, end: bool) -> Self {
        Self::new(if end { header_id::END_OF_BODY } else { header_id::BODY }, data.into())
    }

    pub fn connection_id(id: u32) -> Self {
        Self::new(header_id::CONNECTION_ID, id.to_be_bytes().to_vec())
    }

    pub fn auth_response(digest: &[u8; 16], nonce: &[u8; 16]) -> Self {
        let mut value = Vec::with_capacity(36);
        value.push(0x00);
        value.push(0x10);
        value.extend_from_slice(digest);
        value.push(0x02);
        value.push(0x10);
        value.extend_from_slice(nonce);
        Self::new(header_id::AUTH_RESPONSE, value)
    }

    pub fn as_u32(&self) -> Option<u32> {
        if self.value.len() != 4 {
            return None;
        }
        Some(u32::from_be_bytes([
            self.value[0],
            self.value[1],
            self.value[2],
            self.value[3],
        ]))
    }

    pub fn as_name(&self) -> Option<String> {
        let payload = &self.value;
        if payload.len() < 2 || payload.len() % 2 != 0 {
            return None;
        }
        let start = if payload.len() >= 2 && payload[0] == 0xfe && payload[1] == 0xff {
            2
        } else {
            0
        };
        let mut end = payload.len();
        while end >= start + 2 && payload[end - 1] == 0 && payload[end - 2] == 0 {
            end -= 2;
        }
        let units: Vec<u16> = payload[start..end]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).ok()
    }
}

/// OBEX 包：操作码、2 字节长度、可选的 CONNECT 固定字段、头序列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObexPacket {
    pub opcode: u8,
    pub include_connect_fields: bool,
    pub version: u8,
    pub flags: u8,
    pub max_packet_length: u16,
    pub headers: Vec<ObexHeader>,
}

impl ObexPacket {
    pub fn new(opcode: u8) -> Self {
        Self {
            opcode,
            include_connect_fields: false,
            version: 0x10,
            flags: 0,
            max_packet_length: u16::MAX,
            headers: Vec::new(),
        }
    }

    pub fn connect() -> Self {
        Self::new(opcode::CONNECT)
    }

    pub fn encode(&self) -> Result<Vec<u8>, BtError> {
        let mut body = Vec::new();
        if self.include_connect_fields || self.opcode == opcode::CONNECT {
            body.push(self.version);
            body.push(self.flags);
            body.extend_from_slice(&self.max_packet_length.to_be_bytes());
        }
        for header in &self.headers {
            encode_header(&mut body, header)?;
        }
        let total = body.len() + 3;
        if total > u16::MAX as usize {
            return Err(BtError::protocol("OBEX 包长度超出 65535 上限"));
        }
        let mut out = Vec::with_capacity(total);
        out.push(self.opcode);
        out.extend_from_slice(&(total as u16).to_be_bytes());
        out.extend_from_slice(&body);
        Ok(out)
    }

    pub fn decode(data: &[u8], connect_response: bool) -> Result<Self, BtError> {
        if data.len() < 3 {
            return Err(BtError::protocol("OBEX 包长度不足 3 字节"));
        }
        let length = u16::from_be_bytes([data[1], data[2]]) as usize;
        if length < 3 || length != data.len() {
            return Err(BtError::protocol(format!(
                "OBEX 长度字段({length})与实际数据({})不一致",
                data.len()
            )));
        }
        let mut packet = Self::new(data[0]);
        let mut pos = 3usize;
        if connect_response || packet.opcode == opcode::CONNECT {
            if length < 7 {
                return Err(BtError::protocol("CONNECT 包缺少固定字段"));
            }
            packet.version = data[pos];
            packet.flags = data[pos + 1];
            packet.max_packet_length = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);
            pos += 4;
        }
        while pos < length {
            let id = data[pos];
            pos += 1;
            if id & 0xc0 == 0xc0 {
                if pos + 4 > length {
                    return Err(BtError::protocol("4 字节头越界"));
                }
                packet.headers.push(ObexHeader::new(id, data[pos..pos + 4].to_vec()));
                pos += 4;
            } else if id & 0xc0 == 0x80 {
                if pos + 1 > length {
                    return Err(BtError::protocol("1 字节头越界"));
                }
                packet.headers.push(ObexHeader::new(id, data[pos..pos + 1].to_vec()));
                pos += 1;
            } else {
                if pos + 2 > length {
                    return Err(BtError::protocol("头长度字段越界"));
                }
                let header_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
                pos += 2;
                if header_len < 3 || pos + header_len - 3 > length {
                    return Err(BtError::protocol(format!("头长度({header_len})越界")));
                }
                let value_len = header_len - 3;
                packet
                    .headers
                    .push(ObexHeader::new(id, data[pos..pos + value_len].to_vec()));
                pos += value_len;
            }
        }
        Ok(packet)
    }

    pub fn get_connection_id(&self) -> Option<u32> {
        self.headers
            .iter()
            .find(|header| header.id == header_id::CONNECTION_ID)
            .and_then(ObexHeader::as_u32)
    }

    pub fn get_auth_challenge_nonce(&self) -> Option<[u8; 16]> {
        let header = self
            .headers
            .iter()
            .find(|header| header.id == header_id::AUTH_CHALLENGE)?;
        let value = &header.value;
        let mut pos = 0usize;
        while pos < value.len() {
            let tag = value[pos];
            pos += 1;
            match tag {
                0x00 => {
                    if pos + 1 + 16 > value.len() {
                        return None;
                    }
                    pos += 1;
                    let mut nonce = [0u8; 16];
                    nonce.copy_from_slice(&value[pos..pos + 16]);
                    return Some(nonce);
                }
                0x01 => pos += 1,
                0x02 => {
                    if pos + 1 > value.len() {
                        return None;
                    }
                    pos += 1 + value[pos] as usize;
                }
                _ => return None,
            }
        }
        None
    }
}

fn encode_header(out: &mut Vec<u8>, header: &ObexHeader) -> Result<(), BtError> {
    out.push(header.id);
    if header.id & 0xc0 == 0xc0 {
        if header.value.len() != 4 {
            return Err(BtError::protocol(format!(
                "4 字节头 0x{:02X} 值长度必须为 4",
                header.id
            )));
        }
        out.extend_from_slice(&header.value);
    } else if header.id & 0xc0 == 0x80 {
        if header.value.len() != 1 {
            return Err(BtError::protocol(format!(
                "1 字节头 0x{:02X} 值长度必须为 1",
                header.id
            )));
        }
        out.extend_from_slice(&header.value);
    } else {
        let length = header.value.len() + 3;
        if length > u16::MAX as usize {
            return Err(BtError::protocol("OBEX 头长度超出 65535 上限"));
        }
        out.extend_from_slice(&(length as u16).to_be_bytes());
        out.extend_from_slice(&header.value);
    }
    Ok(())
}

/// 认证摘要 = MD5(nonce ":" password)。
pub fn auth_digest(nonce: &[u8; 16], password: &str) -> [u8; 16] {
    let mut data = Vec::with_capacity(16 + 1 + password.len());
    data.extend_from_slice(nonce);
    data.push(b':');
    data.extend_from_slice(password.as_bytes());
    md5(&data)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObexPushResult {
    pub response_code: u8,
    pub bytes_sent: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ObexOptions {
    pub name_use_bom: bool,
}

/// OBEX 客户端：CONNECT（含认证挑战重试）→ 两阶段流式 PUT → DISCONNECT。
pub struct ObexClient<'a> {
    stream: &'a mut dyn TransportStream,
    options: ObexOptions,
    password: String,
    connection_id: Option<u32>,
    max_packet_length: u16,
}

impl<'a> ObexClient<'a> {
    pub fn new(stream: &'a mut dyn TransportStream, options: ObexOptions, password: impl Into<String>) -> Self {
        Self {
            stream,
            options,
            password: password.into(),
            connection_id: None,
            max_packet_length: u16::MAX,
        }
    }

    pub fn connect(&mut self, cancel: &CancelFlag, idle: Duration) -> Result<(), BtError> {
        let mut packet = ObexPacket::connect();
        packet.max_packet_length = u16::MAX;
        self.stream.write_all(&packet.encode()?)?;
        let mut response = read_response(self.stream, true, cancel, idle)?;

        if response.opcode == response::UNAUTHORIZED {
            let nonce = response
                .get_auth_challenge_nonce()
                .ok_or_else(|| BtError::permission("设备要求 OBEX 认证但未提供 Nonce"))?;
            if self.password.is_empty() {
                return Err(BtError::permission(
                    "设备要求 OBEX 认证但未提供密码（可通过配置 OPP 认证密码提供）",
                ));
            }
            let digest = auth_digest(&nonce, &self.password);
            let mut retry = ObexPacket::connect();
            retry.headers.push(ObexHeader::auth_response(&digest, &nonce));
            self.stream.write_all(&retry.encode()?)?;
            response = read_response(self.stream, true, cancel, idle)?;
            if response.opcode == response::UNAUTHORIZED {
                return Err(BtError::permission("OBEX 认证被拒绝：密码不正确或设备要求额外交互"));
            }
        }

        if response.opcode != response::OK && response.opcode != response::SUCCESS_FINAL {
            return Err(BtError::protocol(format!(
                "OBEX CONNECT 失败：{}",
                response::describe(response.opcode)
            )));
        }
        self.connection_id = response.get_connection_id();
        self.max_packet_length = self.max_packet_length.min(response.max_packet_length);
        if self.max_packet_length < 64 {
            return Err(BtError::protocol(format!(
                "对端 OBEX 最大包长过小（{}），无法传输",
                self.max_packet_length
            )));
        }
        Ok(())
    }

    /// 两阶段 PUT：首包只带 Name/Type/Length，随后按分片发送 Body，末包 PUT Final。
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        name: &str,
        mime: &str,
        total_length: u64,
        chunk_size: u32,
        source: &mut dyn ChunkSource,
        cancel: &CancelFlag,
        idle: Duration,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<ObexPushResult, BtError> {
        if total_length > u32::MAX as u64 {
            return Err(BtError::invalid("文件超过 OBEX 单对象 4GB 上限"));
        }
        if chunk_size == 0 {
            return Err(BtError::invalid("分片大小不能为 0"));
        }
        let mut header_packet = ObexPacket::new(opcode::PUT);
        self.add_common_headers(&mut header_packet);
        header_packet
            .headers
            .push(ObexHeader::name(name, self.options.name_use_bom));
        if !mime.is_empty() {
            header_packet.headers.push(ObexHeader::type_header(mime));
        }
        header_packet.headers.push(ObexHeader::length(total_length as u32));
        self.stream.write_all(&header_packet.encode()?)?;
        let header_response = read_response(self.stream, false, cancel, idle)?;
        if header_response.opcode != response::CONTINUE {
            if response::is_success(header_response.opcode) {
                return Ok(ObexPushResult {
                    response_code: header_response.opcode,
                    bytes_sent: 0,
                });
            }
            return Err(BtError::protocol(format!(
                "OBEX PUT 头包被拒绝：{}",
                response::describe(header_response.opcode)
            )));
        }

        if total_length == 0 {
            let mut empty_final = ObexPacket::new(opcode::PUT_FINAL);
            self.add_common_headers(&mut empty_final);
            empty_final.headers.push(ObexHeader::body(Vec::new(), true));
            self.stream.write_all(&empty_final.encode()?)?;
            let final_response = read_response(self.stream, false, cancel, idle)?;
            if !response::is_success(final_response.opcode) {
                return Err(BtError::protocol(format!(
                    "OBEX PUT 失败：{}",
                    response::describe(final_response.opcode)
                )));
            }
            return Ok(ObexPushResult {
                response_code: final_response.opcode,
                bytes_sent: 0,
            });
        }

        const HEADER_OVERHEAD: u16 = 512;
        let max_body = self.max_packet_length.saturating_sub(HEADER_OVERHEAD).max(16);
        let chunk = (chunk_size as u16).min(max_body) as usize;
        let mut buffer = vec![0u8; chunk];
        let mut offset = 0u64;
        let mut bytes_sent = 0u64;
        progress(0, total_length);

        while offset < total_length {
            if cancel.is_canceled() {
                return Err(BtError::canceled("用户取消"));
            }
            let read = source.read_at(offset, &mut buffer)?;
            if read == 0 {
                return Err(BtError::protocol("读取源数据失败（源不可读或已提前结束）"));
            }
            let is_last = offset + read as u64 >= total_length;
            let mut packet = ObexPacket::new(if is_last { opcode::PUT_FINAL } else { opcode::PUT });
            self.add_common_headers(&mut packet);
            packet.headers.push(ObexHeader::body(buffer[..read].to_vec(), is_last));
            self.stream.write_all(&packet.encode()?)?;
            offset += read as u64;
            bytes_sent += read as u64;
            let response = read_response(self.stream, false, cancel, idle)?;
            if is_last {
                if response::is_success(response.opcode) {
                    progress(total_length, total_length);
                    return Ok(ObexPushResult {
                        response_code: response.opcode,
                        bytes_sent,
                    });
                }
                return Err(BtError::protocol(format!(
                    "OBEX PUT 失败：{}",
                    response::describe(response.opcode)
                )));
            }
            if response.opcode == response::CONTINUE {
                progress(offset, total_length);
                continue;
            }
            if response::is_success(response.opcode) {
                progress(offset, total_length);
                return Ok(ObexPushResult {
                    response_code: response.opcode,
                    bytes_sent,
                });
            }
            return Err(BtError::protocol(format!(
                "OBEX PUT 失败：{}",
                response::describe(response.opcode)
            )));
        }
        Err(BtError::protocol("OBEX PUT 未完成（数据循环异常结束）"))
    }

    pub fn disconnect(&mut self, cancel: &CancelFlag, idle: Duration) -> Result<(), BtError> {
        let mut packet = ObexPacket::new(opcode::DISCONNECT);
        self.add_common_headers(&mut packet);
        self.stream.write_all(&packet.encode()?)?;
        let response = read_response(self.stream, false, cancel, idle)?;
        if response.opcode == response::OK || response.opcode == response::SUCCESS_FINAL {
            Ok(())
        } else {
            Err(BtError::protocol(format!(
                "OBEX DISCONNECT 响应异常：{}",
                response::describe(response.opcode)
            )))
        }
    }

    fn add_common_headers(&self, packet: &mut ObexPacket) {
        if let Some(id) = self.connection_id {
            packet.headers.push(ObexHeader::connection_id(id));
        }
    }
}

fn read_response(
    stream: &mut dyn TransportStream,
    connect_response: bool,
    cancel: &CancelFlag,
    idle: Duration,
) -> Result<ObexPacket, BtError> {
    let mut header = [0u8; 3];
    read_exact_poll(stream, &mut header, cancel, idle)?;
    let length = u16::from_be_bytes([header[1], header[2]]) as usize;
    if length < 3 {
        return Err(BtError::protocol(format!("非法 OBEX 响应长度：{length}")));
    }
    let mut body = vec![0u8; length - 3];
    read_exact_poll(stream, &mut body, cancel, idle)?;
    let mut full = Vec::with_capacity(length);
    full.extend_from_slice(&header);
    full.extend_from_slice(&body);
    let is_connect_ok = connect_response && (header[0] == response::OK || header[0] == response::SUCCESS_FINAL);
    match ObexPacket::decode(&full, is_connect_ok) {
        Ok(packet) => Ok(packet),
        Err(first) if connect_response => ObexPacket::decode(&full, !is_connect_ok).map_err(|_| {
            BtError::protocol(format!(
                "无法解析 OBEX 响应：{first}（原始字节：{}）",
                full.iter().map(|b| format!("{b:02X}")).collect::<String>()
            ))
        }),
        Err(error) => Err(error),
    }
}

fn read_exact_poll(
    stream: &mut dyn TransportStream,
    buffer: &mut [u8],
    cancel: &CancelFlag,
    idle: Duration,
) -> Result<(), BtError> {
    let deadline = Instant::now() + idle;
    let mut position = 0usize;
    while position < buffer.len() {
        if cancel.is_canceled() {
            return Err(BtError::canceled("用户取消"));
        }
        match std::io::Read::read(stream, &mut buffer[position..]) {
            Ok(0) => return Err(BtError::protocol("对端提前关闭连接（响应不完整）")),
            Ok(read) => position += read,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                if Instant::now() >= deadline {
                    return Err(BtError::timeout("等待 OBEX 响应超时"));
                }
            }
            Err(error) => return Err(BtError::protocol(format!("读取 OBEX 响应失败：{error}"))),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_transport::mock::MockStream;

    fn connect_ok() -> Vec<u8> {
        let mut packet = ObexPacket::new(response::OK);
        packet.include_connect_fields = true;
        packet.max_packet_length = u16::MAX;
        packet.encode().unwrap()
    }

    fn simple(opcode: u8) -> Vec<u8> {
        ObexPacket::new(opcode).encode().unwrap()
    }

    fn collect_packets(bytes: &[u8]) -> Vec<ObexPacket> {
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos + 3 <= bytes.len() {
            let length = u16::from_be_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize;
            assert!(length >= 3 && pos + length <= bytes.len());
            out.push(ObexPacket::decode(&bytes[pos..pos + length], false).unwrap());
            pos += length;
        }
        assert_eq!(pos, bytes.len());
        out
    }

    #[test]
    fn name_header_uses_utf16be_without_bom_and_null_terminator() {
        let header = ObexHeader::name("你好.txt", false);
        assert_eq!(
            header.value,
            vec![0x4f, 0x60, 0x59, 0x7d, 0x00, 0x2e, 0x00, 0x74, 0x00, 0x78, 0x00, 0x74, 0x00, 0x00]
        );
        assert_eq!(header.as_name().as_deref(), Some("你好.txt"));
    }

    #[test]
    fn name_header_can_include_bom_and_truncates_on_boundary() {
        let header = ObexHeader::name("a.bin", true);
        assert_eq!(&header.value[..2], &[0xfe, 0xff]);
        assert_eq!(header.as_name().as_deref(), Some("a.bin"));
        let long = format!("{}.txt", "汉".repeat(200));
        let header = ObexHeader::name(&long, false);
        assert!(header.value.len() <= 250);
        assert!(header.value.len() % 2 == 0);
        let decoded = header.as_name().unwrap();
        assert!(decoded.starts_with(&"汉".repeat(123)));
    }

    #[test]
    fn header_length_field_includes_id_and_length_bytes() {
        let packet = {
            let mut packet = ObexPacket::new(opcode::PUT);
            packet.headers.push(ObexHeader::name("a.txt", false));
            packet.headers.push(ObexHeader::length(1024));
            packet.encode().unwrap()
        };
        // 操作码 1 + 长度 2；Name 头：ID 1 + 长度字段 2 + 值 12；Length 头：ID 1 + 值 4
        assert_eq!(packet.len(), 3 + 3 + 12 + 5);
        assert_eq!(u16::from_be_bytes([packet[3 + 1], packet[3 + 2]]), 15);
        let parsed = ObexPacket::decode(&packet, false).unwrap();
        assert_eq!(parsed.headers[0].as_name().as_deref(), Some("a.txt"));
        assert_eq!(parsed.headers[1].as_u32(), Some(1024));
    }

    #[test]
    fn packet_round_trips_all_header_types() {
        let mut packet = ObexPacket::new(opcode::PUT_FINAL);
        packet.headers.push(ObexHeader::name("a.txt", false));
        packet.headers.push(ObexHeader::type_header("application/octet-stream"));
        packet.headers.push(ObexHeader::length(1024));
        packet.headers.push(ObexHeader::body(vec![1, 2, 3], true));
        packet.headers.push(ObexHeader::connection_id(0x11223344));
        let encoded = packet.encode().unwrap();
        let parsed = ObexPacket::decode(&encoded, false).unwrap();
        assert_eq!(parsed.opcode, opcode::PUT_FINAL);
        assert_eq!(parsed.headers.len(), 5);
        assert_eq!(parsed.get_connection_id(), Some(0x11223344));
    }

    #[test]
    fn rejects_malformed_packets() {
        assert!(ObexPacket::decode(&[], false).is_err());
        assert!(ObexPacket::decode(&[0x20, 0x00], false).is_err());
        assert!(ObexPacket::decode(&[0x20, 0x00, 0x02, 0x00], false).is_err());
        let mut packet = simple(response::OK);
        packet[1] = 0xff;
        assert!(ObexPacket::decode(&packet, false).is_err());
        let raw = [0x20, 0x00, 0x08, 0x48, 0x00, 0x40, 0x01, 0x02];
        assert!(ObexPacket::decode(&raw, false).is_err());
        let raw = [0x20, 0x00, 0x06, 0xc3, 0x00, 0x01];
        assert!(ObexPacket::decode(&raw, false).is_err());
    }

    #[test]
    fn auth_challenge_nonce_is_parsed() {
        let nonce: Vec<u8> = (0..16).collect();
        let mut value = vec![0x00, 0x10];
        value.extend_from_slice(&nonce);
        let mut packet = ObexPacket::new(response::UNAUTHORIZED);
        packet.headers.push(ObexHeader::new(header_id::AUTH_CHALLENGE, value));
        assert_eq!(packet.get_auth_challenge_nonce().unwrap().to_vec(), nonce);
    }

    #[test]
    fn auth_digest_is_md5_of_nonce_colon_password() {
        let nonce = [0x11u8; 16];
        let digest = auth_digest(&nonce, "pass");
        assert_eq!(md5(&[&nonce[..], b":pass"].concat()), digest);
    }

    #[test]
    fn connect_auth_retry_sends_second_connect() {
        let nonce = [7u8; 16];
        let mut challenge = vec![0x00, 0x10];
        challenge.extend_from_slice(&nonce);
        let mut unauthorized = ObexPacket::new(response::UNAUTHORIZED);
        unauthorized
            .headers
            .push(ObexHeader::new(header_id::AUTH_CHALLENGE, challenge));
        let mut inbound = unauthorized.encode().unwrap();
        inbound.extend_from_slice(&connect_ok());
        let mut stream = MockStream::new(None, inbound);
        {
            let mut client = ObexClient::new(&mut stream, ObexOptions::default(), "secret");
            client.connect(&CancelFlag::new(), Duration::from_secs(1)).unwrap();
        }
        let packets = collect_packets(&stream.written());
        assert_eq!(packets.len(), 2);
        assert!(packets[1]
            .headers
            .iter()
            .any(|header| header.id == header_id::AUTH_RESPONSE));
    }

    #[test]
    fn connect_without_password_reports_permission() {
        let nonce = [7u8; 16];
        let mut challenge = vec![0x00, 0x10];
        challenge.extend_from_slice(&nonce);
        let mut unauthorized = ObexPacket::new(response::UNAUTHORIZED);
        unauthorized
            .headers
            .push(ObexHeader::new(header_id::AUTH_CHALLENGE, challenge));
        let mut stream = MockStream::new(None, unauthorized.encode().unwrap());
        let mut client = ObexClient::new(&mut stream, ObexOptions::default(), "");
        let error = client.connect(&CancelFlag::new(), Duration::from_secs(1)).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::Permission);
    }

    #[test]
    fn two_phase_put_writes_expected_packet_sequence() {
        let data = vec![5u8; 100];
        let mut inbound = connect_ok();
        inbound.extend_from_slice(&simple(response::CONTINUE));
        inbound.extend_from_slice(&simple(response::OK));
        let mut stream = MockStream::new(None, inbound);
        let bytes = data.clone();
        {
            let mut client = ObexClient::new(&mut stream, ObexOptions::default(), "");
            client.connect(&CancelFlag::new(), Duration::from_secs(1)).unwrap();
            let mut source: &[u8] = &bytes;
            let result = client
                .push(
                    "small.bin",
                    "application/octet-stream",
                    data.len() as u64,
                    32768,
                    &mut source,
                    &CancelFlag::new(),
                    Duration::from_secs(1),
                    &mut |_, _| {},
                )
                .unwrap();
            assert_eq!(result.bytes_sent, 100);
        }
        let packets = collect_packets(&stream.written());
        assert_eq!(packets[0].opcode, opcode::CONNECT);
        assert_eq!(packets[1].opcode, opcode::PUT);
        assert!(packets[1].headers.iter().all(|header| header.id != header_id::BODY));
        assert!(packets[1].headers.iter().any(|header| header.id == header_id::LENGTH));
        assert_eq!(packets[2].opcode, opcode::PUT_FINAL);
        assert!(packets[2]
            .headers
            .iter()
            .any(|header| header.id == header_id::END_OF_BODY));
    }

    #[test]
    fn multi_chunk_put_uses_put_for_non_final_chunks() {
        let data = vec![3u8; 70_000];
        let mut inbound = connect_ok();
        inbound.extend_from_slice(&simple(response::CONTINUE));
        inbound.extend_from_slice(&simple(response::CONTINUE));
        inbound.extend_from_slice(&simple(response::OK));
        let mut stream = MockStream::new(None, inbound);
        let bytes = data.clone();
        {
            let mut client = ObexClient::new(&mut stream, ObexOptions::default(), "");
            client.connect(&CancelFlag::new(), Duration::from_secs(1)).unwrap();
            let mut source: &[u8] = &bytes;
            client
                .push(
                    "big.bin",
                    "",
                    70_000,
                    65_023,
                    &mut source,
                    &CancelFlag::new(),
                    Duration::from_secs(1),
                    &mut |_, _| {},
                )
                .unwrap();
        }
        let packets = collect_packets(&stream.written());
        assert_eq!(packets[2].opcode, opcode::PUT);
        assert_eq!(packets[3].opcode, opcode::PUT_FINAL);
        let body_len = packets[2]
            .headers
            .iter()
            .find(|header| header.id == header_id::BODY)
            .map(|header| header.value.len())
            .unwrap();
        assert_eq!(body_len, 65_023);
    }

    #[test]
    fn empty_file_sends_empty_end_of_body() {
        let mut inbound = connect_ok();
        inbound.extend_from_slice(&simple(response::CONTINUE));
        inbound.extend_from_slice(&simple(response::OK));
        let mut stream = MockStream::new(None, inbound);
        {
            let mut client = ObexClient::new(&mut stream, ObexOptions::default(), "");
            client.connect(&CancelFlag::new(), Duration::from_secs(1)).unwrap();
            let mut source: &[u8] = &[];
            let result = client
                .push(
                    "empty.bin",
                    "",
                    0,
                    32768,
                    &mut source,
                    &CancelFlag::new(),
                    Duration::from_secs(1),
                    &mut |_, _| {},
                )
                .unwrap();
            assert_eq!(result.bytes_sent, 0);
        }
        let packets = collect_packets(&stream.written());
        assert_eq!(packets[2].opcode, opcode::PUT_FINAL);
        let end = packets[2]
            .headers
            .iter()
            .find(|header| header.id == header_id::END_OF_BODY)
            .unwrap();
        assert!(end.value.is_empty());
    }

    #[test]
    fn forbidden_header_put_reports_code() {
        let mut inbound = connect_ok();
        inbound.extend_from_slice(&simple(response::FORBIDDEN));
        let mut stream = MockStream::new(None, inbound);
        let mut client = ObexClient::new(&mut stream, ObexOptions::default(), "");
        client.connect(&CancelFlag::new(), Duration::from_secs(1)).unwrap();
        let bytes = [1u8; 10];
        let mut source: &[u8] = &bytes;
        let error = client
            .push(
                "a.bin",
                "",
                10,
                32768,
                &mut source,
                &CancelFlag::new(),
                Duration::from_secs(1),
                &mut |_, _| {},
            )
            .unwrap_err();
        assert!(error.to_string().contains("头包被拒绝"));
    }

    #[test]
    fn success_final_is_accepted_as_put_result() {
        let mut inbound = connect_ok();
        inbound.extend_from_slice(&simple(response::CONTINUE));
        inbound.extend_from_slice(&simple(response::SUCCESS_FINAL));
        let mut stream = MockStream::new(None, inbound);
        let mut client = ObexClient::new(&mut stream, ObexOptions::default(), "");
        client.connect(&CancelFlag::new(), Duration::from_secs(1)).unwrap();
        let bytes = [1u8; 10];
        let mut source: &[u8] = &bytes;
        let result = client
            .push(
                "a.bin",
                "",
                10,
                32768,
                &mut source,
                &CancelFlag::new(),
                Duration::from_secs(1),
                &mut |_, _| {},
            )
            .unwrap();
        assert_eq!(result.response_code, response::SUCCESS_FINAL);
    }
}
