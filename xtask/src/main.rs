//! 构建任务入口；参数解析刻意保持零依赖。

use std::process::ExitCode;

const HELP: &str = "用法：cargo xtask <任务>

任务：
  dist        发布打包入口，实际执行见发布工作流
  bundle      安装器入口，实际执行见发布工作流
  help        显示本帮助";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("help") | Some("-h") | Some("--help") => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("dist") => {
            eprintln!("dist 由 .github/workflows/release.yml 或 packaging/linux 脚本执行");
            ExitCode::from(1)
        }
        Some("bundle") => {
            eprintln!("bundle 由 .github/workflows/release.yml 的 Windows runner 执行");
            ExitCode::from(1)
        }
        Some(other) => {
            eprintln!("未知任务 {other}");
            println!("{HELP}");
            ExitCode::from(2)
        }
    }
}
