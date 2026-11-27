use std::io::{IsTerminal, Write};
use std::time::Duration;

use crate::backend::Context;
use crate::cli::RecvArgs;
use crate::error::{CliError, EXIT_FAILED, EXIT_OK};
use crate::output::{emit_event, emit_result, event_to_json};
use crate::signal;
use bt_api::{Event, ReceiveOptions};
use crossbeam_channel::RecvTimeoutError;
use serde::Serialize;

#[derive(Default)]
struct ReceivedFile {
    ok: bool,
    path: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct ReceiveResult {
    ok: bool,
    stopped: bool,
    listening: bool,
    dir: String,
    ask: bool,
    received: Vec<ReceivedFileJson>,
    received_count: usize,
}

#[derive(Serialize)]
struct ReceivedFileJson {
    ok: bool,
    path: Option<String>,
    error: Option<String>,
}

impl From<ReceivedFile> for ReceivedFileJson {
    fn from(value: ReceivedFile) -> Self {
        Self {
            ok: value.ok,
            path: value.path,
            error: value.error,
        }
    }
}

pub fn run(ctx: &Context, args: &RecvArgs) -> Result<u8, CliError> {
    let ask = if args.ask {
        Some(true)
    } else if args.no_ask {
        Some(false)
    } else {
        None
    };
    ctx.drain_events();
    ctx.backend.start_receive(ReceiveOptions {
        dir: args.dir.clone(),
        ask,
    })?;
    let session = ctx.backend.receive_session();
    if ctx.mode.is_human() {
        eprintln!(
            "接收服务已启动：目录 {}，{}；Ctrl+C 停止",
            session.dir.display(),
            if session.ask { "接收前询问" } else { "自动接收" }
        );
    }
    let mut received: Vec<ReceivedFile> = Vec::new();
    let mut stopped_by_signal = false;
    let mut exit_code = EXIT_OK;
    loop {
        if signal::canceled() {
            let _ = ctx.backend.stop_receive();
            stopped_by_signal = true;
            break;
        }
        match ctx.events.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => {
                emit_event(ctx.mode, event_to_json(&event));
                match event {
                    Event::ReceiveAsk { id, name, size } => {
                        if ctx.mode.is_human() {
                            eprintln!("接收请求：{name}（{size} 字节）");
                        }
                        let accept = decide_ask(&name)?;
                        if let Some(path) = ctx.backend.answer_receive_ask(id, accept)? {
                            if ctx.mode.is_human() {
                                eprintln!("已接受，落盘路径：{}", path.display());
                            }
                        } else if ctx.mode.is_human() {
                            eprintln!("已拒绝接收：{name}");
                        }
                    }
                    Event::ReceiveProgress { bytes, total, name } => {
                        if ctx.mode.is_human() {
                            match total {
                                Some(total) if total > 0 => {
                                    let percent = (bytes.min(total) as f64 / total as f64 * 100.0).round();
                                    eprint!("\r接收 {name}：{percent}%  {bytes}/{total} 字节   ");
                                }
                                _ => eprint!("\r接收 {name}：{bytes} 字节   "),
                            }
                            let _ = std::io::stderr().flush();
                        }
                    }
                    Event::ReceiveDone { ok, path, error } => {
                        if ctx.mode.is_human() {
                            eprintln!();
                        }
                        received.push(ReceivedFile {
                            ok,
                            path: path.map(|value| value.display().to_string()),
                            error,
                        });
                        if ctx.mode.is_human() {
                            if ok {
                                println!("接收完成");
                            } else {
                                eprintln!("接收失败");
                            }
                        }
                        if args.once {
                            let _ = ctx.backend.stop_receive();
                            if !ok {
                                exit_code = EXIT_FAILED;
                            }
                            break;
                        }
                    }
                    Event::Log { level, message, .. } if ctx.mode.is_human() => {
                        eprintln!("[{level}] {message}");
                    }
                    _ => {}
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(CliError::failed("事件通道已关闭，接收服务状态未知"));
            }
        }
    }
    let session = ctx.backend.receive_session();
    let result = ReceiveResult {
        ok: received.iter().all(|item| item.ok) && exit_code == EXIT_OK,
        stopped: true,
        listening: session.listening,
        dir: session.dir.display().to_string(),
        ask: session.ask,
        received_count: received.len(),
        received: received.into_iter().map(ReceivedFileJson::from).collect(),
    };
    emit_result(ctx.mode, &result);
    if ctx.mode.is_human() {
        if stopped_by_signal {
            println!("接收服务已停止，共接收 {} 个文件", result.received_count);
        } else {
            println!("接收结束，共接收 {} 个文件", result.received_count);
        }
    }
    Ok(exit_code)
}

fn decide_ask(name: &str) -> Result<bool, CliError> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        eprintln!("非交互环境默认拒绝未确认的接收请求：{name}");
        eprintln!("提示：无人值守接收请使用 --no-ask");
        return Ok(false);
    }
    eprint!("接收 {name}？[y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let read = stdin.read_line(&mut answer).map_err(CliError::from)?;
    if read == 0 {
        return Ok(false);
    }
    let answer = answer.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}
