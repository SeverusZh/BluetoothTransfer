use bt_core::{hex_lower, parse_transfer_id, BtError, TransferId};

/// 单条 journal 记录的体积上限，验收按此断言。
pub const MAX_JOURNAL_ENTRY_BYTES: usize = 256;
pub const JOURNAL_PREFIX: &str = "bt1";

/// 断点续传索引条目，按行追加写入，崩溃后可据此恢复。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalEntry {
    pub job_id: u64,
    pub transfer: TransferId,
    pub offset: u64,
    pub total: u64,
    pub state: String,
    pub updated_at: String,
    pub target: String,
}

impl JournalEntry {
    pub fn encode(&self) -> Result<String, BtError> {
        if self.target.contains('|') {
            return Err(BtError::invalid("journal 目标路径不能包含竖线"));
        }
        let line = format!(
            "{JOURNAL_PREFIX}|{}|{}|{}|{}|{}|{}|{}",
            self.job_id,
            hex_lower(&self.transfer),
            self.offset,
            self.total,
            self.state,
            self.updated_at,
            self.target
        );
        if line.len() > MAX_JOURNAL_ENTRY_BYTES {
            return Err(BtError::invalid(format!(
                "journal 条目超出上限 {} > {MAX_JOURNAL_ENTRY_BYTES}",
                line.len()
            )));
        }
        Ok(line)
    }

    pub fn decode(line: &str) -> Result<Self, BtError> {
        let fields: Vec<&str> = line.trim().split('|').collect();
        if fields.len() != 8 {
            return Err(BtError::protocol("journal 字段数量不符"));
        }
        if fields[0] != JOURNAL_PREFIX {
            return Err(BtError::protocol("journal 前缀不匹配"));
        }
        Ok(Self {
            job_id: fields[1].parse().map_err(|_| BtError::protocol("job 编号非法"))?,
            transfer: parse_transfer_id(fields[2])?,
            offset: fields[3].parse().map_err(|_| BtError::protocol("偏移非法"))?,
            total: fields[4].parse().map_err(|_| BtError::protocol("总长非法"))?,
            state: fields[5].to_string(),
            updated_at: fields[6].to_string(),
            target: fields[7].to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> JournalEntry {
        JournalEntry {
            job_id: 42,
            transfer: [9u8; 32],
            offset: 262_144,
            total: 1_048_576,
            state: "sending".to_string(),
            updated_at: "2026-01-02T03:04:05Z".to_string(),
            target: "photo.jpg".to_string(),
        }
    }

    #[test]
    fn entry_round_trips_within_budget() {
        let line = entry().encode().unwrap();
        assert!(line.len() <= MAX_JOURNAL_ENTRY_BYTES);
        assert_eq!(JournalEntry::decode(&line).unwrap(), entry());
    }

    #[test]
    fn overlong_target_is_rejected() {
        let mut item = entry();
        item.target = "x".repeat(300);
        assert!(item.encode().is_err());
    }

    #[test]
    fn malformed_lines_are_rejected() {
        assert!(JournalEntry::decode("bt1|1").is_err());
        assert!(JournalEntry::decode("xx|1|2|3|4|5|6|7").is_err());
    }
}
