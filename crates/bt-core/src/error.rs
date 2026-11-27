use std::fmt;

/// 错误分类，供调用方做分支处理而不必解析文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BtErrorKind {
    InvalidArgument,
    NotFound,
    Unsupported,
    NotImplemented,
    Io,
    Protocol,
    Timeout,
    Canceled,
    Busy,
    Permission,
    Storage,
    Internal,
}

impl BtErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::NotFound => "not_found",
            Self::Unsupported => "unsupported",
            Self::NotImplemented => "not_implemented",
            Self::Io => "io",
            Self::Protocol => "protocol",
            Self::Timeout => "timeout",
            Self::Canceled => "canceled",
            Self::Busy => "busy",
            Self::Permission => "permission",
            Self::Storage => "storage",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for BtErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 跨 crate 统一错误：分类加可读说明，可跨线程传递。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind}: {message}")]
pub struct BtError {
    kind: BtErrorKind,
    message: String,
}

impl BtError {
    pub fn new(kind: BtErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub const fn kind(&self) -> BtErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::InvalidArgument, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::NotFound, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Unsupported, message)
    }

    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::NotImplemented, message)
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Protocol, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Timeout, message)
    }

    pub fn canceled(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Canceled, message)
    }

    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Busy, message)
    }

    pub fn permission(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Permission, message)
    }

    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Storage, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(BtErrorKind::Internal, message)
    }
}

impl From<std::io::Error> for BtError {
    fn from(error: std::io::Error) -> Self {
        Self::new(BtErrorKind::Io, error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_and_message_are_preserved() {
        let error = BtError::timeout("连接超时");
        assert_eq!(error.kind(), BtErrorKind::Timeout);
        assert_eq!(error.message(), "连接超时");
        assert!(error.to_string().contains("timeout"));
    }

    #[test]
    fn io_error_maps_to_io_kind() {
        let error: BtError = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "拒绝").into();
        assert_eq!(error.kind(), BtErrorKind::Io);
    }
}
