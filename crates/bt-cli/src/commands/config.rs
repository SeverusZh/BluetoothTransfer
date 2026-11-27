use std::path::PathBuf;

use bt_api::ConfigPatch;
use bt_core::{Config, ProtectionLevel, TransferMode};
use serde::Serialize;
use serde_json::{json, Value};

use crate::backend::{read_auth_password, Context};
use crate::cli::ConfigCommand;
use crate::error::{CliError, EXIT_OK};
use crate::output::emit_result;

#[derive(Serialize)]
struct ConfigListResult {
    ok: bool,
    config: Config,
}

#[derive(Serialize)]
struct ConfigGetResult {
    ok: bool,
    key: String,
    value: Value,
}

#[derive(Serialize)]
struct ConfigSetResult {
    ok: bool,
    key: String,
    value: Value,
}

#[derive(Serialize)]
struct PasswordResult {
    ok: bool,
    key: &'static str,
    set: bool,
}

const PASSWORD_KEY: &str = "auth-password";

pub fn dispatch(ctx: &Context, action: Option<&ConfigCommand>) -> Result<u8, CliError> {
    match action {
        None | Some(ConfigCommand::List) => list(ctx),
        Some(ConfigCommand::Get { key }) => get(ctx, key),
        Some(ConfigCommand::Set { key, value }) => set(ctx, key, value.as_deref()),
    }
}

pub fn list(ctx: &Context) -> Result<u8, CliError> {
    let config = ctx.backend.config();
    emit_result(
        ctx.mode,
        &ConfigListResult {
            ok: true,
            config: config.clone(),
        },
    );
    if ctx.mode.is_human() {
        for (key, value) in config_entries(&config) {
            println!("{key} = {value}");
        }
        println!("auth-password = （不可读取，仅可通过 stdin 设置）");
    }
    Ok(EXIT_OK)
}

pub fn get(ctx: &Context, key: &str) -> Result<u8, CliError> {
    let key = canonical_key(key)?;
    if key == PASSWORD_KEY {
        return Err(CliError::usage("认证密码不可读取，只能通过 stdin 设置")
            .with_hint("使用 printf '%s' \"$PASSWORD\" | bt config set auth-password"));
    }
    let config = ctx.backend.config();
    let value = key_value(&config, &key)?;
    emit_result(
        ctx.mode,
        &ConfigGetResult {
            ok: true,
            key: key.clone(),
            value: value.clone(),
        },
    );
    if ctx.mode.is_human() {
        println!("{key} = {}", display_value(&value));
    }
    Ok(EXIT_OK)
}

pub fn set(ctx: &Context, key: &str, value: Option<&str>) -> Result<u8, CliError> {
    let key = canonical_key(key)?;
    if key == PASSWORD_KEY {
        if value.is_some() {
            return Err(CliError::usage("认证密码不接受命令行值")
                .with_hint("改用：printf '%s' \"$PASSWORD\" | bt config set auth-password"));
        }
        if ctx.repl {
            return Err(CliError::usage("repl 模式下无法安全读取认证密码")
                .with_hint("退出 repl 后用管道 stdin 设置：printf '%s' \"$PASSWORD\" | bt config set auth-password"));
        }
        let password = read_auth_password()?;
        ctx.backend.set_auth_password(password.as_deref())?;
        let set = password.is_some();
        emit_result(
            ctx.mode,
            &PasswordResult {
                ok: true,
                key: PASSWORD_KEY,
                set,
            },
        );
        if ctx.mode.is_human() {
            if set {
                println!("已设置 OPP 认证密码（不回显）");
            } else {
                println!("已清除 OPP 认证密码");
            }
        }
        return Ok(EXIT_OK);
    }
    let raw = value
        .ok_or_else(|| CliError::usage(format!("配置项 {key} 需要值")).with_hint("示例：bt config set mode opp"))?;
    let patch = patch_for(&key, raw)?;
    let config = ctx.backend.update_config(patch)?;
    let value = key_value(&config, &key)?;
    emit_result(
        ctx.mode,
        &ConfigSetResult {
            ok: true,
            key: key.clone(),
            value: value.clone(),
        },
    );
    if ctx.mode.is_human() {
        println!("已保存：{key} = {}", display_value(&value));
    }
    Ok(EXIT_OK)
}

fn canonical_key(raw: &str) -> Result<String, CliError> {
    let lower = raw.trim().to_ascii_lowercase();
    let key = match lower.as_str() {
        "auth-password" | "auth_password" | "oppauthpassword" | "opp_auth_password" => PASSWORD_KEY.to_string(),
        "oppchunk" | "oppchunksize" | "opp-chunk-size" | "opp_chunk_size" => "opp_chunk_size".to_string(),
        "oppconnecttimeout" | "opp-connect-timeout" | "opp_connect_timeout_s" => "opp_connect_timeout_s".to_string(),
        "oppsendtimeout" | "opp-send-idle-timeout" | "opp_send_idle_timeout_s" => "opp_send_idle_timeout_s".to_string(),
        "oppretrycount" | "opp-retry-count" | "opp_retry_count" => "opp_retry_count".to_string(),
        "oppretrydelay" | "opp-retry-delay" | "opp_retry_delay_s" => "opp_retry_delay_s".to_string(),
        "oppprotectionlevel" | "oppprotection" | "opp-protection" | "opp_protection" => "opp_protection".to_string(),
        "oppnameusebom" | "oppbom" | "opp-name-bom" | "opp_name_bom" => "opp_name_bom".to_string(),
        "assistantchunk" | "assistantchunksize" | "assistant-chunk-size" | "assistant_chunk_size" => {
            "assistant_chunk_size".to_string()
        }
        "pushtextfilename" | "push-text-file-name" | "push_text_file_name" => "push_text_file_name".to_string(),
        "receivedir" | "receive-dir" | "receive_dir" => "receive_dir".to_string(),
        "receiveask" | "receive-ask" | "receive_ask" => "receive_ask".to_string(),
        "transfermode" | "mode" => "mode".to_string(),
        other => {
            return Err(
                CliError::usage(format!("未知配置项：{other}")).with_hint("运行 bt config list --json 查看全部合法键")
            );
        }
    };
    Ok(key)
}

fn config_entries(config: &Config) -> Vec<(&'static str, String)> {
    vec![
        ("opp_chunk_size", config.opp_chunk_size.to_string()),
        ("opp_connect_timeout_s", config.opp_connect_timeout_s.to_string()),
        ("opp_send_idle_timeout_s", config.opp_send_idle_timeout_s.to_string()),
        ("opp_retry_count", config.opp_retry_count.to_string()),
        ("opp_retry_delay_s", config.opp_retry_delay_s.to_string()),
        ("opp_protection", config.opp_protection.to_string()),
        ("opp_name_bom", config.opp_name_bom.to_string()),
        ("assistant_chunk_size", config.assistant_chunk_size.to_string()),
        ("push_text_file_name", config.push_text_file_name.clone()),
        ("receive_dir", config.receive_dir.display().to_string()),
        ("receive_ask", config.receive_ask.to_string()),
        ("mode", config.mode.to_string()),
    ]
}

fn key_value(config: &Config, key: &str) -> Result<Value, CliError> {
    let value = match key {
        "opp_chunk_size" => json!(config.opp_chunk_size),
        "opp_connect_timeout_s" => json!(config.opp_connect_timeout_s),
        "opp_send_idle_timeout_s" => json!(config.opp_send_idle_timeout_s),
        "opp_retry_count" => json!(config.opp_retry_count),
        "opp_retry_delay_s" => json!(config.opp_retry_delay_s),
        "opp_protection" => json!(config.opp_protection),
        "opp_name_bom" => json!(config.opp_name_bom),
        "assistant_chunk_size" => json!(config.assistant_chunk_size),
        "push_text_file_name" => json!(config.push_text_file_name),
        "receive_dir" => json!(config.receive_dir.display().to_string()),
        "receive_ask" => json!(config.receive_ask),
        "mode" => json!(config.mode),
        other => return Err(CliError::usage(format!("未知配置项：{other}"))),
    };
    Ok(value)
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn patch_for(key: &str, raw: &str) -> Result<ConfigPatch, CliError> {
    let mut patch = ConfigPatch::default();
    match key {
        "opp_chunk_size" => patch.opp_chunk_size = Some(parse_u32(key, raw)?),
        "opp_connect_timeout_s" => patch.opp_connect_timeout_s = Some(parse_u32(key, raw)?),
        "opp_send_idle_timeout_s" => patch.opp_send_idle_timeout_s = Some(parse_u32(key, raw)?),
        "opp_retry_count" => patch.opp_retry_count = Some(parse_u32(key, raw)?),
        "opp_retry_delay_s" => patch.opp_retry_delay_s = Some(parse_u32(key, raw)?),
        "opp_protection" => {
            patch.opp_protection = Some(parse_enum::<ProtectionLevel>(key, raw, &["auto", "plain", "encrypt"])?)
        }
        "opp_name_bom" => patch.opp_name_bom = Some(parse_bool(key, raw)?),
        "assistant_chunk_size" => patch.assistant_chunk_size = Some(parse_u32(key, raw)?),
        "push_text_file_name" => {
            if raw.trim().is_empty() {
                return Err(CliError::usage("push_text_file_name 不能为空"));
            }
            patch.push_text_file_name = Some(raw.to_string());
        }
        "receive_dir" => {
            if raw.trim().is_empty() {
                return Err(CliError::usage("receive_dir 不能为空"));
            }
            patch.receive_dir = Some(PathBuf::from(raw));
        }
        "receive_ask" => patch.receive_ask = Some(parse_bool(key, raw)?),
        "mode" => patch.mode = Some(parse_enum::<TransferMode>(key, raw, &["auto", "assistant", "opp"])?),
        other => return Err(CliError::usage(format!("未知配置项：{other}"))),
    }
    Ok(patch)
}

fn parse_u32(key: &str, raw: &str) -> Result<u32, CliError> {
    raw.trim()
        .parse::<u32>()
        .map_err(|_| CliError::usage(format!("{key} 需要非负整数，收到：{raw}")))
}

fn parse_bool(key: &str, raw: &str) -> Result<bool, CliError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(CliError::usage(format!("{key} 需要 true 或 false，收到：{raw}"))),
    }
}

fn parse_enum<T: std::str::FromStr<Err = bt_core::BtError>>(
    key: &str,
    raw: &str,
    allowed: &[&'static str],
) -> Result<T, CliError> {
    raw.trim()
        .parse::<T>()
        .map_err(|_| CliError::usage(format!("{key} 需要 {}，收到：{raw}", allowed.join("|"))))
}
