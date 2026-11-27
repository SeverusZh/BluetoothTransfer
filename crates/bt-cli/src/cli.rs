use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// 蓝牙传输命令行。
#[derive(Debug, Parser)]
#[command(
    name = "bt",
    version,
    about = "蓝牙传输：OPP 与私有助手通道发送、接收、记录与迁移",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// 可用子命令；本轮只有自检可用，发送与接收在 T2 落地。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 运行自检并输出逐项结果
    Selftest,
    /// 发送文件、文件夹或文本到指定设备
    Send {
        /// 目标设备蓝牙地址
        device: String,
        /// 待发送的路径；不传则从标准输入读取文本
        paths: Vec<PathBuf>,
        /// 通道模式 auto、assistant 或 opp
        #[arg(long, value_name = "MODE")]
        mode: Option<String>,
        /// 文本内容，与 paths 二选一
        #[arg(long, value_name = "TEXT")]
        text: Option<String>,
    },
    /// 启动接收服务
    Recv {
        /// 落盘目录，缺省使用配置值
        #[arg(long)]
        dir: Option<PathBuf>,
        /// 接收前是否询问
        #[arg(long)]
        ask: Option<bool>,
    },
}
