use std::path::PathBuf;

use bt_api::MigrateOptions;
use serde::Serialize;

use crate::backend::Context;
use crate::cli::MigrateArgs;
use crate::error::{CliError, EXIT_OK};
use crate::output::emit_result;

#[derive(Serialize)]
struct MigrateResult {
    ok: bool,
    from: String,
    dry_run: bool,
    imported_records: u64,
    imported_config: bool,
    copied_files: u64,
    skipped: u64,
    warnings: Vec<String>,
}

pub fn run(ctx: &Context, args: &MigrateArgs) -> Result<u8, CliError> {
    let root = args.from.clone().unwrap_or_else(|| default_legacy_root(ctx));
    let report = ctx
        .backend
        .migrate_legacy(MigrateOptions {
            legacy_root: root.clone(),
            target_dir: None,
            dry_run: args.dry_run,
        })
        .map_err(|error| {
            use bt_core::BtErrorKind;
            let hint = match error.kind() {
                BtErrorKind::NotFound => "确认 --from 指向旧版数据根；旧版数据通常包含 config.json 与 data.db",
                BtErrorKind::InvalidArgument => "确认目录中是旧版 BluetoothTransfer 数据；可先用 --dry-run 检查",
                _ => return CliError::from(error),
            };
            CliError::new(error.kind(), error.message()).with_hint(hint)
        })?;
    emit_result(
        ctx.mode,
        &MigrateResult {
            ok: true,
            from: root.display().to_string(),
            dry_run: report.dry_run,
            imported_records: report.imported_records,
            imported_config: report.imported_config,
            copied_files: report.copied_files,
            skipped: report.skipped,
            warnings: report.warnings.clone(),
        },
    );
    if ctx.mode.is_human() {
        println!("旧数据目录：{}", root.display());
        println!(
            "导入记录 {} 条，配置 {}，复制文件 {} 个，跳过 {} 个",
            report.imported_records,
            if report.imported_config {
                "已导入"
            } else {
                "未发现"
            },
            report.copied_files,
            report.skipped
        );
        for warning in &report.warnings {
            eprintln!("警告：{warning}");
        }
    }
    Ok(EXIT_OK)
}

fn default_legacy_root(ctx: &Context) -> PathBuf {
    ctx.paths
        .config_file
        .parent()
        .map(|parent| parent.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("BluetoothTransfer"))
}
