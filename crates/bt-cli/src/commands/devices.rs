use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use bt_api::{Device, Event, PinPromptKind, ScanOptions};
use bt_core::{AssistantState, BtErrorKind, DeviceId};
use crossbeam_channel::RecvTimeoutError;
use serde::Serialize;

use crate::backend::{find_device, Context};
use crate::cli::{AliasArgs, DetectArgs, FavoriteArgs, ListArgs, PairArgs, ScanArgs, SortArg};
use crate::error::{CliError, EXIT_FAILED, EXIT_NOT_FOUND, EXIT_OK};
use crate::output::{emit_event, emit_result, event_to_json, Table};
use crate::signal;

#[derive(Serialize)]
struct DevicesResult {
    ok: bool,
    devices: Vec<Device>,
    count: usize,
}

#[derive(Serialize)]
struct PairResult {
    ok: bool,
    address: String,
    paired: bool,
    device: Option<Device>,
}

#[derive(Serialize)]
struct AliasResult {
    ok: bool,
    address: String,
    alias: Option<String>,
    device: Option<Device>,
}

#[derive(Serialize)]
struct FavoriteResult {
    ok: bool,
    address: String,
    favorite: bool,
    device: Option<Device>,
}

#[derive(Serialize)]
struct ScanResult {
    ok: bool,
    devices: Vec<Device>,
    count: usize,
    paired_only: bool,
    duration_s: Option<u32>,
}

#[derive(Serialize)]
struct DetectResult {
    ok: bool,
    address: String,
    state: &'static str,
    device: Option<Device>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub fn list(ctx: &Context, args: &ListArgs) -> Result<u8, CliError> {
    let mut devices = ctx.backend.devices();
    sort_devices(&mut devices, args.sort);
    emit_result(
        ctx.mode,
        &DevicesResult {
            ok: true,
            count: devices.len(),
            devices: devices.clone(),
        },
    );
    if ctx.mode.is_human() {
        if devices.is_empty() {
            println!("无设备缓存；运行 bt devices scan 扫描设备");
        } else {
            print_devices(&devices);
        }
    }
    Ok(EXIT_OK)
}

pub fn scan(ctx: &Context, args: &ScanArgs) -> Result<u8, CliError> {
    if args.seconds == Some(0) {
        return Err(CliError::usage("--seconds 必须大于 0"));
    }
    if ctx.mode.is_human() {
        match args.seconds {
            Some(seconds) => eprintln!("正在扫描设备（最多 {seconds} 秒）..."),
            None => eprintln!("正在扫描设备..."),
        }
    }
    ctx.drain_events();
    ctx.backend.start_scan(ScanOptions {
        duration_s: args.seconds,
        refresh_paired: false,
    })?;
    let wait = Duration::from_secs(u64::from(args.seconds.unwrap_or(15)) + 30);
    let deadline = Instant::now() + wait;
    let mut finished = false;
    loop {
        if signal::canceled() {
            let _ = ctx.backend.cancel_scan();
            return Err(CliError::canceled("扫描已取消"));
        }
        match ctx.events.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                let done = matches!(event, Event::ScanFinished { .. });
                emit_event(ctx.mode, event_to_json(&event));
                if done {
                    finished = true;
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if !finished {
        return Err(CliError::new(BtErrorKind::Timeout, "扫描未在预期时间内结束")
            .with_hint("检查蓝牙适配器与 BlueZ 服务；无硬件时可用 BT_BACKEND=mock"));
    }
    let mut devices = ctx.backend.devices();
    if args.paired_only {
        devices.retain(|device| device.paired);
    }
    devices.sort_by(|left, right| {
        right
            .paired
            .cmp(&left.paired)
            .then_with(|| left.label().to_lowercase().cmp(&right.label().to_lowercase()))
    });
    emit_result(
        ctx.mode,
        &ScanResult {
            ok: true,
            count: devices.len(),
            paired_only: args.paired_only,
            duration_s: args.seconds,
            devices: devices.clone(),
        },
    );
    if ctx.mode.is_human() {
        println!("扫描完成：{} 台设备", devices.len());
        if devices.is_empty() {
            println!("未发现设备；确认对端已开启蓝牙并处于可发现状态");
        } else {
            print_devices(&devices);
        }
    }
    Ok(EXIT_OK)
}

pub fn pair(ctx: &Context, args: &PairArgs) -> Result<u8, CliError> {
    let id = parse_device(&args.address)?;
    ctx.drain_events();
    ctx.backend.pair(&id)?;
    let interactive = std::io::stdin().is_terminal();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if signal::canceled() {
            return Err(CliError::canceled("配对被取消"));
        }
        match ctx.events.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                emit_event(ctx.mode, event_to_json(&event));
                match event {
                    Event::PairPrompt { request, device, kind } => {
                        if ctx.mode.is_human() {
                            eprintln!("配对请求：{}（{}）", device.label(), prompt_label(&kind));
                        }
                        match decide_pair(&kind, args.pin.as_deref(), interactive)? {
                            PairReply::Accept => ctx.backend.answer_prompt(request.id, true)?,
                            PairReply::Pin(pin) => {
                                if ctx.mode.is_human() {
                                    eprintln!("已提交 PIN，长度 {} 位", pin.chars().count());
                                }
                                ctx.backend.answer_pin(request.id, &pin)?;
                            }
                            PairReply::Reject => {
                                let _ = ctx.backend.answer_prompt(request.id, false);
                                return Err(CliError::failed("配对请求被拒绝"));
                            }
                        }
                    }
                    Event::PairResult { ok, error, .. } => {
                        if !ok {
                            return Err(CliError::failed(error.unwrap_or_else(|| "配对失败".to_string())));
                        }
                        let device = find_device(ctx.backend.as_ref(), id);
                        emit_result(
                            ctx.mode,
                            &PairResult {
                                ok: true,
                                address: id.to_string(),
                                paired: true,
                                device: device.clone(),
                            },
                        );
                        if ctx.mode.is_human() {
                            println!("配对成功：{id}");
                        }
                        return Ok(EXIT_OK);
                    }
                    _ => {}
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    return Err(CliError::new(BtErrorKind::Timeout, "配对超时，未收到配对结果")
                        .with_hint("确认对端在线；必要时先在系统蓝牙设置中完成配对"));
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(CliError::failed("事件通道已关闭，配对未完成"));
            }
        }
    }
}

pub fn alias(ctx: &Context, args: &AliasArgs) -> Result<u8, CliError> {
    let id = parse_device(&args.address)?;
    let joined = args.alias.join(" ");
    let trimmed = joined.trim();
    let value = if trimmed.is_empty() { None } else { Some(trimmed) };
    ctx.backend.set_alias(&id, value)?;
    let device = find_device(ctx.backend.as_ref(), id);
    let alias = value.map(|text| text.to_string());
    emit_result(
        ctx.mode,
        &AliasResult {
            ok: true,
            address: id.to_string(),
            alias: alias.clone(),
            device: device.clone(),
        },
    );
    if ctx.mode.is_human() {
        match alias {
            Some(alias) => println!("已设置别名：{id} = {alias}"),
            None => println!("已清除别名：{id}"),
        }
    }
    Ok(EXIT_OK)
}

pub fn favorite(ctx: &Context, args: &FavoriteArgs) -> Result<u8, CliError> {
    let id = parse_device(&args.address)?;
    let favorite = !args.unset;
    ctx.backend.set_favorite(&id, favorite)?;
    let device = find_device(ctx.backend.as_ref(), id);
    emit_result(
        ctx.mode,
        &FavoriteResult {
            ok: true,
            address: id.to_string(),
            favorite,
            device: device.clone(),
        },
    );
    if ctx.mode.is_human() {
        if favorite {
            println!("已收藏：{id}");
        } else {
            println!("已取消收藏：{id}");
        }
    }
    Ok(EXIT_OK)
}

pub fn detect(ctx: &Context, args: &DetectArgs) -> Result<u8, CliError> {
    let id = parse_device(&args.address)?;
    ctx.drain_events();
    let mut failure = None;
    if let Err(error) = ctx.backend.detect(&id) {
        match error.kind() {
            BtErrorKind::Permission | BtErrorKind::InvalidArgument => {
                return Err(CliError::from(error));
            }
            _ => failure = Some(CliError::from(error)),
        }
    }
    let mut state = if failure.is_some() {
        AssistantState::Unknown
    } else {
        AssistantState::Probing
    };
    if failure.is_none() {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(device) = find_device(ctx.backend.as_ref(), id) {
                state = device.assistant_state;
            }
            if matches!(state, AssistantState::Available | AssistantState::Unavailable) {
                break;
            }
            if Instant::now() >= deadline {
                state = AssistantState::Unknown;
                break;
            }
            match ctx.events.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => {
                    emit_event(ctx.mode, event_to_json(&event));
                    if let Event::AssistantStateChanged { device_id, .. } = event {
                        if device_id == id {
                            if let Some(device) = find_device(ctx.backend.as_ref(), id) {
                                state = device.assistant_state;
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    state = AssistantState::Unknown;
                    break;
                }
            }
        }
    }
    let (label, code) = detect_outcome(state);
    let device = find_device(ctx.backend.as_ref(), id);
    emit_result(
        ctx.mode,
        &DetectResult {
            ok: label == "present",
            address: id.to_string(),
            state: label,
            device: device.clone(),
            error: failure.as_ref().map(|error| error.message().to_string()),
        },
    );
    if ctx.mode.is_human() {
        match state {
            AssistantState::Available => println!("设备 {id}：对端助手通道可用（present）"),
            AssistantState::Unavailable => println!("设备 {id}：对端助手通道不可用（absent）"),
            _ => println!("设备 {id}：助手通道状态未知（unknown）"),
        }
        if let Some(error) = &failure {
            eprintln!("错误：{}", error.message());
        }
    }
    Ok(code)
}

fn detect_outcome(state: AssistantState) -> (&'static str, u8) {
    match state {
        AssistantState::Available => ("present", EXIT_OK),
        AssistantState::Unavailable => ("absent", EXIT_FAILED),
        _ => ("unknown", EXIT_NOT_FOUND),
    }
}

#[derive(Debug)]
enum PairReply {
    Accept,
    Reject,
    Pin(String),
}

fn decide_pair(kind: &PinPromptKind, pin: Option<&str>, interactive: bool) -> Result<PairReply, CliError> {
    if let Some(pin) = pin {
        let pin = validate_pin(pin)?;
        return Ok(match kind {
            PinPromptKind::Pin => PairReply::Pin(pin),
            _ => PairReply::Accept,
        });
    }
    if !interactive {
        return match kind {
            PinPromptKind::Pin => Err(CliError::usage("对端要求 PIN，请用 --pin 提供")
                .with_hint("示例：bt devices pair AA:BB:CC:DD:EE:FF --pin 1234 --json")),
            _ => Ok(PairReply::Accept),
        };
    }
    let prompt = match kind {
        PinPromptKind::Pin => "输入对端显示的 PIN：".to_string(),
        PinPromptKind::Passkey(code) => format!("对端匹配口令 {code}，确认配对？[y/N] "),
        PinPromptKind::Confirm => "确认配对请求？[y/N] ".to_string(),
    };
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let read = std::io::stdin().read_line(&mut answer).map_err(CliError::from)?;
    if read == 0 {
        return Ok(PairReply::Reject);
    }
    let answer = answer.trim();
    if matches!(kind, PinPromptKind::Pin) {
        if answer.is_empty() {
            return Ok(PairReply::Reject);
        }
        return Ok(PairReply::Pin(validate_pin(answer)?));
    }
    let accepted = answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");
    Ok(if accepted { PairReply::Accept } else { PairReply::Reject })
}

fn validate_pin(pin: &str) -> Result<String, CliError> {
    let pin = pin.trim();
    if pin.is_empty() {
        return Err(CliError::usage("--pin 不能为空"));
    }
    if pin.chars().count() > 16 {
        return Err(CliError::usage("--pin 最多 16 个字符"));
    }
    Ok(pin.to_string())
}

fn prompt_label(kind: &PinPromptKind) -> String {
    match kind {
        PinPromptKind::Pin => "需要 PIN".to_string(),
        PinPromptKind::Confirm => "需要确认".to_string(),
        PinPromptKind::Passkey(code) => format!("匹配口令 {code}"),
    }
}

pub fn parse_device(text: &str) -> Result<DeviceId, CliError> {
    DeviceId::parse(text).map_err(|_| {
        CliError::usage(format!("蓝牙地址无效：{text}"))
            .with_hint("地址形如 AA:BB:CC:DD:EE:FF；可先用 bt devices scan 获取")
    })
}

fn sort_devices(devices: &mut [Device], sort: SortArg) {
    match sort {
        SortArg::Last => devices.sort_by(|left, right| {
            match (left.last_seen.as_deref(), right.last_seen.as_deref()) {
                (Some(a), Some(b)) => b.cmp(a),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => left.label().to_lowercase().cmp(&right.label().to_lowercase()),
            }
            .then_with(|| left.label().to_lowercase().cmp(&right.label().to_lowercase()))
        }),
        SortArg::Name => devices.sort_by_key(|device| device.label().to_lowercase()),
        SortArg::Favorite => devices.sort_by(|left, right| {
            right
                .favorite
                .cmp(&left.favorite)
                .then_with(|| left.label().to_lowercase().cmp(&right.label().to_lowercase()))
        }),
    }
}

fn print_devices(devices: &[Device]) {
    let mut table = Table::new(&["地址", "名称", "别名", "配对", "收藏", "助手", "最近发现"]);
    table = table.cap(1, 24).cap(2, 16).cap(6, 20);
    for device in devices {
        table.row([
            device.id.to_string(),
            device.name.clone(),
            device.alias.clone().unwrap_or_default(),
            yes_no(device.paired),
            yes_no(device.favorite),
            assistant_label(device.assistant_state).to_string(),
            device.last_seen.clone().unwrap_or_else(|| "-".to_string()),
        ]);
    }
    table.print();
}

fn yes_no(value: bool) -> String {
    if value {
        "是".to_string()
    } else {
        "否".to_string()
    }
}

pub fn assistant_label(state: AssistantState) -> &'static str {
    match state {
        AssistantState::Unknown => "未知",
        AssistantState::Probing => "探测中",
        AssistantState::Available => "可用",
        AssistantState::Unavailable => "不可用",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EXIT_USAGE;

    #[test]
    fn detect_maps_three_states_to_exit_codes() {
        assert_eq!(detect_outcome(AssistantState::Available), ("present", EXIT_OK));
        assert_eq!(detect_outcome(AssistantState::Unavailable), ("absent", EXIT_FAILED));
        assert_eq!(detect_outcome(AssistantState::Unknown), ("unknown", EXIT_NOT_FOUND));
        assert_eq!(detect_outcome(AssistantState::Probing), ("unknown", EXIT_NOT_FOUND));
    }

    #[test]
    fn invalid_address_is_usage_error() {
        let error = parse_device("not-an-address").unwrap_err();
        assert_eq!(error.exit_code(), EXIT_USAGE);
    }

    #[test]
    fn pin_prompt_requires_pin_in_noninteractive_mode() {
        let error = decide_pair(&PinPromptKind::Pin, None, false).unwrap_err();
        assert_eq!(error.exit_code(), EXIT_USAGE);
        assert!(error.message().contains("--pin"));
        assert!(matches!(
            decide_pair(&PinPromptKind::Pin, Some("1234"), false).unwrap(),
            PairReply::Pin(pin) if pin == "1234"
        ));
    }

    #[test]
    fn pin_flag_does_not_override_confirm_prompts() {
        assert!(matches!(
            decide_pair(&PinPromptKind::Confirm, Some("1234"), false).unwrap(),
            PairReply::Accept
        ));
        assert!(matches!(
            decide_pair(&PinPromptKind::Passkey(123456), None, false).unwrap(),
            PairReply::Accept
        ));
        assert!(decide_pair(&PinPromptKind::Pin, Some("   "), false).is_err());
        assert!(decide_pair(&PinPromptKind::Pin, Some("12345678901234567"), false).is_err());
    }
}
