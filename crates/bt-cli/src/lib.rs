//! 命令行前端：bt 提供完整命令集，btrecv 是只做接收的薄壳。
//!
//! 默认特性启用 gui，使 bt 在无子命令时直接打开图形界面；
//! 关闭默认特性后 bt 只保留纯命令行路径，不链接 eframe。

pub mod cli;

use bt_app::{build_backend, AppPaths};
use clap::Parser;

pub use cli::{Cli, Command};

/// 进程入口，返回退出码：0 成功、1 自检未通过、2 用法错误、3 未实现或运行失败。
pub fn main_entry(args: Vec<String>) -> i32 {
    let parsed = Cli::try_parse_from(std::iter::once("bt".to_string()).chain(args));
    let cli = match parsed {
        Ok(cli) => cli,
        Err(error) => {
            let code = match error.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion
                | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => 0,
                _ => 2,
            };
            let _ = error.print();
            return code;
        }
    };
    dispatch(cli)
}

fn dispatch(cli: Cli) -> i32 {
    match cli.command {
        None => launch_default(),
        Some(Command::Selftest) => run_selftest(),
        Some(Command::Send { .. }) => pending("send"),
        Some(Command::Recv { .. }) => pending("recv"),
    }
}

fn launch_default() -> i32 {
    #[cfg(feature = "gui")]
    {
        match bt_gui::run_window() {
            Ok(code) => i32::from(code),
            Err(error) => {
                eprintln!("{error}");
                3
            }
        }
    }
    #[cfg(not(feature = "gui"))]
    {
        eprintln!("未指定子命令，且本构建不含图形界面；请使用 bt --help 查看命令");
        2
    }
}

fn run_selftest() -> i32 {
    let paths = match AppPaths::resolve() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("{error}");
            return 3;
        }
    };
    let backend = match build_backend(paths) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("{error}");
            return 3;
        }
    };
    let _events = backend.subscribe();
    match backend.selftest() {
        Ok(report) => {
            for check in &report.checks {
                let mark = if check.ok { "ok" } else { "fail" };
                println!("[{mark}] {}: {}", check.name, check.detail);
            }
            if report.passed() {
                0
            } else {
                println!("自检未通过");
                3
            }
        }
        Err(error) => {
            eprintln!("{error}");
            3
        }
    }
}

fn pending(command: &str) -> i32 {
    eprintln!("{command} 子命令待 T2 实现");
    3
}

/// btrecv 入口：默认走接收语义。
pub fn recv_entry() -> i32 {
    eprintln!("接收服务待 T2 实现");
    3
}
