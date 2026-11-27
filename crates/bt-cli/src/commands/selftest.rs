use serde::Serialize;

use crate::backend::Context;
use crate::error::{CliError, EXIT_FAILED, EXIT_OK};
use crate::output::emit_result;

#[derive(Serialize)]
struct SelftestResult {
    ok: bool,
    checks: Vec<bt_api::SelftestCheck>,
    failed: Vec<String>,
}

pub fn run(ctx: &Context) -> Result<u8, CliError> {
    let report = ctx.backend.selftest()?;
    let ok = report.passed();
    let failed: Vec<String> = report.failed_names().into_iter().map(|name| name.to_string()).collect();
    emit_result(
        ctx.mode,
        &SelftestResult {
            ok,
            checks: report.checks.clone(),
            failed: failed.clone(),
        },
    );
    if ctx.mode.is_human() {
        for check in &report.checks {
            let mark = if check.ok { "ok" } else { "fail" };
            println!("[{mark}] {}: {}", check.name, check.detail);
        }
        if ok {
            println!("自检通过：{} 项", report.checks.len());
        } else {
            println!("自检未通过：{}", failed.join(", "));
        }
    }
    Ok(if ok { EXIT_OK } else { EXIT_FAILED })
}
