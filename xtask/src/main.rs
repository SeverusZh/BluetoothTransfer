//! 构建任务入口；参数解析刻意保持零依赖。

use std::process::ExitCode;

const HELP: &str = "用法：cargo xtask <任务>

任务：
  dist        打包发布产物，待实现
  bundle      生成安装器，待实现
  help        显示本帮助";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("help") | Some("-h") | Some("--help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("dist") => {
            eprintln!("dist 任务待实现：发布打包属于 T6");
            ExitCode::from(1)
        }
        Some("bundle") => {
            eprintln!("bundle 任务待实现：安装器属于 T6");
            ExitCode::from(1)
        }
        Some(other) => {
            eprintln!("未知任务 {other}");
            println!("{HELP}");
            ExitCode::from(2)
        }
    }
}
