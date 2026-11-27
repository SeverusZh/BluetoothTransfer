//! 持久化层：SQLite 记录库、TOML 配置、流式导出与旧格式迁移。

pub mod config_file;
pub mod db;
pub mod export;
pub mod migrate;

pub use config_file::{load_config, render_config, save_config};
pub use db::{DeviceRecord, Store};
pub use export::{export_records, render_csv, render_csv_row, render_jsonl, render_jsonl_row};
pub use migrate::{
    looks_like_legacy_root, migrate_legacy, read_legacy_config, read_legacy_journal, read_legacy_records,
    LegacyJournalEntry, MigrateTarget, LEGACY_CONFIG_FILE, LEGACY_DATABASE_FILE, LEGACY_JOURNAL_FILE,
};

/// 当前数据库结构版本，写入 PRAGMA user_version。
pub const SCHEMA_VERSION: i64 = 1;
