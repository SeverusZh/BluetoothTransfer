use serde::Serialize;

use crate::backend::Context;
use crate::cli::ExportArgs;
use crate::error::{CliError, EXIT_OK};
use crate::output::emit_result;

#[derive(Serialize)]
struct ExportResult {
    ok: bool,
    path: String,
    rows: u64,
    bytes: u64,
    format: &'static str,
}

pub fn run(ctx: &Context, args: &ExportArgs) -> Result<u8, CliError> {
    let format = args.format.format();
    let summary = ctx.backend.export(
        bt_api::RecordQuery {
            limit: None,
            ..Default::default()
        },
        format,
        args.out.clone(),
    )?;
    emit_result(
        ctx.mode,
        &ExportResult {
            ok: true,
            path: summary.path.display().to_string(),
            rows: summary.rows,
            bytes: summary.bytes,
            format: format.extension(),
        },
    );
    if ctx.mode.is_human() {
        println!(
            "已导出 {} 条记录到 {}（{} 字节）",
            summary.rows,
            summary.path.display(),
            summary.bytes
        );
    }
    Ok(EXIT_OK)
}
