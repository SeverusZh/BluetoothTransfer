use std::path::PathBuf;

use bt_api::{JobState, SendItem};
use bt_core::{Channel, DeviceId, SendSource};
use serde::Serialize;

use crate::backend::Context;
use crate::cli::ResumeCommand;
use crate::commands::send::{summarize, wait_jobs, SendSummary};
use crate::error::{CliError, EXIT_FAILED, EXIT_OK};
use crate::output::{emit_result, Table};

#[derive(Clone, Serialize)]
struct ResumeEntry {
    id: String,
    peer: String,
    display_name: String,
    payload: String,
    size: u64,
    state: String,
    attempts: u32,
    updated_at: String,
}

#[derive(Serialize)]
struct ResumeListResult {
    ok: bool,
    journal_dir: String,
    entries: Vec<ResumeEntry>,
    count: usize,
}

#[derive(Serialize)]
struct ResumeRunResult {
    ok: bool,
    resumed: usize,
    jobs: Vec<bt_api::JobSnapshot>,
    summary: SendSummary,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ResumeCancelResult {
    ok: bool,
    canceled: usize,
}

pub fn dispatch(ctx: &Context, action: &ResumeCommand) -> Result<u8, CliError> {
    match action {
        ResumeCommand::List => list(ctx),
        ResumeCommand::Run => run(ctx),
        ResumeCommand::Cancel => cancel(ctx),
    }
}

pub fn list(ctx: &Context) -> Result<u8, CliError> {
    let infos = bt_app::list_resume(&ctx.paths).map_err(CliError::from)?;
    let entries: Vec<ResumeEntry> = infos
        .into_iter()
        .map(|info| ResumeEntry {
            id: info.id,
            peer: info.peer,
            display_name: info.display_name,
            payload: info.payload.display().to_string(),
            size: info.size,
            state: info.state.to_string(),
            attempts: info.attempts,
            updated_at: info.updated_at,
        })
        .collect();
    emit_result(
        ctx.mode,
        &ResumeListResult {
            ok: true,
            journal_dir: ctx.paths.journal_dir.display().to_string(),
            count: entries.len(),
            entries: entries.clone(),
        },
    );
    if ctx.mode.is_human() {
        if entries.is_empty() {
            println!("journal 中没有续传条目：{}", ctx.paths.journal_dir.display());
        } else {
            let mut table = Table::new(&["ID", "对端", "名称", "大小", "状态", "尝试", "更新时间"]);
            table = table.cap(1, 17).cap(2, 28).cap(6, 20);
            for entry in &entries {
                table.row([
                    entry.id.clone(),
                    entry.peer.clone(),
                    entry.display_name.clone(),
                    bt_core::format_bytes(entry.size),
                    entry.state.clone(),
                    entry.attempts.to_string(),
                    entry.updated_at.clone(),
                ]);
            }
            table.print();
        }
    }
    Ok(EXIT_OK)
}

pub fn run(ctx: &Context) -> Result<u8, CliError> {
    let plan = bt_app::resume_plan(&ctx.paths).map_err(CliError::from)?;
    let mut warnings: Vec<String> = plan
        .abandoned
        .iter()
        .map(|item| format!("放弃续传 {}：{}", item.info.id, item.reason))
        .collect();
    for entry in &plan.busy {
        warnings.push(format!("续传条目 {} 正在其他进程发送，已跳过", entry.id));
    }
    if !plan.abandoned.is_empty() {
        let ids: Vec<String> = plan.abandoned.iter().map(|item| item.info.id.clone()).collect();
        bt_app::drop_resume(&ctx.paths, &ids).map_err(CliError::from)?;
    }
    let mut queued: Vec<(String, bt_api::JobId)> = Vec::new();
    for info in &plan.recoverable {
        let device = match DeviceId::parse(&info.peer) {
            Ok(device) => device,
            Err(_) => {
                warnings.push(format!("续传条目 {} 的对端地址非法：{}", info.id, info.peer));
                continue;
            }
        };
        let payload = PathBuf::from(&info.payload);
        let mut item = SendItem::from_source(SendSource::File(payload));
        item.display_name = info.display_name.clone();
        item.total_bytes = Some(info.size);
        match ctx.backend.enqueue(&device, vec![item], Some(Channel::Assistant)) {
            Ok(job_ids) => {
                if let Some(job_id) = job_ids.first() {
                    queued.push((info.id.clone(), *job_id));
                }
            }
            Err(error) => warnings.push(format!("续传条目 {} 入队失败：{}", info.id, error.message())),
        }
    }
    if queued.is_empty() {
        let ok = warnings.is_empty();
        let result = ResumeRunResult {
            ok,
            resumed: 0,
            jobs: Vec::new(),
            summary: summarize(&[]),
            warnings: warnings.clone(),
        };
        emit_result(ctx.mode, &result);
        if ctx.mode.is_human() {
            if plan.recoverable.is_empty() && warnings.is_empty() {
                println!("没有可续传的记录");
            } else {
                println!("没有任务可以续传");
            }
            for warning in &warnings {
                eprintln!("警告：{warning}");
            }
        }
        return Ok(if ok { EXIT_OK } else { EXIT_FAILED });
    }
    let ids: Vec<bt_api::JobId> = queued.iter().map(|(_, id)| *id).collect();
    let outcome = wait_jobs(ctx, &ids)?;
    for (entry_id, job_id) in &queued {
        if outcome
            .jobs
            .iter()
            .find(|job| job.id == *job_id)
            .is_some_and(|job| job.state == JobState::Completed)
        {
            let _ = bt_app::cancel_resume(&ctx.paths, entry_id);
        }
    }
    let result = ResumeRunResult {
        ok: outcome.ok,
        resumed: ids.len(),
        jobs: outcome.jobs,
        summary: outcome.summary,
        warnings: warnings.clone(),
    };
    emit_result(ctx.mode, &result);
    if ctx.mode.is_human() {
        println!(
            "续传任务：{} 个，完成 {}，失败 {}，取消 {}",
            result.resumed, result.summary.completed, result.summary.failed, result.summary.canceled
        );
        for warning in &warnings {
            eprintln!("警告：{warning}");
        }
    }
    Ok(if result.ok { EXIT_OK } else { EXIT_FAILED })
}

pub fn cancel(ctx: &Context) -> Result<u8, CliError> {
    let infos = bt_app::list_resume(&ctx.paths).map_err(CliError::from)?;
    let mut canceled = 0usize;
    for info in infos {
        if bt_app::cancel_resume(&ctx.paths, &info.id).map_err(CliError::from)? {
            canceled += 1;
        }
    }
    emit_result(ctx.mode, &ResumeCancelResult { ok: true, canceled });
    if ctx.mode.is_human() {
        println!("已取消 {canceled} 条续传条目");
    }
    Ok(EXIT_OK)
}
