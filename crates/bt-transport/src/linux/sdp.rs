use bt_core::BtError;

/// SDP 服务发现使用的 L2CAP PSM。
pub const SDP_PSM: u16 = 0x0001;
pub const RFCOMM_PROTOCOL_UUID: u16 = 0x0003;

/// 解析 8-4-4-4-12 形式的 UUID 为 16 字节网络序。
pub fn parse_uuid(text: &str) -> Result<[u8; 16], BtError> {
    let digits: String = text.chars().filter(|ch| *ch != '-').collect();
    if digits.len() != 32 {
        return Err(BtError::invalid(format!("UUID 长度非法 {text}")));
    }
    let bytes = digits.as_bytes();
    let mut out = [0u8; 16];
    for (index, slot) in out.iter_mut().enumerate() {
        let high = hex_value(bytes[index * 2])?;
        let low = hex_value(bytes[index * 2 + 1])?;
        *slot = high << 4 | low;
    }
    Ok(out)
}

fn hex_value(byte: u8) -> Result<u8, BtError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(BtError::invalid("UUID 包含非十六进制字符")),
    }
}

/// ServiceSearchAttributeRequest：搜索指定 UUID 并取全部属性。
pub fn service_search_attribute_request(uuid: &str, transaction: u16) -> Result<Vec<u8>, BtError> {
    let uuid = parse_uuid(uuid)?;
    let mut pattern = Vec::with_capacity(19);
    pattern.push(0x35);
    pattern.push(17);
    pattern.push(0x1c);
    pattern.extend_from_slice(&uuid);
    let attribute_ids = [0x35, 0x05, 0x0a, 0x00, 0x00, 0xff, 0xff];
    let mut parameters = Vec::with_capacity(pattern.len() + attribute_ids.len() + 2);
    parameters.extend_from_slice(&pattern);
    parameters.extend_from_slice(&attribute_ids);
    parameters.extend_from_slice(&u16::MAX.to_be_bytes());
    let mut request = Vec::with_capacity(parameters.len() + 5);
    request.extend_from_slice(&transaction.to_be_bytes());
    request.extend_from_slice(&(parameters.len() as u16).to_be_bytes());
    request.extend_from_slice(&parameters);
    Ok(request)
}

/// 从 ServiceSearchAttributeResponse 中提取 RFCOMM 通道号。
pub fn parse_rfcomm_channel(response: &[u8], transaction: u16) -> Result<Option<u8>, BtError> {
    if response.len() < 5 {
        return Err(BtError::protocol("SDP 响应长度不足"));
    }
    let reply_transaction = u16::from_be_bytes([response[0], response[1]]);
    if reply_transaction != transaction {
        return Err(BtError::protocol("SDP 事务号不匹配"));
    }
    let parameter_length = u16::from_be_bytes([response[2], response[3]]) as usize;
    if response.len() < 4 + parameter_length {
        return Err(BtError::protocol("SDP 响应参数截断"));
    }
    let parameters = &response[4..4 + parameter_length];
    if parameters.len() < 3 {
        return Err(BtError::protocol("SDP 属性列表长度不足"));
    }
    let attribute_bytes = u16::from_be_bytes([parameters[0], parameters[1]]) as usize;
    if parameters.len() < 2 + attribute_bytes {
        return Err(BtError::protocol("SDP 属性列表截断"));
    }
    let mut reader = DeaReader::new(&parameters[2..2 + attribute_bytes]);
    let element = reader
        .read_element()?
        .ok_or_else(|| BtError::protocol("SDP 属性列表为空"))?;
    let Element::Sequence(attributes) = element else {
        return Err(BtError::protocol("SDP 属性列表不是序列"));
    };
    Ok(find_rfcomm_channel(&attributes))
}

fn find_rfcomm_channel(attributes: &[Element]) -> Option<u8> {
    for attribute in attributes {
        let Element::Sequence(pair) = attribute else {
            continue;
        };
        if pair.len() < 2 {
            continue;
        }
        let Some(Element::Uint(0x0004)) = pair.first() else {
            continue;
        };
        let Element::Sequence(protocols) = &pair[1] else {
            continue;
        };
        for protocol in protocols {
            let Element::Sequence(parts) = protocol else {
                continue;
            };
            if parts.len() < 2 {
                continue;
            }
            let is_rfcomm = match parts.first() {
                Some(Element::Uuid16(uuid)) => *uuid == RFCOMM_PROTOCOL_UUID,
                Some(Element::Uuid128(uuid)) => uuid[..12] == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3],
                _ => false,
            };
            if !is_rfcomm {
                continue;
            }
            if let Some(Element::Uint(channel)) = parts.get(1) {
                return Some(*channel as u8);
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Element {
    Uint(u64),
    Int(i64),
    Uuid16(u16),
    Uuid32(u32),
    Uuid128([u8; 16]),
    Text(Vec<u8>),
    Bool(bool),
    Sequence(Vec<Element>),
    Alternative(Vec<Element>),
    Other(u8, Vec<u8>),
}

pub struct DeaReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> DeaReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub fn read_element(&mut self) -> Result<Option<Element>, BtError> {
        if self.position >= self.data.len() {
            return Ok(None);
        }
        let header = self.data[self.position];
        self.position += 1;
        let type_descriptor = header >> 3;
        let size_descriptor = header & 0x07;
        let (length, length_bytes) = match size_descriptor {
            0 => (1usize, 0usize),
            1 => (2, 0),
            2 => (4, 0),
            3 => (8, 0),
            4 => (16, 0),
            5 => {
                if self.position + 1 > self.data.len() {
                    return Err(BtError::protocol("SDP 数据元素长度截断"));
                }
                let length = self.data[self.position] as usize;
                self.position += 1;
                (length, 1)
            }
            6 => {
                if self.position + 2 > self.data.len() {
                    return Err(BtError::protocol("SDP 数据元素长度截断"));
                }
                let length = u16::from_be_bytes([self.data[self.position], self.data[self.position + 1]]) as usize;
                self.position += 2;
                (length, 2)
            }
            _ => {
                if self.position + 4 > self.data.len() {
                    return Err(BtError::protocol("SDP 数据元素长度截断"));
                }
                let length = u32::from_be_bytes([
                    self.data[self.position],
                    self.data[self.position + 1],
                    self.data[self.position + 2],
                    self.data[self.position + 3],
                ]) as usize;
                self.position += 4;
                (length, 4)
            }
        };
        let _ = length_bytes;
        if self.position + length > self.data.len() {
            return Err(BtError::protocol("SDP 数据元素越界"));
        }
        let value = &self.data[self.position..self.position + length];
        self.position += length;
        let element = match type_descriptor {
            0 => Element::Other(0, Vec::new()),
            1 => Element::Uint(read_uint(value)),
            2 => Element::Int(read_uint(value) as i64),
            3 => match value.len() {
                2 => Element::Uuid16(u16::from_be_bytes([value[0], value[1]])),
                4 => Element::Uuid32(u32::from_be_bytes([value[0], value[1], value[2], value[3]])),
                16 => {
                    let mut uuid = [0u8; 16];
                    uuid.copy_from_slice(value);
                    Element::Uuid128(uuid)
                }
                _ => Element::Other(3, value.to_vec()),
            },
            4 => Element::Text(value.to_vec()),
            5 => Element::Bool(!value.is_empty() && value[0] != 0),
            6 => {
                let mut reader = DeaReader::new(value);
                let mut items = Vec::new();
                while let Some(item) = reader.read_element()? {
                    items.push(item);
                }
                Element::Sequence(items)
            }
            7 => {
                let mut reader = DeaReader::new(value);
                let mut items = Vec::new();
                while let Some(item) = reader.read_element()? {
                    items.push(item);
                }
                Element::Alternative(items)
            }
            other => Element::Other(other, value.to_vec()),
        };
        Ok(Some(element))
    }
}

fn read_uint(value: &[u8]) -> u64 {
    let mut out = 0u64;
    for byte in value {
        out = out << 8 | u64::from(*byte);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(children: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = children.iter().flatten().copied().collect();
        let mut out = vec![0x35, body.len() as u8];
        out.extend(body);
        out
    }

    fn uint8(value: u8) -> Vec<u8> {
        vec![0x08, value]
    }

    fn uint16(value: u16) -> Vec<u8> {
        let mut out = vec![0x09];
        out.extend_from_slice(&value.to_be_bytes());
        out
    }

    fn uuid16(value: u16) -> Vec<u8> {
        let mut out = vec![0x19];
        out.extend_from_slice(&value.to_be_bytes());
        out
    }

    #[test]
    fn builds_expected_request_bytes() {
        let request = service_search_attribute_request("9F3C0E2A-1B4D-4E6F-8A9B-0C1D2E3F4A5B", 7).unwrap();
        assert_eq!(&request[..2], &[0, 7]);
        let parameter_length = u16::from_be_bytes([request[2], request[3]]) as usize;
        assert_eq!(parameter_length + 4, request.len());
        assert_eq!(&request[4..7], &[0x35, 17, 0x1c]);
        assert_eq!(
            &request[7..23],
            &[0x9f, 0x3c, 0x0e, 0x2a, 0x1b, 0x4d, 0x4e, 0x6f, 0x8a, 0x9b, 0x0c, 0x1d, 0x2e, 0x3f, 0x4a, 0x5b]
        );
        assert_eq!(
            &request[request.len() - 9..],
            &[0x35, 0x05, 0x0a, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff]
        );
    }

    #[test]
    fn parses_rfcomm_channel_from_attribute_list() {
        let protocol = seq(&[uuid16(RFCOMM_PROTOCOL_UUID), uint8(10)]);
        let pdl = seq(&[protocol]);
        let attribute = seq(&[uint16(0x0004), pdl]);
        let attributes = seq(&[attribute]);
        let mut parameters = Vec::new();
        parameters.extend_from_slice(&(attributes.len() as u16).to_be_bytes());
        parameters.extend_from_slice(&attributes);
        // 属性字节数之后是 ContinuationState 长度 0（允许缺省）。
        let mut response = Vec::new();
        response.extend_from_slice(&1u16.to_be_bytes());
        response.extend_from_slice(&(parameters.len() as u16).to_be_bytes());
        response.extend_from_slice(&parameters);
        assert_eq!(parse_rfcomm_channel(&response, 1).unwrap(), Some(10));
    }

    #[test]
    fn rejects_bad_transaction_and_truncation() {
        let response = [0u8, 2, 0, 0];
        assert!(parse_rfcomm_channel(&response, 2).is_err());
        assert!(parse_rfcomm_channel(&[0, 1, 0, 9], 1).is_err());
    }

    #[test]
    fn parses_128_bit_uuid_and_rejects_garbage() {
        assert_eq!(
            parse_uuid("00001105-0000-1000-8000-00805F9B34FB").unwrap()[..4],
            [0, 0, 0x11, 0x05]
        );
        assert!(parse_uuid("xyz").is_err());
    }
}
