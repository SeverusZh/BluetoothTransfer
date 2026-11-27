//! 展示格式化：所有用户可见的状态文案集中在此，避免各页面各写一套。

use bt_api::{JobSnapshot, JobState, LogLevel};
use bt_core::{AssistantState, Channel, Direction, RecordKind, RecordStatus, SendSource};

pub fn bytes(value: u64) -> String {
    bt_core::format_bytes(value)
}

pub fn duration(seconds: u64) -> String {
    bt_core::format_duration(seconds)
}

pub fn speed(bps: u64) -> String {
    if bps == 0 {
        "待测速".to_string()
    } else {
        format!("{}/s", bt_core::format_bytes(bps))
    }
}

pub fn eta(job: &JobSnapshot) -> String {
    match job.eta_seconds {
        Some(seconds) if seconds > 0 && job.state.is_running() => format!("剩余 {}", duration(seconds)),
        _ => String::new(),
    }
}

pub fn job_state(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "排队中",
        JobState::Connecting => "连接中",
        JobState::Sending => "发送中",
        JobState::Retrying => "重试中",
        JobState::Paused => "已暂停",
        JobState::Completed => "已完成",
        JobState::Failed => "失败",
        JobState::Canceled => "已取消",
    }
}

pub fn channel(channel: Channel) -> &'static str {
    match channel {
        Channel::Opp => "OPP",
        Channel::Assistant => "助手",
    }
}

pub fn direction(direction: Direction) -> &'static str {
    match direction {
        Direction::Send => "发送",
        Direction::Receive => "接收",
    }
}

pub fn kind(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::File => "文件",
        RecordKind::MultiFile => "多文件",
        RecordKind::Folder => "文件夹",
        RecordKind::Text => "文本",
        RecordKind::Other => "其它",
    }
}

pub fn status(status: RecordStatus) -> &'static str {
    match status {
        RecordStatus::Running => "进行中",
        RecordStatus::Completed => "已完成",
        RecordStatus::Failed => "失败",
        RecordStatus::Canceled => "已取消",
        RecordStatus::Interrupted => "已中断",
    }
}

pub fn log_level(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "调试",
        LogLevel::Info => "信息",
        LogLevel::Warn => "警告",
        LogLevel::Error => "错误",
    }
}

pub fn assistant(state: AssistantState) -> &'static str {
    match state {
        AssistantState::Unknown => "助手未探测",
        AssistantState::Probing => "助手探测中",
        AssistantState::Available => "助手可用",
        AssistantState::Unavailable => "助手不可用",
    }
}

pub fn source_name(source: &SendSource) -> String {
    source.display_name()
}

/// `2026-11-27T12:34:56Z` 显示为 `2026-11-27 12:34:56`。
pub fn timestamp(value: &str) -> String {
    if value.len() >= 19 {
        value[..19].replace('T', " ")
    } else {
        value.to_string()
    }
}

pub fn percent(bytes_sent: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((bytes_sent.min(total) as f64 / total as f64) * 100.0).round() as u8
}

pub fn short_address(id: bt_core::DeviceId) -> String {
    id.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_and_sizes_are_human_readable() {
        assert_eq!(timestamp("2026-11-27T12:34:56Z"), "2026-11-27 12:34:56");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(percent(50, 200), 25);
        assert_eq!(percent(0, 0), 0);
        assert_eq!(channel(Channel::Assistant), "助手");
    }
}
