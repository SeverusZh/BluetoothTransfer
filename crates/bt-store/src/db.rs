use std::path::Path;
use std::sync::Mutex;

use bt_api::{DayStat, RecordQuery, Stats};
use bt_core::{BtError, Channel, DeviceId, Direction, Record, RecordKind, RecordStatus};
use rusqlite::{params_from_iter, types::Value, Connection};

use crate::SCHEMA_VERSION;

const RECORD_COLUMNS: &str =
    "id, direction, channel, kind, status, device_id, device_name, display_name, path, byte_size, transferred, error, started_at, finished_at, resumable";

/// 记录库。内部串行化连接访问，对外保持 Send 与 Sync。
pub struct Store {
    conn: Mutex<Connection>,
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
        conn.pragma_update(None, "journal_mode", "WAL").map_err(storage_error)?;
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
                 device_id TEXT,
                 device_name TEXT NOT NULL DEFAULT '',
                 display_name TEXT NOT NULL DEFAULT '',
                 path TEXT,
                 byte_size INTEGER NOT NULL DEFAULT 0,
                 transferred INTEGER NOT NULL DEFAULT 0,
                 error TEXT,
                 started_at TEXT NOT NULL,
                 finished_at TEXT,
                 resumable INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_records_started ON records(started_at DESC);
             CREATE INDEX IF NOT EXISTS idx_records_device ON records(device_id);
             CREATE INDEX IF NOT EXISTS idx_records_status ON records(status);",
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
        conn.execute(
            "INSERT INTO records (direction, channel, kind, status, device_id, device_name, display_name, path, byte_size, transferred, error, started_at, finished_at, resumable) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                record.direction.as_str(),
                record.channel.as_str(),
                record.kind.as_str(),
                record.status.as_str(),
                record.device_id.map(|id| id.to_string()),
                record.device_name,
                record.display_name,
                record.path.as_ref().map(|path| path.to_string_lossy().into_owned()),
                record.byte_size as i64,
                record.transferred as i64,
                record.error,
                record.started_at,
                record.finished_at,
                record.resumable as i64,
            ],
        )
        .map_err(storage_error)?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_record(&self, record: &Record) -> Result<(), BtError> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE records SET status = ?1, byte_size = ?2, transferred = ?3, error = ?4, finished_at = ?5, path = ?6, resumable = ?7 WHERE id = ?8",
            rusqlite::params![
                record.status.as_str(),
                record.byte_size as i64,
                record.transferred as i64,
                record.error,
                record.finished_at,
                record.path.as_ref().map(|path| path.to_string_lossy().into_owned()),
                record.resumable as i64,
                record.id,
            ],
        )
        .map_err(storage_error)?;
        Ok(())
    }

    pub fn query_records(&self, query: &RecordQuery) -> Result<Vec<Record>, BtError> {
        let conn = self.lock()?;
        let (clauses, values) = build_filters(query);
        let mut sql = format!("SELECT {RECORD_COLUMNS} FROM records");
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY started_at DESC, id DESC");
        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        if let Some(offset) = query.offset {
            sql.push_str(&format!(" OFFSET {offset}"));
        }
        let mut statement = conn.prepare(&sql).map_err(storage_error)?;
        let rows = statement
            .query_map(params_from_iter(values.iter()), row_to_record)
            .map_err(storage_error)?;
        let mut records = Vec::new();
        for row in rows {
            let data = row.map_err(storage_error)?;
            records.push(data.into_record()?);
        }
        Ok(records)
    }

    pub fn stats(&self) -> Result<Stats, BtError> {
        let conn = self.lock()?;
        let mut statement = conn
            .prepare("SELECT direction, status, byte_size, started_at FROM records")
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?.max(0) as u64,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(storage_error)?;
        let mut stats = Stats::default();
        for row in rows {
            let (direction, status, bytes, started_at) = row.map_err(storage_error)?;
            stats.total += 1;
            let is_receive = direction == "receive";
            if is_receive {
                stats.received += 1;
                stats.bytes_received += bytes;
            } else {
                stats.sent += 1;
                stats.bytes_sent += bytes;
            }
            if status == "failed" {
                stats.failed += 1;
            }
            let date = started_at.chars().take(10).collect::<String>();
            match stats.days.iter_mut().find(|day| day.date == date) {
                Some(day) => {
                    if is_receive {
                        day.received += 1;
                    } else {
                        day.sent += 1;
                    }
                    day.bytes += bytes;
                }
                None => {
                    let mut day = DayStat {
                        date,
                        ..DayStat::default()
                    };
                    if is_receive {
                        day.received = 1;
                    } else {
                        day.sent = 1;
                    }
                    day.bytes = bytes;
                    stats.days.push(day);
                }
            }
            if stats
                .last_activity
                .as_deref()
                .is_none_or(|value| started_at.as_str() > value)
            {
                stats.last_activity = Some(started_at);
            }
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

    pub fn clear_records(&self) -> Result<u64, BtError> {
        let conn = self.lock()?;
        let affected = conn.execute("DELETE FROM records", []).map_err(storage_error)?;
        Ok(affected as u64)
    }
}

fn storage_error(error: rusqlite::Error) -> BtError {
    BtError::storage(format!("记录库错误：{error}"))
}

fn build_filters(query: &RecordQuery) -> (Vec<String>, Vec<Value>) {
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
        clauses.push("device_id = ?".to_string());
        values.push(Value::Text(device.to_string()));
    }
    if let Some(search) = query.search.as_deref().filter(|text| !text.is_empty()) {
        clauses.push("(display_name LIKE ? OR device_name LIKE ?)".to_string());
        let pattern = format!("%{search}%");
        values.push(Value::Text(pattern.clone()));
        values.push(Value::Text(pattern));
    }
    if let Some(from) = query.from.as_deref() {
        clauses.push("started_at >= ?".to_string());
        values.push(Value::Text(from.to_string()));
    }
    if let Some(to) = query.to.as_deref() {
        clauses.push("started_at <= ?".to_string());
        values.push(Value::Text(to.to_string()));
    }
    (clauses, values)
}

struct RowData {
    id: i64,
    direction: String,
    channel: String,
    kind: String,
    status: String,
    device_id: Option<String>,
    device_name: String,
    display_name: String,
    path: Option<String>,
    byte_size: i64,
    transferred: i64,
    error: Option<String>,
    started_at: String,
    finished_at: Option<String>,
    resumable: i64,
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RowData> {
    Ok(RowData {
        id: row.get(0)?,
        direction: row.get(1)?,
        channel: row.get(2)?,
        kind: row.get(3)?,
        status: row.get(4)?,
        device_id: row.get(5)?,
        device_name: row.get(6)?,
        display_name: row.get(7)?,
        path: row.get(8)?,
        byte_size: row.get(9)?,
        transferred: row.get(10)?,
        error: row.get(11)?,
        started_at: row.get(12)?,
        finished_at: row.get(13)?,
        resumable: row.get(14)?,
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
        let device_id = match self.device_id.as_deref() {
            Some(value) => Some(DeviceId::parse(value)?),
            None => None,
        };
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
            byte_size: self.byte_size.max(0) as u64,
            transferred: self.transferred.max(0) as u64,
            error: self.error,
            started_at: self.started_at,
            finished_at: self.finished_at,
            resumable: self.resumable != 0,
        })
    }
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
        assert_eq!(records[0].status, RecordStatus::Completed);
        assert_eq!(records[0].device_name, "Pixel");
        assert!(store.delete_record(id).unwrap());
        assert!(!store.delete_record(id).unwrap());
        assert!(store.query_records(&RecordQuery::default()).unwrap().is_empty());
    }

    #[test]
    fn query_filters_by_direction_and_search() {
        let store = Store::open_in_memory().unwrap();
        store.insert_record(&sample_record()).unwrap();
        let mut received = sample_record();
        received.direction = Direction::Receive;
        received.display_name = "note.txt".to_string();
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
                search: Some("note".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(searched.len(), 1);
        assert_eq!(searched[0].display_name, "note.txt");
    }

    #[test]
    fn stats_and_clear_cover_all_records() {
        let store = Store::open_in_memory().unwrap();
        store.insert_record(&sample_record()).unwrap();
        let stats = store.stats().unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.sent, 1);
        assert_eq!(stats.bytes_sent, 1024);
        assert_eq!(store.clear_records().unwrap(), 1);
        assert_eq!(store.stats().unwrap().total, 0);
    }

    #[test]
    fn send_source_display_name_is_used_by_records() {
        let source = SendSource::Text("hello".into());
        assert_eq!(source.display_name(), "hello");
    }
}
