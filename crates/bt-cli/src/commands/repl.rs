use std::io::{BufRead, IsTerminal, Write};

use clap::Parser;
use serde::Serialize;

use crate::backend::Context;
use crate::clap_message;
use crate::cli::{Cli, Command};
use crate::error::CliError;
use crate::output::{emit_result, OutputMode};
use crate::signal;

const REPL_COMMANDS: &[&str] = &[
    "devices list|scan|pair|alias|favorite|detect",
    "send file|files|text|folder <ADDR> ...",
    "records list|delete|clear|stats",
    "export <csv|jsonl> <OUT>",
    "config list|get|set",
    "resume list|run|cancel",
    "selftest",
    "help 或 ? 显示本帮助",
    "quit 或 exit 退出",
];

#[derive(Serialize)]
struct ReplHelp {
    ok: bool,
    command: &'static str,
    commands: Vec<&'static str>,
    note: &'static str,
}

pub fn run(cli: &Cli) -> i32 {
    let mode = OutputMode::from_flags(cli.json, cli.events);
    let mut ctx = match Context::open_without_resume(mode) {
        Ok(ctx) => ctx,
        Err(error) => return i32::from(error.report(mode, "repl")),
    };
    ctx.repl = true;
    let interactive = std::io::stdin().is_terminal();
    if interactive {
        eprintln!("交互模式：输入 help 查看命令，quit 退出");
    }
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        if interactive {
            eprint!("bt> ");
            let _ = std::io::stderr().flush();
        }
        let line = match lines.next() {
            Some(Ok(line)) => line,
            Some(Err(error)) => {
                eprintln!("读取输入失败：{error}");
                break;
            }
            None => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if matches!(line, "quit" | "exit" | ":q") {
            break;
        }
        if matches!(line, "help" | "?" | ":help") {
            print_help(mode);
            continue;
        }
        let tokens = split_args(line);
        if tokens.is_empty() {
            continue;
        }
        let line_mode = OutputMode::from_args(&tokens);
        let parsed = match Cli::try_parse_from(std::iter::once("bt".to_string()).chain(tokens)) {
            Ok(parsed) => parsed,
            Err(error) => {
                let output_mode = if line_mode.is_human() { mode } else { line_mode };
                if output_mode.is_human() {
                    eprintln!("{error}");
                } else {
                    let _ = CliError::usage(clap_message(&error))
                        .with_hint("在 repl 中输入 help 查看命令")
                        .report(output_mode, "bt");
                }
                continue;
            }
        };
        let mut parsed = parsed;
        if cli.json {
            parsed.json = true;
        }
        if cli.events {
            parsed.events = true;
        }
        let inner_mode = OutputMode::from_flags(parsed.json, parsed.events);
        let name = parsed.command_name();
        let Some(command) = parsed.command.take() else {
            if mode.is_human() {
                eprintln!("未指定子命令；输入 help 查看命令");
            } else {
                let _ = CliError::usage("未指定子命令")
                    .with_hint("在 repl 中输入 help 查看命令")
                    .report(inner_mode, "bt");
            }
            continue;
        };
        match &command {
            Command::Repl => {
                let _ = CliError::usage("已在 repl 中，不能再次启动 repl")
                    .with_hint("输入 quit 退出后重新运行 bt repl")
                    .report(inner_mode, name);
                continue;
            }
            Command::Gui => {
                let _ = CliError::usage("repl 中不能启动图形界面")
                    .with_hint("输入 quit 退出后运行 bt gui")
                    .report(inner_mode, name);
                continue;
            }
            Command::Recv(_) => {
                let _ = CliError::usage("repl 中不能启动阻塞接收")
                    .with_hint("输入 quit 退出后运行 bt recv 或 btrecv")
                    .report(inner_mode, name);
                continue;
            }
            _ => {}
        }
        signal::reset();
        if let Err(error) = crate::commands::dispatch(&command, &ctx) {
            error.report(inner_mode, name);
        }
    }
    ctx.shutdown();
    0
}

fn print_help(mode: OutputMode) {
    if !mode.is_human() {
        emit_result(
            mode,
            &ReplHelp {
                ok: true,
                command: "repl.help",
                commands: REPL_COMMANDS.to_vec(),
                note: "输入中的双引号可保留空格，例如 send text AA:BB:CC:00:00:01 \"hello world\"",
            },
        );
        return;
    }
    eprintln!("repl 命令：与 bt 子命令相同，可附加 --json 或 --events");
    for command in REPL_COMMANDS {
        eprintln!("  {command}");
    }
    eprintln!("输入中的双引号可保留空格，例如 send text AA:BB:CC:00:00:01 \"hello world\"");
}

/// 最小引号分词：双引号与单引号内的空白不拆分。
pub fn split_args(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for ch in line.chars() {
        match quote {
            Some(mark) if ch == mark => quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_args_keeps_quoted_spaces() {
        assert_eq!(
            split_args("send text AA:BB \"hello world\" --json"),
            vec!["send", "text", "AA:BB", "hello world", "--json"]
        );
        assert_eq!(split_args("  devices   list  "), vec!["devices", "list"]);
        assert_eq!(
            split_args("config set 'push_text_file_name' 'a b.txt'"),
            vec!["config", "set", "push_text_file_name", "a b.txt"]
        );
    }
}
