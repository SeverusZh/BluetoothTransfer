use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bt_api::{JobId, JobSnapshot, SendItem};
use bt_core::{BtErrorKind, SendSource};
use crossbeam_channel::RecvTimeoutError;
use serde::Serialize;

use crate::backend::Context;
use crate::cli::{ModeArg, SendFolderArgs, SendPathsArgs, SendTextArgs};
use crate::commands::devices::parse_device;
use crate::error::{CliError, EXIT_FAILED, EXIT_OK};
use crate::output::{emit_event, emit_result, event_to_json, Table};
use crate::signal;

#[derive(Debug, Serialize)]
pub struct SendSummary {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub canceled: usize,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct SendResult {
    pub ok: bool,
    pub jobs: Vec<JobSnapshot>,
    pub summary: SendSummary,
}

pub fn file(ctx: &Context, args: &SendPathsArgs) -> Result<u8, CliError> {
    let device = parse_device(&args.device)?;
    let mut owned: Vec<PathBuf> = Vec::new();
    let result = send_owned_file_items(ctx, device, args, &mut owned);
    for path in &owned {
        bt_app::settle_owned_payload(&ctx.paths, path);
    }
    result
}

fn send_owned_file_items(
    ctx: &Context,
    device: bt_core::DeviceId,
    args: &SendPathsArgs,
    owned: &mut Vec<PathBuf>,
) -> Result<u8, CliError> {
    let mut items = Vec::with_capacity(args.paths.len());
    for path in &args.paths {
        items.push(file_item(ctx, path, args.zip, owned)?);
    }
    send_items(ctx, device, items, args.mode)
}

pub fn text(ctx: &Context, args: &SendTextArgs) -> Result<u8, CliError> {
    let device = parse_device(&args.device)?;
    let text = args.text.join(" ");
    if text.is_empty() {
        return Err(CliError::usage("文本内容不能为空"));
    }
    let mut item = SendItem::from_source(SendSource::Text(text.clone()));
    if let Some(name) = &args.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(CliError::usage("--name 不能为空"));
        }
        item.display_name = trimmed.to_string();
    }
    item.total_bytes = Some(text.len() as u64);
    send_items(ctx, device, vec![item], args.mode)
}

pub fn folder(ctx: &Context, args: &SendFolderArgs) -> Result<u8, CliError> {
    let device = parse_device(&args.device)?;
    if !args.dir.is_dir() {
        return Err(CliError::usage(format!("目录不存在：{}", args.dir.display()))
            .with_hint("确认路径拼写；文件夹会自动打包为 zip 后发送"));
    }
    bt_app::check_zip_folder(&args.dir).map_err(|error| {
        let error = CliError::from(error);
        if error.kind() == BtErrorKind::Unsupported {
            error.with_hint(
                "文件夹累计内容达到 4 GiB 的 ZIP32 上限；请拆分成多个文件夹分批发送，或先自行压缩后用 send file 发送",
            )
        } else {
            error
        }
    })?;
    let item = SendItem::from_source(SendSource::Folder(args.dir.clone()));
    send_items(ctx, device, vec![item], args.mode)
}

pub fn send_items(
    ctx: &Context,
    device: bt_core::DeviceId,
    items: Vec<SendItem>,
    mode: Option<ModeArg>,
) -> Result<u8, CliError> {
    if items.is_empty() {
        return Err(CliError::usage("没有待发送的条目"));
    }
    ctx.drain_events();
    let ids = ctx.backend.enqueue(&device, items, mode.and_then(ModeArg::channel))?;
    let outcome = wait_jobs(ctx, &ids)?;
    output_result(ctx, &outcome);
    Ok(if outcome.ok { EXIT_OK } else { EXIT_FAILED })
}

fn file_item(ctx: &Context, path: &Path, zip_enabled: bool, owned: &mut Vec<PathBuf>) -> Result<SendItem, CliError> {
    if bt_app::is_pending_source(&ctx.paths, path) {
        return Err(
            CliError::usage(format!("源文件位于待续传目录 pending/：{}", path.display()))
                .with_hint("把源文件移出 pending/ 后再发送；该目录只存放本应用生成的待续传载荷"),
        );
    }
    if !path.is_file() {
        return Err(CliError::usage(format!("文件不存在：{}", path.display()))
            .with_hint("确认路径；发送多个文件时每个路径都必须存在"));
    }
    if zip_enabled {
        let packed = bt_app::pack_zip_file_in(&ctx.paths, path).map_err(|error| {
            let error = CliError::from(error);
            if error.kind() == BtErrorKind::Unsupported {
                error.with_hint("单文件 zip 不支持 4 GiB 及以上内容；去掉 --zip 直接发送，或先拆分文件")
            } else {
                error
            }
        })?;
        if packed.owned {
            owned.push(packed.path.clone());
        }
        let size = std::fs::metadata(&packed.path)
            .map_err(|error| CliError::failed(format!("读取 zip 失败：{error}")))?
            .len();
        let produced = packed.owned;
        let mut item = SendItem::from_source(SendSource::File(packed.path));
        item.display_name = packed.display_name;
        item.total_bytes = Some(size);
        item.produced = produced;
        return Ok(item);
    }
    let size = std::fs::metadata(path)
        .map_err(|error| CliError::failed(format!("读取文件失败：{error}")))?
        .len();
    let mut item = SendItem::from_source(SendSource::File(path.to_path_buf()));
    item.total_bytes = Some(size);
    Ok(item)
}

/// 等待一组任务进入终态，期间输出事件与进度。
pub fn wait_jobs(ctx: &Context, ids: &[JobId]) -> Result<SendResult, CliError> {
    let config = ctx.backend.config();
    let idle_timeout = Duration::from_secs(u64::from(config.opp_send_idle_timeout_s.max(30)));
    let mut snapshots: Vec<JobSnapshot> = Vec::new();
    let mut last_event = Instant::now();
    let mut last_percent: HashMap<u64, u8> = HashMap::new();
    let mut interrupted = false;
    refresh_snapshots(ctx, ids, &mut snapshots);
    loop {
        if snapshots.len() == ids.len() && snapshots.iter().all(|job| job.state.is_terminal()) {
            drain_pending_events(ctx);
            break;
        }
        if signal::canceled() && !interrupted {
            interrupted = true;
            let _ = ctx.backend.cancel_all_jobs();
            last_event = Instant::now();
        }
        match ctx.events.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                last_event = Instant::now();
                if interrupted {
                    if let bt_api::Event::JobStateChanged { .. } = &event {
                        emit_event(ctx.mode, event_to_json(&event));
                    }
                    refresh_snapshots(ctx, ids, &mut snapshots);
                    continue;
                }
                emit_event(ctx.mode, event_to_json(&event));
                match &event {
                    bt_api::Event::JobProgress {
                        job_id,
                        bytes,
                        total,
                        bps,
                        eta_seconds,
                    } => {
                        if ctx.mode.is_human() {
                            let percent = if *total == 0 {
                                0
                            } else {
                                ((*bytes).min(*total) as f64 / *total as f64 * 100.0).round() as u8
                            };
                            let changed = last_percent.get(&job_id.0).copied() != Some(percent);
                            if changed {
                                last_percent.insert(job_id.0, percent);
                                let eta = eta_seconds
                                    .map(|seconds| format!("，剩余 {seconds}s"))
                                    .unwrap_or_default();
                                eprint!(
                                    "\r任务 {}：{percent}%  {bytes}/{total} 字节  {bps} B/s{eta}   ",
                                    job_id.0
                                );
                                let _ = std::io::stderr().flush();
                            }
                        }
                    }
                    bt_api::Event::JobStateChanged { job } if ctx.mode.is_human() => {
                        if job.state.is_terminal() {
                            eprintln!();
                        }
                    }
                    bt_api::Event::Log { level, message, .. } if ctx.mode.is_human() => {
                        eprintln!("[{level}] {message}");
                    }
                    _ => {}
                }
                refresh_snapshots(ctx, ids, &mut snapshots);
            }
            Err(RecvTimeoutError::Timeout) => {
                refresh_snapshots(ctx, ids, &mut snapshots);
                if snapshots.len() == ids.len() && snapshots.iter().all(|job| job.state.is_terminal()) {
                    drain_pending_events(ctx);
                    break;
                }
                if interrupted {
                    if last_event.elapsed() >= Duration::from_secs(5) {
                        break;
                    }
                    continue;
                }
                if last_event.elapsed() >= idle_timeout {
                    let _ = ctx.backend.cancel_all_jobs();
                    return Err(CliError::new(BtErrorKind::Timeout, "发送长时间无进度，已取消")
                        .with_hint("检查对端是否在线；可调大 opp_send_idle_timeout_s"));
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                refresh_snapshots(ctx, ids, &mut snapshots);
                if snapshots.len() != ids.len() || !snapshots.iter().all(|job| job.state.is_terminal()) {
                    return Err(CliError::failed("事件通道已关闭，任务状态未知"));
                }
                break;
            }
        }
    }
    if ctx.mode.is_human() {
        eprintln!();
    }
    let summary = summarize(&snapshots);
    let ok = summary.failed == 0 && summary.canceled == 0 && !interrupted;
    Ok(SendResult {
        ok,
        jobs: snapshots,
        summary,
    })
}

fn drain_pending_events(ctx: &Context) {
    while let Ok(event) = ctx.events.try_recv() {
        emit_event(ctx.mode, event_to_json(&event));
    }
}

fn refresh_snapshots(ctx: &Context, ids: &[JobId], snapshots: &mut Vec<JobSnapshot>) {
    let all = ctx.backend.jobs();
    *snapshots = all.into_iter().filter(|job| ids.contains(&job.id)).collect();
}

pub fn summarize(snapshots: &[JobSnapshot]) -> SendSummary {
    let mut summary = SendSummary {
        total: snapshots.len(),
        completed: 0,
        failed: 0,
        canceled: 0,
        bytes: 0,
    };
    for job in snapshots {
        summary.bytes += job.bytes_sent;
        match job.state {
            bt_api::JobState::Completed => summary.completed += 1,
            bt_api::JobState::Failed => summary.failed += 1,
            bt_api::JobState::Canceled => summary.canceled += 1,
            _ => {}
        }
    }
    summary
}

pub fn output_result(ctx: &Context, result: &SendResult) {
    emit_result(ctx.mode, result);
    if ctx.mode.is_human() {
        print_send_human(&result.jobs, &result.summary);
    }
}

fn print_send_human(jobs: &[JobSnapshot], summary: &SendSummary) {
    let mut table = Table::new(&["ID", "名称", "通道", "状态", "进度", "字节", "错误"]);
    table = table.cap(1, 28).cap(5, 12).cap(6, 30);
    for job in jobs {
        table.row([
            job.id.0.to_string(),
            job.item.display_name.clone(),
            job.channel.as_str().to_string(),
            state_label(job.state).to_string(),
            format!("{}%", job.percent()),
            bt_core::format_bytes(job.bytes_sent),
            job.error.clone().unwrap_or_default(),
        ]);
    }
    table.print();
    println!(
        "汇总：{} 个任务，完成 {}，失败 {}，取消 {}，共 {} 字节",
        summary.total, summary.completed, summary.failed, summary.canceled, summary.bytes
    );
}

fn state_label(state: bt_api::JobState) -> &'static str {
    use bt_api::JobState;
    match state {
        JobState::Queued => "排队",
        JobState::Connecting => "连接",
        JobState::Sending => "发送",
        JobState::Retrying => "重试",
        JobState::Paused => "暂停",
        JobState::Completed => "完成",
        JobState::Failed => "失败",
        JobState::Canceled => "取消",
    }
}
