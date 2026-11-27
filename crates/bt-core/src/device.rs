use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::BtError;

/// 蓝牙设备地址，内部固定 6 字节，序列化为冒号分隔的大写形式。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId([u8; 6]);

impl DeviceId {
    pub const fn from_bytes(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 6] {
        &self.0
    }

    /// 解析 `AA:BB:CC:DD:EE:FF`、`aa-bb-cc-dd-ee-ff` 或 12 位连续十六进制。
    pub fn parse(value: &str) -> Result<Self, BtError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(BtError::invalid("蓝牙地址为空"));
        }
        let mut digits = [0u8; 12];
        let mut count = 0usize;
        for ch in trimmed.chars() {
            match ch {
                ':' | '-' | '.' | ' ' => continue,
                _ => {
                    let nibble = ch
                        .to_digit(16)
                        .ok_or_else(|| BtError::invalid(format!("蓝牙地址包含非法字符 {ch}")))?;
                    if count == digits.len() {
                        return Err(BtError::invalid(format!("蓝牙地址过长 {trimmed}")));
                    }
                    digits[count] = nibble as u8;
                    count += 1;
                }
            }
        }
        if count != digits.len() {
            return Err(BtError::invalid(format!("蓝牙地址长度不正确 {trimmed}")));
        }
        let mut bytes = [0u8; 6];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = digits[index * 2] << 4 | digits[index * 2 + 1];
        }
        Ok(Self(bytes))
    }

    /// 无分隔符的小写十六进制表示，用作记录键与文件名片段。
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(12);
        for byte in &self.0 {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(":")?;
            }
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({self})")
    }
}

impl FromStr for DeviceId {
    type Err = BtError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for DeviceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DeviceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        DeviceId::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colon_dash_and_compact_forms() {
        let expected = DeviceId::from_bytes([0xAA, 0xBB, 0xCC, 0x00, 0x11, 0x22]);
        assert_eq!(DeviceId::parse("AA:BB:CC:00:11:22").unwrap(), expected);
        assert_eq!(DeviceId::parse("aa-bb-cc-00-11-22").unwrap(), expected);
        assert_eq!(DeviceId::parse("aabbcc001122").unwrap(), expected);
    }

    #[test]
    fn rejects_wrong_length_and_bad_chars() {
        assert!(DeviceId::parse("AA:BB:CC").is_err());
        assert!(DeviceId::parse("").is_err());
        assert!(DeviceId::parse("ZZ:BB:CC:00:11:22").is_err());
    }

    #[test]
    fn display_round_trips_through_parse() {
        let original = DeviceId::parse("9f:3c:0e:2a:1b:4d").unwrap();
        assert_eq!(original.to_string(), "9F:3C:0E:2A:1B:4D");
        assert_eq!(DeviceId::parse(&original.to_string()).unwrap(), original);
        assert_eq!(original.to_hex(), "9f3c0e2a1b4d");
    }
}
