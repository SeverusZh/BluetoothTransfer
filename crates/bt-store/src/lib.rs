//! 持久化层：SQLite 记录库、TOML 配置读写、导出与旧格式迁移。

pub mod config_file;
pub mod db;
pub mod export;
pub mod migrate;

pub use config_file::{load_config, render_config, save_config};
pub use db::Store;
pub use export::{export_records, render_csv, render_jsonl};
pub use migrate::migrate_legacy;

/// 当前数据库结构版本，写入 PRAGMA user_version。
pub const SCHEMA_VERSION: i64 = 1;
