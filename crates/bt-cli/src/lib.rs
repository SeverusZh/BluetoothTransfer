//! 命令行前端：bt 提供完整命令集，btrecv 是只做接收的薄壳。
//!
//! 默认特性启用 gui，使 bt 无子命令时打开图形界面；
//! 关闭默认特性后 bt 只保留纯命令行路径，不链接 eframe。

pub mod backend;
pub mod cli;
pub mod commands;
pub mod error;
pub mod output;
pub mod signal;

pub use cli::{BtrecvCli, Cli, Command};

use clap::{CommandFactory, Parser};

use crate::backend::Context;
use crate::cli::BtrecvCommand;
use crate::error::CliError;
use crate::output::OutputMode;

/// bt 进程入口。
pub fn main_entry(args: Vec<String>) -> i32 {
    signal::install();
    let mode = OutputMode::from_args(&args);
    let parsed = match Cli::try_parse_from(std::iter::once("bt".to_string()).chain(args)) {
        Ok(cli) => cli,
        Err(error) => return clap_failure("bt", &error, mode),
    };
    run_cli(parsed)
}

fn run_cli(cli: Cli) -> i32 {
    if cli.command.is_none() {
        let mode = OutputMode::from_flags(cli.json, cli.events);
        return default_command(mode);
    }
    if matches!(&cli.command, Some(Command::Repl)) {
        return commands::repl_entry(&cli);
    }
    let mode = OutputMode::from_flags(cli.json, cli.events);
    let name = cli.command_name();
    if matches!(&cli.command, Some(Command::Gui)) {
        return match run_gui() {
            Ok(code) => i32::from(code),
            Err(error) => i32::from(error.report(mode, name)),
        };
    }
    let auto_resume = cli.command.as_ref().is_some_and(Command::auto_resumes_journal);
    let ctx = if auto_resume {
        Context::open(mode)
    } else {
        Context::open_without_resume(mode)
    };
    let ctx = match ctx {
        Ok(ctx) => ctx,
        Err(error) => return i32::from(error.report(mode, name)),
    };
    let result = match cli.command.as_ref() {
        Some(command) => commands::dispatch(command, &ctx),
        None => Ok(0),
    };
    let code = match result {
        Ok(code) => i32::from(code),
        Err(error) => i32::from(error.report(mode, name)),
    };
    ctx.shutdown();
    code
}

/// btrecv 进程入口：无参即监听，其余参数与 bt recv 一致。
pub fn recv_entry(args: Vec<String>) -> i32 {
    signal::install();
    let mode = OutputMode::from_args(&args);
    let parsed = match BtrecvCli::try_parse_from(std::iter::once("btrecv".to_string()).chain(args)) {
        Ok(cli) => cli,
        Err(error) => return clap_failure("btrecv", &error, mode),
    };
    if let Some(command) = &parsed.command {
        match command {
            BtrecvCommand::Help => {
                let mut help = BtrecvCli::command();
                let _ = help.print_help();
                println!();
                return 0;
            }
            BtrecvCommand::Version => {
                println!("btrecv {}", env!("CARGO_PKG_VERSION"));
                return 0;
            }
            BtrecvCommand::Recv(_) => {}
        }
    }
    let recv_args = match &parsed.command {
        Some(BtrecvCommand::Recv(args)) => args.clone(),
        _ => parsed.direct.clone(),
    };
    let mode = OutputMode::from_flags(parsed.json, parsed.events);
    let ctx = match Context::open(mode) {
        Ok(ctx) => ctx,
        Err(error) => return i32::from(error.report(mode, "recv")),
    };
    let code = match commands::recv::run(&ctx, &recv_args) {
        Ok(code) => i32::from(code),
        Err(error) => i32::from(error.report(mode, "recv")),
    };
    ctx.shutdown();
    code
}

fn default_command(mode: OutputMode) -> i32 {
    #[cfg(feature = "gui")]
    {
        match run_gui() {
            Ok(code) => i32::from(code),
            Err(error) => i32::from(error.report(mode, "gui")),
        }
    }
    #[cfg(not(feature = "gui"))]
    {
        i32::from(
            CliError::usage("当前构建为 cli-only，未启用图形界面")
                .with_hint("使用 bt --help 查看命令行子命令")
                .report(mode, "bt"),
        )
    }
}

fn run_gui() -> Result<u8, CliError> {
    #[cfg(feature = "gui")]
    {
        bt_gui::main_entry(Vec::new()).map_err(gui_error)
    }
    #[cfg(not(feature = "gui"))]
    {
        Err(CliError::failed("当前构建为 cli-only，未启用图形界面")
            .with_hint("安装带 GUI 的发行包，或使用 bt --help 查看命令行能力"))
    }
}

#[cfg(feature = "gui")]
fn gui_error(error: bt_core::BtError) -> CliError {
    let error = CliError::from(error);
    let hint = if error.kind() == bt_core::BtErrorKind::Busy {
        "已有实例在运行；关闭旧窗口后重试，或直接使用 CLI 子命令"
    } else {
        "无显示环境时无法打开窗口；请改用 CLI 子命令"
    };
    error.with_hint(hint)
}

fn clap_failure(program: &str, error: &clap::Error, mode: OutputMode) -> i32 {
    let code = clap_error_code(error);
    if code == 0 {
        let _ = error.print();
        return 0;
    }
    if mode.is_human() {
        let _ = error.print();
        return code;
    }
    let error = CliError::usage(clap_message(error)).with_hint(format!("运行 {program} --help 查看用法"));
    i32::from(error.report(mode, program))
}

/// 取 clap 错误首行，去掉 clap 自带的错误前缀。
pub(crate) fn clap_message(error: &clap::Error) -> String {
    error
        .to_string()
        .lines()
        .next()
        .unwrap_or("用法错误")
        .trim_start_matches("error: ")
        .to_string()
}

fn clap_error_code(error: &clap::Error) -> i32 {
    match error.kind() {
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => 0,
        _ => 2,
    }
}

#[cfg(all(test, feature = "gui"))]
mod tests {
    use super::*;

    #[test]
    fn gui_error_hint_follows_error_kind() {
        let busy = gui_error(bt_core::BtError::busy("已有实例在运行，但无法转交启动参数"));
        assert_eq!(busy.kind(), bt_core::BtErrorKind::Busy);
        let hint = busy.hint_text().unwrap_or_default();
        assert!(hint.contains("已有实例"), "busy hint 不得写无显示环境：{hint}");

        let window = gui_error(bt_core::BtError::internal("窗口创建失败"));
        assert!(window.hint_text().unwrap_or_default().contains("显示环境"));
    }
}
