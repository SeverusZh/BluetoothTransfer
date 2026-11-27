use std::path::{Path, PathBuf};

use bt_api::{MigrateOptions, MigrateReport};
use bt_core::{
    parse_transfer_id, BtError, Channel, Config, DeviceId, Direction, ProtectionLevel, Record, RecordKind,
    RecordStatus, TransferMode, MAX_RECORD_CONTENT_BYTES,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use crate::config_file::{load_config, save_config};
use crate::db::{normalize_time, now_epoch, Store};

/// 旧版本 .NET 应用在 %APPDATA% 下使用的配置文件名。
pub const LEGACY_CONFIG_FILE: &str = "config.json";
pub const LEGACY_DATABASE_FILE: &str = "data.db";
pub const LEGACY_JOURNAL_FILE: &str = "send-journal.json";

/// 迁移落点。
#[derive(Debug, Clone)]
pub struct MigrateTarget {
    pub config_file: PathBuf,
    pub database_file: PathBuf,
    pub password_file: PathBuf,
    pub defaults: Config,
}

/// 旧 journal 条目，供装配层转换为新格式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyJournalEntry {
    pub id: String,
    pub peer_addr: String,
    pub display_name: String,
    pub size: u64,
    pub sha256: String,
    pub payload_path: PathBuf,
    pub kind: String,
    pub produced: bool,
    pub state: String,
    pub attempts: u32,
    pub created_epoch: u32,
    pub updated_epoch: u32,
}

/// 判断目录是否是旧版应用数据目录。
pub fn looks_like_legacy_root(root: &Path) -> bool {
    root.join(LEGACY_CONFIG_FILE).is_file() || root.join(LEGACY_DATABASE_FILE).is_file()
}

/// 迁移旧配置、记录与认证密码；journal 由装配层用 read_legacy_journal 单独导入。
pub fn migrate_legacy(options: &MigrateOptions, target: &MigrateTarget) -> Result<MigrateReport, BtError> {
    if !options.legacy_root.exists() {
        return Err(BtError::not_found(format!(
            "旧数据目录不存在 {}",
            options.legacy_root.display()
        )));
    }
    let mut report = MigrateReport {
        dry_run: options.dry_run,
        ..MigrateReport::default()
    };
    if !looks_like_legacy_root(&options.legacy_root) {
        report.warnings.push(format!(
            "目录中没有可识别的旧版数据，未做任何修改：{}",
            options.legacy_root.display()
        ));
        return Ok(report);
    }
    let mut password: Option<String> = None;

    match read_legacy_config(&options.legacy_root.join(LEGACY_CONFIG_FILE), &target.defaults) {
        Ok(Some((config, secret))) => {
            password = secret;
            if options.dry_run {
                report.imported_config = true;
            } else if target.config_file.exists()
                && std::fs::metadata(&target.config_file)
                    .map(|meta| meta.len())
                    .unwrap_or(0)
                    > 0
            {
                report
                    .warnings
                    .push("目标配置已存在且非空，保留现有配置，未覆盖".to_string());
            } else {
                save_config(&target.config_file, &config, &target.defaults)?;
                report.imported_config = true;
            }
        }
        Ok(None) => {}
        Err(error) => report.warnings.push(format!("旧配置解析失败：{error}")),
    }

    let legacy_records = match read_legacy_records(&options.legacy_root.join(LEGACY_DATABASE_FILE)) {
        Ok(records) => records,
        Err(error) => {
            report.warnings.push(format!("旧记录库读取失败：{error}"));
            Vec::new()
        }
    };
    let total_legacy = legacy_records.len() as u64;
    if options.dry_run {
        report.imported_records = total_legacy;
    } else if total_legacy > 0 {
        let store = Store::open(&target.database_file)?;
        if store.record_count()? > 0 {
            report.skipped += total_legacy;
            report
                .warnings
                .push("目标记录库已有数据，跳过记录导入以避免重复".to_string());
        } else {
            let mut oversized = 0u64;
            for record in legacy_records {
                if record
                    .content
                    .as_ref()
                    .is_some_and(|text| text.len() > MAX_RECORD_CONTENT_BYTES)
                {
                    oversized += 1;
                }
                match store.insert_record(&record) {
                    Ok(_) => report.imported_records += 1,
                    Err(error) => {
                        report.skipped += 1;
                        if report.warnings.len() < 20 {
                            report
                                .warnings
                                .push(format!("记录导入失败 {}：{error}", record.display_name));
                        }
                    }
                }
            }
            if oversized > 0 {
                report.warnings.push(format!(
                    "{oversized} 条文本记录正文超过 {MAX_RECORD_CONTENT_BYTES} 字节，正文未导入"
                ));
            }
        }
    }

    if let Some(secret) = password {
        if options.dry_run {
            report
                .warnings
                .push("检测到旧 OPP 认证密码，正式迁移会写入权限 0600 的独立文件".to_string());
        } else {
            write_secret(&target.password_file, &secret)?;
            report
                .warnings
                .push("旧 OPP 认证密码已迁移到权限 0600 的独立文件，未写入配置".to_string());
        }
    }
    if options.legacy_root.join(LEGACY_JOURNAL_FILE).is_file() {
        report
            .warnings
            .push("检测到旧发送日志，装配层会按续传条目导入；源文件变更的条目将被放弃".to_string());
    }
    Ok(report)
}

/// 读取旧 config.json；返回归一化配置与独立保存的认证密码。
pub fn read_legacy_config(path: &Path, defaults: &Config) -> Result<Option<(Config, Option<String>)>, BtError> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| BtError::invalid(format!("旧配置不是合法 JSON：{error}")))?;
    let Some(table) = value.as_object() else {
        return Err(BtError::invalid("旧配置根节点不是对象"));
    };
    let mut config = defaults.clone();
    if let Some(value) = pick_u32(table, &["OppChunkSize", "opp_chunk_size"]) {
        config.opp_chunk_size = value;
    }
    if let Some(value) = pick_u32(table, &["OppConnectTimeoutSeconds", "opp_connect_timeout_s"]) {
        config.opp_connect_timeout_s = value;
    }
    if let Some(value) = pick_u32(table, &["OppSendTimeoutSeconds", "opp_send_idle_timeout_s"]) {
        config.opp_send_idle_timeout_s = value;
    }
    if let Some(value) = pick_u32(table, &["OppRetryCount", "opp_retry_count"]) {
        config.opp_retry_count = value;
    }
    if let Some(value) = pick_u32(table, &["OppRetryDelaySeconds", "opp_retry_delay_s"]) {
        config.opp_retry_delay_s = value;
    }
    if let Some(value) = pick_str(table, &["OppProtectionLevel", "opp_protection"]) {
        config.opp_protection = value.parse::<ProtectionLevel>()?;
    }
    if let Some(value) = pick_bool(table, &["OppNameUseBom", "opp_name_bom"]) {
        config.opp_name_bom = value;
    }
    if let Some(value) = pick_u32(table, &["AssistantChunkSize", "assistant_chunk_size"]) {
        config.assistant_chunk_size = value;
    }
    if let Some(value) = pick_str(table, &["PushTextFileName", "push_text_file_name"]) {
        config.push_text_file_name = value;
    }
    if let Some(value) = pick_str(table, &["ReceiveDirectory", "receive_dir"]) {
        config.receive_dir = PathBuf::from(value);
    }
    if let Some(value) = pick_bool(table, &["ReceiveAsk", "receive_ask"]) {
        config.receive_ask = value;
    }
    if let Some(value) = pick_str(table, &["TransferMode", "mode"]) {
        config.mode = value.parse::<TransferMode>()?;
    }
    config.normalize();
    let secret = pick_str(table, &["OppAuthPassword", "opp_auth_password"]).filter(|value| !value.is_empty());
    Ok(Some((config, secret)))
}

/// 读取旧 data.db 的 transfer_records，字段映射到新记录模型。
pub fn read_legacy_records(path: &Path) -> Result<Vec<Record>, BtError> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| BtError::storage(format!("打开旧记录库失败：{error}")))?;
    let table: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'transfer_records'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| BtError::storage(format!("查询旧表失败：{error}")))?;
    if table.is_none() {
        return Ok(Vec::new());
    }
    let has_content = column_exists(&conn, "transfer_records", "content")?;
    let content_column = if has_content { "content" } else { "''" };
    let sql = format!(
        "SELECT created_at, direction, type, peer_name, peer_addr, name, size, status, checksum, channel, local_path, note, {content_column} FROM transfer_records"
    );
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| BtError::storage(format!("读取旧表失败：{error}")))?;
    let rows = statement
        .query_map([], |row| {
            Ok(LegacyRow {
                created_at: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                direction: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                kind: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                peer_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                peer_addr: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                name: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                size: row.get::<_, i64>(6).unwrap_or(0),
                status: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                checksum: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                channel: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                local_path: row.get::<_, Option<String>>(10)?.unwrap_or_default(),
                note: row.get::<_, Option<String>>(11)?.unwrap_or_default(),
                content: row.get::<_, Option<String>>(12)?.unwrap_or_default(),
            })
        })
        .map_err(|error| BtError::storage(format!("遍历旧记录失败：{error}")))?;
    let mut records = Vec::new();
    for row in rows {
        let row = row.map_err(|error| BtError::storage(format!("读取旧记录失败：{error}")))?;
        records.push(row.into_record());
    }
    Ok(records)
}

struct LegacyRow {
    created_at: String,
    direction: String,
    kind: String,
    peer_name: String,
    peer_addr: String,
    name: String,
    size: i64,
    status: String,
    checksum: String,
    channel: String,
    local_path: String,
    note: String,
    content: String,
}

impl LegacyRow {
    fn into_record(self) -> Record {
        let kind = match self.kind.as_str() {
            "text" => RecordKind::Text,
            "folder" => RecordKind::Folder,
            "file" => RecordKind::File,
            _ => RecordKind::Other,
        };
        let status = if self.status == "failed" {
            RecordStatus::Failed
        } else {
            RecordStatus::Completed
        };
        let channel = if self.channel == "assistant" {
            Channel::Assistant
        } else {
            Channel::Opp
        };
        let started_epoch = normalize_time(&self.created_at).unwrap_or_else(now_epoch);
        let started_at = bt_core::rfc3339_from_unix(started_epoch);
        let byte_size = self.size.max(0) as u64;
        let checksum = parse_transfer_id(&self.checksum).ok();
        let content = if kind == RecordKind::Text && !self.content.is_empty() {
            Some(self.content)
        } else {
            None
        };
        let mut record = Record::running(
            if self.direction == "recv" || self.direction == "receive" {
                Direction::Receive
            } else {
                Direction::Send
            },
            channel,
            kind,
            self.name,
            started_at.clone(),
        );
        record.status = status;
        record.device_id = DeviceId::parse(&self.peer_addr).ok();
        record.device_name = self.peer_name;
        record.path = if self.local_path.is_empty() {
            None
        } else {
            Some(PathBuf::from(self.local_path))
        };
        record.checksum = checksum;
        record.content = content;
        record.byte_size = byte_size;
        record.transferred = byte_size;
        record.error = if status == RecordStatus::Failed && !self.note.is_empty() {
            Some(self.note)
        } else {
            None
        };
        record.finished_at = Some(started_at);
        record
    }
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, BtError> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| BtError::storage(format!("读取旧表结构失败：{error}")))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| BtError::storage(format!("读取旧表结构失败：{error}")))?;
    for row in rows {
        if row.map_err(|error| BtError::storage(error.to_string()))? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 读取旧 send-journal.json，只取可续传字段。
pub fn read_legacy_journal(path: &Path) -> Result<Vec<LegacyJournalEntry>, BtError> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| BtError::invalid(format!("旧 journal 不是合法 JSON：{error}")))?;
    let Some(items) = value.as_array() else {
        return Err(BtError::invalid("旧 journal 根节点不是数组"));
    };
    let mut entries = Vec::new();
    for item in items {
        let Some(table) = item.as_object() else {
            continue;
        };
        let payload_path = pick_str(table, &["PayloadPath", "payload_path"]).unwrap_or_default();
        if payload_path.is_empty() {
            continue;
        }
        let display_name = pick_str(table, &["DisplayName", "display_name"]).unwrap_or_default();
        let size = pick_u64(table, &["Size", "size"]).unwrap_or(0);
        let sha256 = pick_str(table, &["Sha256", "sha256"]).unwrap_or_default();
        let state = pick_str(table, &["State", "state"]).unwrap_or_else(|| "pending".to_string());
        if display_name.is_empty() || parse_transfer_id(&sha256).is_err() {
            continue;
        }
        let peer_addr = pick_str(table, &["PeerAddr", "peer_addr"]).unwrap_or_default();
        if DeviceId::parse(&peer_addr).is_err() {
            continue;
        }
        let created = pick_str(table, &["CreatedAt", "created_at"])
            .and_then(|text| normalize_time(&text))
            .unwrap_or_else(now_epoch);
        let updated = pick_str(table, &["UpdatedAt", "updated_at"])
            .and_then(|text| normalize_time(&text))
            .unwrap_or(created);
        entries.push(LegacyJournalEntry {
            id: pick_str(table, &["Id", "id"]).unwrap_or_default(),
            peer_addr,
            display_name,
            size,
            sha256,
            payload_path: PathBuf::from(payload_path),
            kind: pick_str(table, &["Kind", "kind"]).unwrap_or_else(|| "file".to_string()),
            produced: pick_bool(table, &["IsProducedPackage", "is_produced_package"]).unwrap_or(false),
            state,
            attempts: pick_u32(table, &["Attempts", "attempts"]).unwrap_or(0),
            created_epoch: created.clamp(0, u32::MAX as i64) as u32,
            updated_epoch: updated.clamp(0, u32::MAX as i64) as u32,
        });
    }
    Ok(entries)
}

fn write_secret(path: &Path, secret: &str) -> Result<(), BtError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, secret.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn pick_str(table: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| table.get(*key).and_then(Value::as_str).map(str::to_string))
}

fn pick_u64(table: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        table.get(*key).and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
    })
}

fn pick_u32(table: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u32> {
    pick_u64(table, keys).map(|value| value.min(u32::MAX as u64) as u32)
}

fn pick_bool(table: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| table.get(*key).and_then(Value::as_bool))
}

/// 迁移后重新读取配置，确认落盘可用。
pub fn reload_config(target: &MigrateTarget) -> Result<Config, BtError> {
    load_config(&target.config_file, target.defaults.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "bt-migrate-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_legacy_database(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE transfer_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at TEXT NOT NULL,
                direction TEXT NOT NULL,
                type TEXT NOT NULL,
                peer_name TEXT,
                peer_addr TEXT,
                name TEXT,
                size INTEGER DEFAULT 0,
                status TEXT DEFAULT 'ok',
                checksum TEXT,
                channel TEXT DEFAULT 'ble',
                local_path TEXT,
                note TEXT,
                content TEXT
            );
            PRAGMA user_version = 2;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transfer_records (created_at, direction, type, peer_name, peer_addr, name, size, status, checksum, channel, local_path, note, content) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                "2026-01-02T03:04:05.0000000+08:00",
                "send",
                "file",
                "Pixel",
                "AA:BB:CC:DD:EE:01",
                "photo.jpg",
                1024,
                "ok",
                "ab".repeat(32),
                "assistant",
                "/data/photo.jpg",
                "",
                "",
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transfer_records (created_at, direction, type, peer_name, peer_addr, name, size, status, checksum, channel, local_path, note, content) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                "2026-01-03T00:00:00Z",
                "recv",
                "text",
                "Pixel",
                "AA:BB:CC:DD:EE:01",
                "note.txt",
                5,
                "failed",
                "",
                "opp",
                "",
                "校验失败",
                "hello",
            ],
        )
        .unwrap();
    }

    fn target(dir: &TempDir) -> MigrateTarget {
        MigrateTarget {
            config_file: dir.0.join("config.toml"),
            database_file: dir.0.join("records.db"),
            password_file: dir.0.join("auth.token"),
            defaults: Config::default(),
        }
    }

    #[test]
    fn migrates_config_records_and_password() {
        let dir = TempDir::new("full");
        let legacy = dir.0.join("legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join(LEGACY_CONFIG_FILE),
            r#"{"OppChunkSize": 4096, "OppRetryCount": 7, "TransferMode": "assistant", "OppAuthPassword": "s3cret", "ReceiveDirectory": "/tmp/recv"}"#,
        )
        .unwrap();
        write_legacy_database(&legacy.join(LEGACY_DATABASE_FILE));
        let target = target(&dir);
        let report = migrate_legacy(&MigrateOptions::new(legacy), &target).unwrap();
        assert_eq!(report.imported_records, 2);
        assert_eq!(report.skipped, 0);
        assert!(report.imported_config);
        let config = load_config(&target.config_file, Config::default()).unwrap();
        assert_eq!(config.opp_chunk_size, 4096);
        assert_eq!(config.opp_retry_count, 7);
        assert_eq!(config.mode, TransferMode::Assistant);
        assert_eq!(config.receive_dir, PathBuf::from("/tmp/recv"));
        assert!(!std::fs::read_to_string(&target.config_file).unwrap().contains("s3cret"));
        assert_eq!(std::fs::read_to_string(&target.password_file).unwrap(), "s3cret");
        let store = Store::open(&target.database_file).unwrap();
        let records = store.query_records(&Default::default()).unwrap();
        assert_eq!(records.len(), 2);
        let failed = records
            .iter()
            .find(|record| record.status == RecordStatus::Failed)
            .unwrap();
        assert_eq!(failed.direction, Direction::Receive);
        assert_eq!(failed.content.as_deref(), Some("hello"));
        assert_eq!(failed.error.as_deref(), Some("校验失败"));
        let sent = records
            .iter()
            .find(|record| record.status == RecordStatus::Completed)
            .unwrap();
        assert_eq!(sent.device_id, DeviceId::parse("AA:BB:CC:DD:EE:01").ok());
        assert_eq!(sent.checksum, Some([0xab; 32]));
        assert_eq!(sent.channel, Channel::Assistant);
    }

    #[test]
    fn dry_run_does_not_write() {
        let dir = TempDir::new("dry");
        let legacy = dir.0.join("legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        writ_legacy_config(&legacy);
        write_legacy_database(&legacy.join(LEGACY_DATABASE_FILE));
        let target = target(&dir);
        let mut options = MigrateOptions::new(legacy);
        options.dry_run = true;
        let report = migrate_legacy(&options, &target).unwrap();
        assert_eq!(report.imported_records, 2);
        assert!(!target.config_file.exists());
        assert!(!target.database_file.exists());
        assert!(!target.password_file.exists());
    }

    fn writ_legacy_config(legacy: &Path) {
        std::fs::write(legacy.join(LEGACY_CONFIG_FILE), r#"{"OppAuthPassword": "x"}"#).unwrap();
    }

    #[test]
    fn second_migration_skips_existing_records() {
        let dir = TempDir::new("twice");
        let legacy = dir.0.join("legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        writ_legacy_config(&legacy);
        write_legacy_database(&legacy.join(LEGACY_DATABASE_FILE));
        let target = target(&dir);
        migrate_legacy(&MigrateOptions::new(legacy.clone()), &target).unwrap();
        let report = migrate_legacy(&MigrateOptions::new(legacy), &target).unwrap();
        assert_eq!(report.imported_records, 0);
        assert_eq!(report.skipped, 2);
    }

    #[test]
    fn reads_legacy_journal_entries() {
        let dir = TempDir::new("journal");
        let path = dir.0.join(LEGACY_JOURNAL_FILE);
        std::fs::write(
            &path,
            r#"[{"Id":"abc","PeerAddr":"AA:BB:CC:DD:EE:02","DisplayName":"a.bin","Size":10,"Sha256":"abababababababababababababababababababababababababababababababab","PayloadPath":"/tmp/a.bin","Kind":"file","IsProducedPackage":false,"State":"sending","Attempts":2,"CreatedAt":"2026-01-01T00:00:00Z","UpdatedAt":"2026-01-02T00:00:00Z"}]"#,
        )
        .unwrap();
        let entries = read_legacy_journal(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].display_name, "a.bin");
        assert_eq!(entries[0].attempts, 2);
        assert_eq!(entries[0].created_epoch, 1_767_225_600);
    }

    #[test]
    fn missing_root_reports_not_found() {
        let options = MigrateOptions::new(PathBuf::from("/nonexistent/bt-legacy-root"));
        let error = migrate_legacy(&options, &target(&TempDir::new("missing"))).unwrap_err();
        assert_eq!(error.kind(), bt_core::BtErrorKind::NotFound);
    }

    #[test]
    fn empty_directory_is_a_warning_not_an_error() {
        let dir = TempDir::new("empty");
        let legacy = dir.0.join("legacy");
        std::fs::create_dir_all(&legacy).unwrap();
        let report = migrate_legacy(&MigrateOptions::new(legacy), &target(&dir)).unwrap();
        assert_eq!(report.imported_records, 0);
        assert_eq!(report.warnings.len(), 1);
        assert!(!target(&dir).config_file.exists());
    }
}
