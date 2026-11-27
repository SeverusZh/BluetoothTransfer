use std::path::Path;

use bt_core::{BtError, Config, ProtectionLevel, TransferMode};
use toml::{Table, Value};

/// 读取配置；文件缺失时返回传入的默认值，字段缺失的项保持默认。
pub fn load_config(path: &Path, defaults: Config) -> Result<Config, BtError> {
    if !path.exists() {
        return Ok(defaults);
    }
    let text = std::fs::read_to_string(path)?;
    let parsed: Value = toml::from_str(&text).map_err(|error| BtError::storage(format!("配置解析失败：{error}")))?;
    let table = parsed
        .as_table()
        .ok_or_else(|| BtError::storage("配置文件根节点不是表"))?;
    let mut config = defaults;

    if let Some(value) = take_integer(table, "opp_chunk_size") {
        config.opp_chunk_size = clamp_u32(value);
    }
    if let Some(value) = take_integer(table, "opp_connect_timeout_s") {
        config.opp_connect_timeout_s = clamp_u32(value);
    }
    if let Some(value) = take_integer(table, "opp_send_idle_timeout_s") {
        config.opp_send_idle_timeout_s = clamp_u32(value);
    }
    if let Some(value) = take_integer(table, "opp_retry_count") {
        config.opp_retry_count = clamp_u32(value);
    }
    if let Some(value) = take_integer(table, "opp_retry_delay_s") {
        config.opp_retry_delay_s = clamp_u32(value);
    }
    if let Some(value) = take_string(table, "opp_protection") {
        config.opp_protection = value.parse::<ProtectionLevel>()?;
    }
    if let Some(value) = take_boolean(table, "opp_name_bom") {
        config.opp_name_bom = value;
    }
    if let Some(value) = take_integer(table, "assistant_chunk_size") {
        config.assistant_chunk_size = clamp_u32(value);
    }
    if let Some(value) = take_string(table, "push_text_file_name") {
        config.push_text_file_name = value;
    }
    if let Some(value) = take_string(table, "receive_dir") {
        config.receive_dir = std::path::PathBuf::from(value);
    }
    if let Some(value) = take_boolean(table, "receive_ask") {
        config.receive_ask = value;
    }
    if let Some(value) = take_string(table, "mode") {
        config.mode = value.parse::<TransferMode>()?;
    }

    config.normalize();
    Ok(config)
}

/// 落盘配置，只写与默认值不同的字段，便于人工审阅与迁移。
pub fn save_config(path: &Path, config: &Config, defaults: &Config) -> Result<(), BtError> {
    let text = render_config(config, defaults);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, text.as_bytes())?;
    restrict_permissions(&temporary)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// 生成只含非默认值的 TOML 文本。
pub fn render_config(config: &Config, defaults: &Config) -> String {
    let mut table = Table::new();
    if config.opp_chunk_size != defaults.opp_chunk_size {
        table.insert(
            "opp_chunk_size".into(),
            Value::Integer(i64::from(config.opp_chunk_size)),
        );
    }
    if config.opp_connect_timeout_s != defaults.opp_connect_timeout_s {
        table.insert(
            "opp_connect_timeout_s".into(),
            Value::Integer(i64::from(config.opp_connect_timeout_s)),
        );
    }
    if config.opp_send_idle_timeout_s != defaults.opp_send_idle_timeout_s {
        table.insert(
            "opp_send_idle_timeout_s".into(),
            Value::Integer(i64::from(config.opp_send_idle_timeout_s)),
        );
    }
    if config.opp_retry_count != defaults.opp_retry_count {
        table.insert(
            "opp_retry_count".into(),
            Value::Integer(i64::from(config.opp_retry_count)),
        );
    }
    if config.opp_retry_delay_s != defaults.opp_retry_delay_s {
        table.insert(
            "opp_retry_delay_s".into(),
            Value::Integer(i64::from(config.opp_retry_delay_s)),
        );
    }
    if config.opp_protection != defaults.opp_protection {
        table.insert(
            "opp_protection".into(),
            Value::String(config.opp_protection.to_string()),
        );
    }
    if config.opp_name_bom != defaults.opp_name_bom {
        table.insert("opp_name_bom".into(), Value::Boolean(config.opp_name_bom));
    }
    if config.assistant_chunk_size != defaults.assistant_chunk_size {
        table.insert(
            "assistant_chunk_size".into(),
            Value::Integer(i64::from(config.assistant_chunk_size)),
        );
    }
    if config.push_text_file_name != defaults.push_text_file_name {
        table.insert(
            "push_text_file_name".into(),
            Value::String(config.push_text_file_name.clone()),
        );
    }
    if config.receive_dir != defaults.receive_dir {
        table.insert(
            "receive_dir".into(),
            Value::String(config.receive_dir.to_string_lossy().into_owned()),
        );
    }
    if config.receive_ask != defaults.receive_ask {
        table.insert("receive_ask".into(), Value::Boolean(config.receive_ask));
    }
    if config.mode != defaults.mode {
        table.insert("mode".into(), Value::String(config.mode.to_string()));
    }
    toml::to_string(&table).unwrap_or_default()
}

fn take_integer(table: &Table, key: &str) -> Option<i64> {
    table.get(key).and_then(Value::as_integer)
}

fn take_boolean(table: &Table, key: &str) -> Option<bool> {
    table.get(key).and_then(Value::as_bool)
}

fn take_string(table: &Table, key: &str) -> Option<String> {
    table.get(key).and_then(Value::as_str).map(|value| value.to_string())
}

fn clamp_u32(value: i64) -> u32 {
    value.clamp(0, i64::from(u32::MAX)) as u32
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), BtError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), BtError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_only_non_default_fields() {
        let defaults = Config::default();
        let rendered = render_config(&defaults, &defaults);
        eprintln!("MEASURE config_default_bytes={}", rendered.len());
        assert!(rendered.is_empty());

        let mut changed = defaults.clone();
        changed.opp_retry_count = 5;
        changed.mode = TransferMode::Opp;
        let text = render_config(&changed, &defaults);
        assert!(text.contains("opp_retry_count = 5"));
        assert!(text.contains("mode = \"opp\""));
        assert!(!text.contains("receive_ask"));
    }

    #[test]
    fn load_applies_partial_file_over_defaults() {
        let dir = std::env::temp_dir().join("bt-store-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "opp_retry_count = 7\nmode = \"assistant\"\n").unwrap();

        let config = load_config(&path, Config::default()).unwrap();
        assert_eq!(config.opp_retry_count, 7);
        assert_eq!(config.mode, TransferMode::Assistant);
        assert_eq!(config.opp_chunk_size, defaults_chunk());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_then_load_round_trips_non_defaults() {
        let dir = std::env::temp_dir().join("bt-store-config-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let defaults = Config::default();
        let mut config = defaults.clone();
        config.receive_ask = false;
        config.push_text_file_name = "note.txt".to_string();

        save_config(&path, &config, &defaults).unwrap();
        let loaded = load_config(&path, defaults.clone()).unwrap();
        assert_eq!(loaded, config);
        let _ = std::fs::remove_file(&path);
    }

    fn defaults_chunk() -> u32 {
        Config::default().opp_chunk_size
    }
}
