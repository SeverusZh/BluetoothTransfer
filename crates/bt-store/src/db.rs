use std::path::Path;
use std::sync::Mutex;

use bt_api::{DayStat, RecordQuery, Stats};
use bt_core::{
    parse_rfc3339, rfc3339_from_unix, unix_now, BtError, Channel, DeviceId, Direction, Record, RecordKind,
    RecordStatus, TransferId, MAX_RECORD_CONTENT_BYTES,
};
use rusqlite::{params_from_iter, types::Value, Connection};

use crate::SCHEMA_VERSION;

const RECORD_COLUMNS: &str = "id, direction, channel, kind, status, device_addr, device_name, display_name, path, checksum, content, byte_size, transferred, error, started_epoch, finished_epoch, resumable";

/// 设备别名与收藏的持久化形态，扫描结果与之合并。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRecord {
    pub id: DeviceId,
    pub name: String,
    pub alias: Option<String>,
    pub favorite: bool,
    pub last_seen: Option<String>,
    pub last_connected: Option<String>,
}

/// 记录库。内部串行化连接访问，对外保持 Send 与 Sync。
pub struct Store {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, BtError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path).map_err(storage_error)?;
        Self::from_connection(conn)
    }

    pub fn open_in_memory() -> Result<Self, BtError> {
        let conn = Connection::open_in_memory().map_err(storage_error)?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self, BtError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage_error)?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(storage_error)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(storage_error)?;
        conn.pragma_update(None, "foreign_keys", "ON").map_err(storage_error)?;
        let store = Self { conn: Mutex::new(conn) };
        store.apply_schema()?;
        Ok(store)
    }

    fn apply_schema(&self) -> Result<(), BtError> {
        let conn = self.lock()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS records (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 direction TEXT NOT NULL,
                 channel TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 status TEXT NOT NULL,
                 device_addr BLOB,
                 device_name TEXT NOT NULL DEFAULT '',
                 display_name TEXT NOT NULL DEFAULT '',
                 path TEXT,
                 checksum BLOB,
                 content TEXT,
                 byte_size INTEGER NOT NULL DEFAULT 0,
                 transferred INTEGER NOT NULL DEFAULT 0,
                 error TEXT,
                 started_epoch INTEGER NOT NULL,
                 finished_epoch INTEGER,
                 resumable INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_records_started ON records(started_epoch DESC);
             CREATE INDEX IF NOT EXISTS idx_records_device ON records(device_addr);
             CREATE INDEX IF NOT EXISTS idx_records_status ON records(status);
             CREATE TABLE IF NOT EXISTS devices (
                 addr BLOB PRIMARY KEY,
                 name TEXT NOT NULL DEFAULT '',
                 alias TEXT,
                 favorite INTEGER NOT NULL DEFAULT 0,
                 last_seen INTEGER,
                 last_connected INTEGER
             );",
        )
        .map_err(storage_error)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(storage_error)?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64, BtError> {
        let conn = self.lock()?;
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(storage_error)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, BtError> {
        self.conn.lock().map_err(|_| BtError::storage("记录库连接被投毒"))
    }

    pub fn insert_record(&self, record: &Record) -> Result<i64, BtError> {
        let conn = self.lock()?;
        insert_record_with(&conn, record)
    }

    /// 事务内批量插入，供迁移与大数据量测试使用。
    pub fn insert_records(&self, records: &[Record]) -> Result<usize, BtError> {
        if records.is_empty() {
            return Ok(0);
        }
        let mut conn = self.lock()?;
        let transaction = conn.transaction().map_err(storage_error)?;
        for record in records {
            insert_record_with(&transaction, record)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(records.len())
    }

    pub fn update_record(&self, record: &Record) -> Result<(), BtError> {
        let finished = record.finished_at.as_deref().map(epoch_of).transpose()?;
        let checksum = record.checksum.map(|digest| digest.to_vec());
        let content = record
            .content
            .as_deref()
            .filter(|text| text.len() <= MAX_RECORD_CONTENT_BYTES);
        let conn = self.lock()?;
        conn.execute(
            "UPDATE records SET status = ?1, byte_size = ?2, transferred = ?3, error = ?4, finished_epoch = ?5, path = ?6, resumable = ?7, checksum = ?8, content = ?9 WHERE id = ?10",
            rusqlite::params![
                record.status.as_str(),
                to_i64(record.byte_size)?,
                to_i64(record.transferred)?,
                record.error,
                finished,
                record.path.as_ref().map(|path| path.to_string_lossy().into_owned()),
                i64::from(record.resumable),
                checksum,
                content,
                record.id,
            ],
        )
        .map_err(storage_error)?;
        Ok(())
    }

    pub fn query_records(&self, query: &RecordQuery) -> Result<Vec<Record>, BtError> {
        let conn = self.lock()?;
        let (clauses, values) = build_filters(query)?;
        let mut sql = format!("SELECT {RECORD_COLUMNS} FROM records");
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY started_epoch DESC, id DESC");
        sql.push_str(&limit_clause(query));
        let mut statement = conn.prepare(&sql).map_err(storage_error)?;
        let rows = statement
            .query_map(params_from_iter(values.iter()), row_to_record)
            .map_err(storage_error)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.map_err(storage_error)?.into_record()?);
        }
        Ok(records)
    }

    /// 流式遍历匹配记录，不把结果集整体载入内存。
    pub fn stream_records(
        &self,
        query: &RecordQuery,
        mut sink: impl FnMut(&Record) -> Result<(), BtError>,
    ) -> Result<u64, BtError> {
        let conn = self.lock()?;
        let (clauses, values) = build_filters(query)?;
        let mut sql = format!("SELECT {RECORD_COLUMNS} FROM records");
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY started_epoch DESC, id DESC");
        sql.push_str(&limit_clause(query));
        let mut statement = conn.prepare(&sql).map_err(storage_error)?;
        let mut rows = statement
            .query(params_from_iter(values.iter()))
            .map_err(storage_error)?;
        let mut count = 0u64;
        while let Some(row) = rows.next().map_err(storage_error)? {
            let record = row_to_record(row).map_err(storage_error)?.into_record()?;
            sink(&record)?;
            count += 1;
        }
        Ok(count)
    }

    pub fn record_count(&self) -> Result<u64, BtError> {
        let conn = self.lock()?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM records", [], |row| row.get(0))
            .map_err(storage_error)?;
        Ok(count.max(0) as u64)
    }

    pub fn stats(&self) -> Result<Stats, BtError> {
        let conn = self.lock()?;
        let (total, sent, received, failed, bytes_sent, bytes_received, last_activity): (
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            Option<i64>,
        ) = conn
            .query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN direction = 'send' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN direction = 'receive' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN direction = 'send' THEN byte_size ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN direction = 'receive' THEN byte_size ELSE 0 END), 0),
                        MAX(started_epoch)
                 FROM records",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .map_err(storage_error)?;
        let mut stats = Stats {
            total: total.max(0) as u64,
            sent: sent.max(0) as u64,
            received: received.max(0) as u64,
            failed: failed.max(0) as u64,
            bytes_sent: bytes_sent.max(0) as u64,
            bytes_received: bytes_received.max(0) as u64,
            last_activity: last_activity.map(rfc3339_from_unix),
            days: Vec::new(),
        };
        let mut statement = conn
            .prepare(
                "SELECT date(started_epoch, 'unixepoch'), direction, COUNT(*), COALESCE(SUM(byte_size), 0)
                 FROM records GROUP BY 1, 2 ORDER BY 1 ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(storage_error)?;
        for row in rows {
            let (date, direction, count, bytes) = row.map_err(storage_error)?;
            let position = match stats.days.iter().position(|day| day.date == date) {
                Some(position) => position,
                None => {
                    stats.days.push(DayStat {
                        date,
                        ..DayStat::default()
                    });
                    stats.days.len() - 1
                }
            };
            let day = &mut stats.days[position];
            if direction == "receive" {
                day.received += count.max(0) as u64;
            } else {
                day.sent += count.max(0) as u64;
            }
            day.bytes += bytes.max(0) as u64;
        }
        Ok(stats)
    }

    pub fn delete_record(&self, id: i64) -> Result<bool, BtError> {
        let conn = self.lock()?;
        let affected = conn
            .execute("DELETE FROM records WHERE id = ?1", [id])
            .map_err(storage_error)?;
        Ok(affected > 0)
    }

    pub fn record_by_id(&self, id: i64) -> Result<Option<Record>, BtError> {
        let conn = self.lock()?;
        let mut statement = conn
            .prepare(&format!("SELECT {RECORD_COLUMNS} FROM records WHERE id = ?1"))
            .map_err(storage_error)?;
        let mut rows = statement.query([id]).map_err(storage_error)?;
        match rows.next().map_err(storage_error)? {
            Some(row) => Ok(Some(row_to_record(row).map_err(storage_error)?.into_record()?)),
            None => Ok(None),
        }
    }

    pub fn clear_records(&self) -> Result<u64, BtError> {
        let conn = self.lock()?;
        let affected = conn.execute("DELETE FROM records", []).map_err(storage_error)?;
        Ok(affected as u64)
    }

    pub fn upsert_device(&self, device: &DeviceRecord) -> Result<(), BtError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO devices (addr, name, alias, favorite, last_seen, last_connected)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(addr) DO UPDATE SET
                 name = excluded.name,
                 alias = COALESCE(excluded.alias, devices.alias),
                 favorite = CASE WHEN excluded.favorite != 0 THEN excluded.favorite ELSE devices.favorite END,
                 last_seen = COALESCE(excluded.last_seen, devices.last_seen),
                 last_connected = COALESCE(excluded.last_connected, devices.last_connected)",
            rusqlite::params![
                device.id.as_bytes().to_vec(),
                device.name,
                device.alias,
                i64::from(device.favorite),
                device.last_seen.as_deref().map(epoch_of).transpose()?,
                device.last_connected.as_deref().map(epoch_of).transpose()?,
            ],
        )
        .map_err(storage_error)?;
        Ok(())
    }

    pub fn set_device_alias(&self, id: &DeviceId, alias: Option<&str>) -> Result<bool, BtError> {
        let conn = self.lock()?;
        let affected = conn
            .execute(
                "UPDATE devices SET alias = ?1 WHERE addr = ?2",
                rusqlite::params![alias, id.as_bytes().to_vec()],
            )
            .map_err(storage_error)?;
        Ok(affected > 0)
    }

    pub fn set_device_favorite(&self, id: &DeviceId, favorite: bool) -> Result<bool, BtError> {
        let conn = self.lock()?;
        let affected = conn
            .execute(
                "UPDATE devices SET favorite = ?1 WHERE addr = ?2",
                rusqlite::params![i64::from(favorite), id.as_bytes().to_vec()],
            )
            .map_err(storage_error)?;
        Ok(affected > 0)
    }

    pub fn devices(&self) -> Result<Vec<DeviceRecord>, BtError> {
        let conn = self.lock()?;
        let mut statement = conn
            .prepare("SELECT addr, name, alias, favorite, last_seen, last_connected FROM devices ORDER BY favorite DESC, last_connected DESC")
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })
            .map_err(storage_error)?;
        let mut devices = Vec::new();
        for row in rows {
            let (addr, name, alias, favorite, last_seen, last_connected) = row.map_err(storage_error)?;
            if addr.len() != 6 {
                continue;
            }
            let mut bytes = [0u8; 6];
            bytes.copy_from_slice(&addr);
            devices.push(DeviceRecord {
                id: DeviceId::from_bytes(bytes),
                name,
                alias,
                favorite: favorite != 0,
                last_seen: last_seen.map(rfc3339_from_unix),
                last_connected: last_connected.map(rfc3339_from_unix),
            });
        }
        Ok(devices)
    }

    pub fn device_name(&self, id: &DeviceId) -> Result<Option<String>, BtError> {
        let conn = self.lock()?;
        let mut statement = conn
            .prepare("SELECT name FROM devices WHERE addr = ?1")
            .map_err(storage_error)?;
        let name = statement
            .query_row([id.as_bytes().to_vec()], |row| row.get::<_, String>(0))
            .ok();
        Ok(name)
    }
}

fn insert_record_with(conn: &Connection, record: &Record) -> Result<i64, BtError> {
    let started = epoch_of(&record.started_at)?;
    let finished = record.finished_at.as_deref().map(epoch_of).transpose()?;
    let content = record
        .content
        .as_deref()
        .filter(|text| text.len() <= MAX_RECORD_CONTENT_BYTES);
    let checksum = record.checksum.map(|digest| digest.to_vec());
    conn.execute(
        "INSERT INTO records (direction, channel, kind, status, device_addr, device_name, display_name, path, checksum, content, byte_size, transferred, error, started_epoch, finished_epoch, resumable) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        rusqlite::params![
            record.direction.as_str(),
            record.channel.as_str(),
            record.kind.as_str(),
            record.status.as_str(),
            record.device_id.map(|id| id.as_bytes().to_vec()),
            record.device_name,
            record.display_name,
            record.path.as_ref().map(|path| path.to_string_lossy().into_owned()),
            checksum,
            content,
            to_i64(record.byte_size)?,
            to_i64(record.transferred)?,
            record.error,
            started,
            finished,
            i64::from(record.resumable),
        ],
    )
    .map_err(storage_error)?;
    Ok(conn.last_insert_rowid())
}

fn limit_clause(query: &RecordQuery) -> String {
    match (query.limit, query.offset) {
        (Some(limit), Some(offset)) => format!(" LIMIT {limit} OFFSET {offset}"),
        (Some(limit), None) => format!(" LIMIT {limit}"),
        (None, Some(offset)) => format!(" LIMIT -1 OFFSET {offset}"),
        (None, None) => String::new(),
    }
}

fn to_i64(value: u64) -> Result<i64, BtError> {
    i64::try_from(value).map_err(|_| BtError::invalid(format!("数值超出 i64 范围 {value}")))
}

fn epoch_of(text: &str) -> Result<i64, BtError> {
    parse_rfc3339(text).ok_or_else(|| BtError::invalid(format!("时间格式非法 {text}")))
}

fn storage_error(error: rusqlite::Error) -> BtError {
    BtError::storage(format!("记录库错误：{error}"))
}

fn build_filters(query: &RecordQuery) -> Result<(Vec<String>, Vec<Value>), BtError> {
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(direction) = query.direction {
        clauses.push("direction = ?".to_string());
        values.push(Value::Text(direction.as_str().to_string()));
    }
    if let Some(channel) = query.channel {
        clauses.push("channel = ?".to_string());
        values.push(Value::Text(channel.as_str().to_string()));
    }
    if let Some(kind) = query.kind {
        clauses.push("kind = ?".to_string());
        values.push(Value::Text(kind.as_str().to_string()));
    }
    if let Some(status) = query.status {
        clauses.push("status = ?".to_string());
        values.push(Value::Text(status.as_str().to_string()));
    }
    if let Some(device) = query.device_id {
        clauses.push("device_addr = ?".to_string());
        values.push(Value::Blob(device.as_bytes().to_vec()));
    }
    if let Some(search) = query.search.as_deref().filter(|text| !text.is_empty()) {
        clauses.push("(display_name LIKE ? ESCAPE '\\' OR device_name LIKE ? ESCAPE '\\')".to_string());
        let pattern = like_pattern(search);
        values.push(Value::Text(pattern.clone()));
        values.push(Value::Text(pattern));
    }
    if let Some(from) = query.from.as_deref().filter(|text| !text.is_empty()) {
        clauses.push("started_epoch >= ?".to_string());
        values.push(Value::Integer(epoch_of(from)?));
    }
    if let Some(to) = query.to.as_deref().filter(|text| !text.is_empty()) {
        clauses.push("started_epoch <= ?".to_string());
        values.push(Value::Integer(epoch_of(to)?));
    }
    Ok((clauses, values))
}

fn like_pattern(search: &str) -> String {
    let mut pattern = String::from("%");
    for ch in search.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

struct RowData {
    id: i64,
    direction: String,
    channel: String,
    kind: String,
    status: String,
    device_addr: Option<Vec<u8>>,
    device_name: String,
    display_name: String,
    path: Option<String>,
    checksum: Option<Vec<u8>>,
    content: Option<String>,
    byte_size: i64,
    transferred: i64,
    error: Option<String>,
    started_epoch: i64,
    finished_epoch: Option<i64>,
    resumable: i64,
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RowData> {
    Ok(RowData {
        id: row.get(0)?,
        direction: row.get(1)?,
        channel: row.get(2)?,
        kind: row.get(3)?,
        status: row.get(4)?,
        device_addr: row.get(5)?,
        device_name: row.get(6)?,
        display_name: row.get(7)?,
        path: row.get(8)?,
        checksum: row.get(9)?,
        content: row.get(10)?,
        byte_size: row.get(11)?,
        transferred: row.get(12)?,
        error: row.get(13)?,
        started_epoch: row.get(14)?,
        finished_epoch: row.get(15)?,
        resumable: row.get(16)?,
    })
}

impl RowData {
    fn into_record(self) -> Result<Record, BtError> {
        let direction = match self.direction.as_str() {
            "send" => Direction::Send,
            "receive" => Direction::Receive,
            other => return Err(BtError::storage(format!("未知方向 {other}"))),
        };
        let channel = match self.channel.as_str() {
            "opp" => Channel::Opp,
            "assistant" => Channel::Assistant,
            other => return Err(BtError::storage(format!("未知通道 {other}"))),
        };
        let kind = match self.kind.as_str() {
            "file" => RecordKind::File,
            "multi_file" => RecordKind::MultiFile,
            "folder" => RecordKind::Folder,
            "text" => RecordKind::Text,
            "other" => RecordKind::Other,
            other => return Err(BtError::storage(format!("未知记录类型 {other}"))),
        };
        let status = match self.status.as_str() {
            "running" => RecordStatus::Running,
            "completed" => RecordStatus::Completed,
            "failed" => RecordStatus::Failed,
            "canceled" => RecordStatus::Canceled,
            "interrupted" => RecordStatus::Interrupted,
            other => return Err(BtError::storage(format!("未知记录状态 {other}"))),
        };
        let device_id = match self.device_addr.as_deref() {
            Some(bytes) if bytes.len() == 6 => {
                let mut id = [0u8; 6];
                id.copy_from_slice(bytes);
                Some(DeviceId::from_bytes(id))
            }
            _ => None,
        };
        let checksum: Option<TransferId> = self
            .checksum
            .as_deref()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
        Ok(Record {
            id: self.id,
            direction,
            channel,
            kind,
            status,
            device_id,
            device_name: self.device_name,
            display_name: self.display_name,
            path: self.path.map(std::path::PathBuf::from),
            checksum,
            content: self.content,
            byte_size: self.byte_size.max(0) as u64,
            transferred: self.transferred.max(0) as u64,
            error: self.error,
            started_at: rfc3339_from_unix(self.started_epoch),
            finished_at: self.finished_epoch.map(rfc3339_from_unix),
            resumable: self.resumable != 0,
        })
    }
}

/// 旧库时间戳归一化：接受带小数秒与时区偏移的 ISO-8601。
pub fn normalize_time(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    if trimmed.len() >= 19 {
        if let Some(position) = trimmed[19..].find(['+', '-']) {
            let mut epoch = parse_rfc3339(&format!("{}Z", &trimmed[..19]))?;
            let offset = &trimmed[19 + position..];
            let sign = if offset.starts_with('-') { -1 } else { 1 };
            let digits = &offset[1..];
            let parts = digits.split(':').take(2).collect::<Vec<_>>();
            if parts.len() == 2 {
                let hours: i64 = parts[0].parse().ok()?;
                let minutes: i64 = parts[1].parse().ok()?;
                epoch -= sign * (hours * 3600 + minutes * 60);
            }
            return Some(epoch);
        }
    }
    parse_rfc3339(trimmed)
}

/// 当前时间戳，迁移与兜底共用。
pub fn now_epoch() -> i64 {
    unix_now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_core::SendSource;
    use std::path::PathBuf;

    fn sample_record() -> Record {
        let mut record = Record::running(
            Direction::Send,
            Channel::Opp,
            RecordKind::File,
            "photo.jpg",
            "2026-01-02T03:04:05Z",
        );
        record.device_id = Some(DeviceId::parse("AA:BB:CC:DD:EE:04").unwrap());
        record.device_name = "Pixel".to_string();
        record.path = Some(PathBuf::from("/data/photo.jpg"));
        record.checksum = Some([9u8; 32]);
        record.byte_size = 1024;
        record.transferred = 1024;
        record.status = RecordStatus::Completed;
        record.finished_at = Some("2026-01-02T03:04:06Z".to_string());
        record
    }

    #[test]
    fn insert_query_and_delete_round_trip() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        let id = store.insert_record(&sample_record()).unwrap();
        assert!(id > 0);
        let records = store.query_records(&RecordQuery::default()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].display_name, "photo.jpg");
        assert_eq!(records[0].checksum, Some([9u8; 32]));
        assert_eq!(records[0].started_at, "2026-01-02T03:04:05Z");
        assert!(store.delete_record(id).unwrap());
        assert!(!store.delete_record(id).unwrap());
        assert!(store.query_records(&RecordQuery::default()).unwrap().is_empty());
    }

    #[test]
    fn query_filters_by_direction_search_and_time() {
        let store = Store::open_in_memory().unwrap();
        store.insert_record(&sample_record()).unwrap();
        let mut received = sample_record();
        received.direction = Direction::Receive;
        received.display_name = "note_100%.txt".to_string();
        received.started_at = "2026-02-01T00:00:00Z".to_string();
        store.insert_record(&received).unwrap();

        let sent = store
            .query_records(&RecordQuery {
                direction: Some(Direction::Send),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(sent.len(), 1);
        let searched = store
            .query_records(&RecordQuery {
                search: Some("note_100%".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(searched.len(), 1);
        let ranged = store
            .query_records(&RecordQuery {
                from: Some("2026-01-15T00:00:00Z".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(ranged.len(), 1);
        assert_eq!(ranged[0].display_name, "note_100%.txt");
    }

    #[test]
    fn stats_include_failed_records() {
        let store = Store::open_in_memory().unwrap();
        store.insert_record(&sample_record()).unwrap();
        let mut failed = sample_record();
        failed.direction = Direction::Receive;
        failed.status = RecordStatus::Failed;
        failed.error = Some("校验失败".into());
        failed.byte_size = 2048;
        store.insert_record(&failed).unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.total, 2);
        assert_eq!(stats.sent, 1);
        assert_eq!(stats.received, 1);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.bytes_sent, 1024);
        assert_eq!(stats.bytes_received, 2048);
        assert_eq!(stats.days.len(), 1);
        assert_eq!(stats.days[0].date, "2026-01-02");
        assert_eq!(stats.days[0].sent, 1);
        assert_eq!(stats.days[0].received, 1);
        assert!(stats.last_activity.is_some());
    }

    #[test]
    fn clear_records_counts_failed_rows() {
        let store = Store::open_in_memory().unwrap();
        store.insert_record(&sample_record()).unwrap();
        let mut failed = sample_record();
        failed.status = RecordStatus::Failed;
        failed.error = Some("对端断开".into());
        store.insert_record(&failed).unwrap();
        assert_eq!(store.clear_records().unwrap(), 2);
        assert_eq!(store.stats().unwrap().total, 0);
    }

    #[test]
    fn content_over_limit_is_not_stored() {
        let store = Store::open_in_memory().unwrap();
        let mut record = sample_record();
        record.content = Some("x".repeat(MAX_RECORD_CONTENT_BYTES + 1));
        store.insert_record(&record).unwrap();
        let stored = &store.query_records(&RecordQuery::default()).unwrap()[0];
        assert!(stored.content.is_none());

        let mut small = sample_record();
        small.content = Some("正文".to_string());
        store.insert_record(&small).unwrap();
        let stored = &store.query_records(&RecordQuery::default()).unwrap()[0];
        assert_eq!(stored.content.as_deref(), Some("正文"));
    }

    #[test]
    fn streaming_has_no_row_cap() {
        let store = Store::open_in_memory().unwrap();
        for index in 0..1200 {
            let mut record = sample_record();
            record.display_name = format!("f{index}.bin");
            store.insert_record(&record).unwrap();
        }
        let mut count = 0u64;
        let rows = store
            .stream_records(&RecordQuery::default(), |_record| {
                count += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(rows, 1200);
        assert_eq!(count, 1200);
        assert_eq!(store.query_records(&RecordQuery::latest(10)).unwrap().len(), 10);
    }

    #[test]
    fn device_alias_and_favorite_persist() {
        let store = Store::open_in_memory().unwrap();
        let id = DeviceId::parse("AA:BB:CC:00:11:22").unwrap();
        store
            .upsert_device(&DeviceRecord {
                id,
                name: "Pixel".into(),
                alias: None,
                favorite: false,
                last_seen: Some("2026-01-01T00:00:00Z".into()),
                last_connected: None,
            })
            .unwrap();
        assert!(store.set_device_alias(&id, Some("工作手机")).unwrap());
        assert!(store.set_device_favorite(&id, true).unwrap());
        let devices = store.devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].alias.as_deref(), Some("工作手机"));
        assert!(devices[0].favorite);
        assert_eq!(store.device_name(&id).unwrap().as_deref(), Some("Pixel"));
    }

    #[test]
    fn legacy_timestamp_normalization_handles_offsets() {
        assert_eq!(normalize_time("2026-01-02T03:04:05Z"), Some(1_767_323_045));
        assert_eq!(normalize_time("2026-01-02T11:04:05.1234567+08:00"), Some(1_767_323_045));
        assert_eq!(normalize_time("not-a-time"), None);
    }

    #[test]
    fn send_source_display_name_is_used_by_records() {
        let source = SendSource::Text("hello".into());
        assert_eq!(source.display_name(), "hello");
    }
}
