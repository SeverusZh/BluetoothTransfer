use bt_api::{RecordQuery, Stats};
use bt_core::Record;
use serde::Serialize;

use crate::backend::Context;
use crate::cli::{RecordFilterArgs, RecordsCommand};
use crate::commands::devices::parse_device;
use crate::error::{CliError, EXIT_OK};
use crate::output::{emit_result, Table};

#[derive(Serialize)]
struct RecordsResult {
    ok: bool,
    records: Vec<RecordView>,
    count: usize,
    limit: u32,
    offset: u32,
}

/// CLI 记录视图：文本正文不进入 stdout，校验和转十六进制。
#[derive(Serialize)]
struct RecordView {
    id: i64,
    direction: String,
    channel: String,
    kind: String,
    status: String,
    device_id: Option<String>,
    device_name: String,
    display_name: String,
    path: Option<String>,
    checksum: Option<String>,
    byte_size: u64,
    transferred: u64,
    error: Option<String>,
    started_at: String,
    finished_at: Option<String>,
    resumable: bool,
}

impl From<&Record> for RecordView {
    fn from(record: &Record) -> Self {
        Self {
            id: record.id,
            direction: record.direction.as_str().to_string(),
            channel: record.channel.as_str().to_string(),
            kind: record.kind.as_str().to_string(),
            status: record.status.as_str().to_string(),
            device_id: record.device_id.map(|id| id.to_string()),
            device_name: record.device_name.clone(),
            display_name: record.display_name.clone(),
            path: record.path.as_ref().map(|path| path.display().to_string()),
            checksum: record.checksum.as_ref().map(|value| bt_core::hex_lower(value)),
            byte_size: record.byte_size,
            transferred: record.transferred,
            error: record.error.clone(),
            started_at: record.started_at.clone(),
            finished_at: record.finished_at.clone(),
            resumable: record.resumable,
        }
    }
}

#[derive(Serialize)]
struct DeleteResult {
    ok: bool,
    id: i64,
    deleted: bool,
}

#[derive(Serialize)]
struct ClearResult {
    ok: bool,
    deleted: u64,
}

#[derive(Serialize)]
struct StatsResult {
    ok: bool,
    stats: Stats,
}

pub fn dispatch(ctx: &Context, action: Option<&RecordsCommand>) -> Result<u8, CliError> {
    match action {
        None | Some(RecordsCommand::List(_)) => {
            let filters = match action {
                Some(RecordsCommand::List(filters)) => filters.clone(),
                _ => RecordFilterArgs::default(),
            };
            list(ctx, &filters)
        }
        Some(RecordsCommand::Delete { id }) => delete(ctx, *id),
        Some(RecordsCommand::Clear { yes }) => clear(ctx, *yes),
        Some(RecordsCommand::Stats) => stats(ctx),
    }
}

pub fn list(ctx: &Context, filters: &RecordFilterArgs) -> Result<u8, CliError> {
    if filters.limit == 0 {
        return Err(CliError::usage("--limit 必须大于 0"));
    }
    let query = build_query(filters)?;
    let records = ctx.backend.records(query.clone())?;
    emit_result(
        ctx.mode,
        &RecordsResult {
            ok: true,
            count: records.len(),
            records: records.iter().map(RecordView::from).collect(),
            limit: query.limit.unwrap_or(0),
            offset: query.offset.unwrap_or(0),
        },
    );
    if ctx.mode.is_human() {
        if records.is_empty() {
            println!("无匹配记录；用 bt records list 不带筛选查看全部");
        } else {
            print_records(&records);
        }
    }
    Ok(EXIT_OK)
}

pub fn build_query(filters: &RecordFilterArgs) -> Result<RecordQuery, CliError> {
    let device_id = match &filters.peer {
        Some(text) => Some(parse_device(text)?),
        None => None,
    };
    Ok(RecordQuery {
        direction: filters.direction.map(|value| value.direction()),
        channel: None,
        kind: filters.kind.map(|value| value.kind()),
        status: filters.status.map(|value| value.status()),
        device_id,
        search: filters.search.clone(),
        from: None,
        to: None,
        limit: Some(filters.limit),
        offset: Some(filters.offset),
    })
}

pub fn delete(ctx: &Context, id: i64) -> Result<u8, CliError> {
    let deleted = ctx.backend.delete_record(id)?;
    if !deleted {
        return Err(
            CliError::new(bt_core::BtErrorKind::NotFound, format!("记录不存在：{id}"))
                .with_hint("用 bt records list 查看记录编号"),
        );
    }
    emit_result(ctx.mode, &DeleteResult { ok: true, id, deleted });
    if ctx.mode.is_human() {
        println!("已删除记录 {id}");
    }
    Ok(EXIT_OK)
}

pub fn clear(ctx: &Context, yes: bool) -> Result<u8, CliError> {
    if !yes {
        return Err(CliError::usage("清空记录必须显式传入 --yes")
            .with_hint("该操作不可恢复；先确认可运行 bt records clear --yes"));
    }
    let deleted = ctx.backend.clear_records()?;
    emit_result(ctx.mode, &ClearResult { ok: true, deleted });
    if ctx.mode.is_human() {
        println!("已清空 {deleted} 条记录");
    }
    Ok(EXIT_OK)
}

pub fn stats(ctx: &Context) -> Result<u8, CliError> {
    let stats = ctx.backend.stats()?;
    emit_result(
        ctx.mode,
        &StatsResult {
            ok: true,
            stats: stats.clone(),
        },
    );
    if ctx.mode.is_human() {
        println!(
            "总记录：{}    发送：{}    接收：{}    失败：{}",
            stats.total, stats.sent, stats.received, stats.failed
        );
        println!(
            "已发送：{}    已接收：{}    最近活动：{}",
            bt_core::format_bytes(stats.bytes_sent),
            bt_core::format_bytes(stats.bytes_received),
            stats.last_activity.clone().unwrap_or_else(|| "-".to_string())
        );
        if !stats.days.is_empty() {
            let mut table = Table::new(&["日期", "发送", "接收", "字节"]);
            for day in &stats.days {
                table.row([
                    day.date.clone(),
                    day.sent.to_string(),
                    day.received.to_string(),
                    bt_core::format_bytes(day.bytes),
                ]);
            }
            table.print();
        }
    }
    Ok(EXIT_OK)
}

fn print_records(records: &[Record]) {
    let mut table = Table::new(&["ID", "时间", "方向", "类型", "状态", "对端", "名称", "大小", "通道"]);
    table = table.cap(1, 20).cap(5, 16).cap(6, 28).cap(8, 12);
    for record in records {
        table.row([
            record.id.to_string(),
            record.started_at.clone(),
            direction_label(record.direction),
            record.kind.as_str().to_string(),
            record.status.as_str().to_string(),
            record.device_name.clone(),
            record.display_name.clone(),
            bt_core::format_bytes(record.byte_size),
            record.channel.as_str().to_string(),
        ]);
    }
    table.print();
}

fn direction_label(direction: bt_core::Direction) -> String {
    match direction {
        bt_core::Direction::Send => "发送",
        bt_core::Direction::Receive => "接收",
    }
    .to_string()
}
