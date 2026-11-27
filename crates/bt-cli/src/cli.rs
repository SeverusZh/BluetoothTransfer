use std::path::PathBuf;

use bt_core::{Channel, Direction, RecordKind, RecordStatus};
use clap::{Args, Parser, Subcommand, ValueEnum};

/// 蓝牙传输命令行。
#[derive(Debug, Parser)]
#[command(
    name = "bt",
    version,
    about = "蓝牙传输：设备发现、OPP 与助手通道发送、接收、记录与迁移",
    long_about = None,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// 输出单个 JSON 文档
    #[arg(long, global = true)]
    pub json: bool,
    /// 输出 NDJSON 事件行，最后一行 type=result
    #[arg(long, global = true)]
    pub events: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 设备发现、配对、别名、收藏与助手通道探测
    Devices {
        #[command(subcommand)]
        action: Option<DevicesCommand>,
    },
    /// 发送文件、多文件、文本或文件夹
    Send {
        #[command(subcommand)]
        action: SendCommand,
    },
    /// 启动接收服务并阻塞监听，Ctrl+C 干净退出
    Recv(RecvArgs),
    /// 查询、删除、清空传输记录与统计
    Records {
        #[command(subcommand)]
        action: Option<RecordsCommand>,
    },
    /// 导出记录为 CSV 或 JSONL
    Export(ExportArgs),
    /// 查看与修改配置
    Config {
        #[command(subcommand)]
        action: Option<ConfigCommand>,
    },
    /// 断点续传索引与恢复
    Resume {
        #[command(subcommand)]
        action: ResumeCommand,
    },
    /// 迁移旧版 .NET 数据
    Migrate(MigrateArgs),
    /// 无蓝牙硬件可运行的自检
    Selftest,
    /// 启动图形界面
    Gui,
    /// 交互式会话，可复用同一后端状态
    Repl,
    /// 兼容旧名：扫描设备
    #[command(name = "opp-scan", hide = true)]
    OppScan(ScanArgs),
    /// 兼容旧名：配对
    #[command(name = "opp-pair", hide = true)]
    OppPair(PairArgs),
    /// 兼容旧名：发送单文件
    #[command(name = "opp-send-file", hide = true)]
    OppSendFile(SendPathsArgs),
    /// 兼容旧名：发送多文件
    #[command(name = "opp-send-files", hide = true)]
    OppSendFiles(SendPathsArgs),
    /// 兼容旧名：发送文本
    #[command(name = "opp-send-text", hide = true)]
    OppSendText(SendTextArgs),
    /// 兼容旧名：发送文件夹
    #[command(name = "opp-send-folder", hide = true)]
    OppSendFolder(SendFolderArgs),
    /// 兼容旧名：探测助手通道
    #[command(name = "detect", hide = true)]
    Detect(DetectArgs),
    /// 兼容旧名：记录统计
    #[command(name = "stats", hide = true)]
    Stats,
}

impl Cli {
    pub fn command_name(&self) -> &'static str {
        match &self.command {
            None => "bt",
            Some(Command::Devices { action }) => match action {
                None | Some(DevicesCommand::List(_)) => "devices.list",
                Some(DevicesCommand::Scan(_)) => "devices.scan",
                Some(DevicesCommand::Pair(_)) => "devices.pair",
                Some(DevicesCommand::Alias(_)) => "devices.alias",
                Some(DevicesCommand::Favorite(_)) => "devices.favorite",
                Some(DevicesCommand::Detect(_)) => "devices.detect",
            },
            Some(Command::Send { action }) => match action {
                SendCommand::File(_) => "send.file",
                SendCommand::Files(_) => "send.files",
                SendCommand::Text(_) => "send.text",
                SendCommand::Folder(_) => "send.folder",
            },
            Some(Command::Recv(_)) => "recv",
            Some(Command::Records { action }) => match action {
                None | Some(RecordsCommand::List(_)) => "records.list",
                Some(RecordsCommand::Delete { .. }) => "records.delete",
                Some(RecordsCommand::Clear { .. }) => "records.clear",
                Some(RecordsCommand::Stats) => "records.stats",
            },
            Some(Command::Export(_)) => "export",
            Some(Command::Config { action }) => match action {
                None | Some(ConfigCommand::List) => "config.list",
                Some(ConfigCommand::Get { .. }) => "config.get",
                Some(ConfigCommand::Set { .. }) => "config.set",
            },
            Some(Command::Resume { action }) => match action {
                ResumeCommand::List => "resume.list",
                ResumeCommand::Run => "resume.run",
                ResumeCommand::Cancel => "resume.cancel",
            },
            Some(Command::Migrate(_)) => "migrate",
            Some(Command::Selftest) => "selftest",
            Some(Command::Gui) => "gui",
            Some(Command::Repl) => "repl",
            Some(Command::OppScan(_)) => "devices.scan",
            Some(Command::OppPair(_)) => "devices.pair",
            Some(Command::OppSendFile(_)) => "send.file",
            Some(Command::OppSendFiles(_)) => "send.files",
            Some(Command::OppSendText(_)) => "send.text",
            Some(Command::OppSendFolder(_)) => "send.folder",
            Some(Command::Detect(_)) => "devices.detect",
            Some(Command::Stats) => "records.stats",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum DevicesCommand {
    /// 列出设备缓存，默认按最近发现排序
    List(ListArgs),
    /// 扫描设备并等待扫描结束
    Scan(ScanArgs),
    /// 发起配对；PIN 类请求需要 --pin 或交互输入
    Pair(PairArgs),
    /// 设置或清除别名
    Alias(AliasArgs),
    /// 收藏或取消收藏
    Favorite(FavoriteArgs),
    /// 探测对端助手通道，三态退出码
    Detect(DetectArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// 排序：last 最近发现、name 名称、favorite 收藏优先
    #[arg(long, value_enum, default_value = "last")]
    pub sort: SortArg,
}

#[derive(Debug, Args)]
pub struct ScanArgs {
    /// 扫描时长；省略由后端决定
    #[arg(long)]
    pub seconds: Option<u32>,
    /// 只保留已配对设备
    #[arg(long)]
    pub paired_only: bool,
}

#[derive(Debug, Args)]
pub struct PairArgs {
    /// 设备蓝牙地址
    pub address: String,
    /// 应答 PIN 类配对请求；值不写入日志与输出
    #[arg(long)]
    pub pin: Option<String>,
}

#[derive(Debug, Args)]
pub struct AliasArgs {
    /// 设备蓝牙地址
    pub address: String,
    /// 别名；省略或空串清除
    #[arg(num_args = 0..)]
    pub alias: Vec<String>,
}

#[derive(Debug, Args)]
pub struct FavoriteArgs {
    /// 设备蓝牙地址
    pub address: String,
    /// 取消收藏
    #[arg(long)]
    pub unset: bool,
}

#[derive(Debug, Args)]
pub struct DetectArgs {
    /// 设备蓝牙地址
    pub address: String,
}

#[derive(Debug, Subcommand)]
pub enum SendCommand {
    /// 发送一个或多个文件，顺序发送并汇总
    File(SendPathsArgs),
    /// 发送多个文件，语义同 send file
    Files(SendPathsArgs),
    /// 发送文本，对端落盘为 txt
    Text(SendTextArgs),
    /// 发送文件夹，自动打包为 zip
    Folder(SendFolderArgs),
}

#[derive(Debug, Args)]
pub struct SendPathsArgs {
    /// 目标设备蓝牙地址
    pub device: String,
    /// 一个或多个文件路径
    #[arg(required = true, num_args = 1..)]
    pub paths: Vec<PathBuf>,
    /// 先打包为 zip 再发送
    #[arg(long)]
    pub zip: bool,
    /// 通道模式；省略时使用配置 mode
    #[arg(long, value_enum)]
    pub mode: Option<ModeArg>,
}

#[derive(Debug, Args)]
pub struct SendTextArgs {
    /// 目标设备蓝牙地址
    pub device: String,
    /// 文本内容，多个参数以空格拼接
    #[arg(required = true, num_args = 1..)]
    pub text: Vec<String>,
    /// 记录展示名；远端文件名由配置 push_text_file_name 决定
    #[arg(long)]
    pub name: Option<String>,
    /// 通道模式；省略时使用配置 mode
    #[arg(long, value_enum)]
    pub mode: Option<ModeArg>,
}

#[derive(Debug, Args)]
pub struct SendFolderArgs {
    /// 目标设备蓝牙地址
    pub device: String,
    /// 待发送目录
    pub dir: PathBuf,
    /// 通道模式；省略时使用配置 mode
    #[arg(long, value_enum)]
    pub mode: Option<ModeArg>,
}

#[derive(Debug, Args, Clone)]
pub struct RecvArgs {
    /// 落盘目录，缺省使用配置 receive_dir
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// 接收前询问
    #[arg(long, conflicts_with = "no_ask")]
    pub ask: bool,
    /// 不询问直接接收
    #[arg(long, conflicts_with = "ask")]
    pub no_ask: bool,
    /// 完成一次接收后退出
    #[arg(long)]
    pub once: bool,
}

#[derive(Debug, Subcommand)]
pub enum RecordsCommand {
    /// 按条件列出记录
    List(RecordFilterArgs),
    /// 删除单条记录
    Delete {
        /// 记录编号
        id: i64,
    },
    /// 清空全部记录，必须显式 --yes
    Clear {
        /// 确认清空
        #[arg(long)]
        yes: bool,
    },
    /// 汇总统计
    Stats,
}

#[derive(Debug, Args, Clone)]
pub struct RecordFilterArgs {
    /// 方向：send 或 recv
    #[arg(long, value_enum)]
    pub direction: Option<DirectionArg>,
    /// 类型：file、multi-file、folder、text、other
    #[arg(long = "type", value_enum)]
    pub kind: Option<KindArg>,
    /// 状态：ok、failed、canceled、interrupted、running
    #[arg(long, value_enum)]
    pub status: Option<StatusArg>,
    /// 对端蓝牙地址
    #[arg(long)]
    pub peer: Option<String>,
    /// 关键词，匹配名称与对端名
    #[arg(long)]
    pub search: Option<String>,
    /// 最多返回条数
    #[arg(long, default_value_t = 100)]
    pub limit: u32,
    /// 跳过条数
    #[arg(long, default_value_t = 0)]
    pub offset: u32,
}

impl Default for RecordFilterArgs {
    fn default() -> Self {
        Self {
            direction: None,
            kind: None,
            status: None,
            peer: None,
            search: None,
            limit: 100,
            offset: 0,
        }
    }
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// 导出格式：csv 或 jsonl
    #[arg(value_enum)]
    pub format: ExportFormatArg,
    /// 输出文件路径
    pub out: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// 列出全部配置
    List,
    /// 读取单项配置
    Get {
        /// 配置键
        key: String,
    },
    /// 修改配置；auth-password 只从 stdin 或 TTY 读取
    Set {
        /// 配置键
        key: String,
        /// 配置值；auth-password 不接受命令行值
        #[arg(value_name = "VALUE")]
        value: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ResumeCommand {
    /// 列出 journal 中的续传条目
    List,
    /// 重新入队可续传条目的载荷
    Run,
    /// 取消全部待续传条目
    Cancel,
}

#[derive(Debug, Args)]
pub struct MigrateArgs {
    /// 旧数据根目录，缺省按平台配置目录推断
    #[arg(long)]
    pub from: Option<PathBuf>,
    /// 只探测不写入
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ModeArg {
    Auto,
    Assistant,
    Opp,
}

impl ModeArg {
    pub fn channel(self) -> Option<Channel> {
        match self {
            Self::Auto => None,
            Self::Assistant => Some(Channel::Assistant),
            Self::Opp => Some(Channel::Opp),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SortArg {
    Last,
    Name,
    Favorite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DirectionArg {
    Send,
    #[value(alias = "receive")]
    Recv,
}

impl DirectionArg {
    pub fn direction(self) -> Direction {
        match self {
            Self::Send => Direction::Send,
            Self::Recv => Direction::Receive,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum KindArg {
    File,
    #[value(name = "multi-file", alias = "multi_file")]
    MultiFile,
    Folder,
    Text,
    Other,
}

impl KindArg {
    pub fn kind(self) -> RecordKind {
        match self {
            Self::File => RecordKind::File,
            Self::MultiFile => RecordKind::MultiFile,
            Self::Folder => RecordKind::Folder,
            Self::Text => RecordKind::Text,
            Self::Other => RecordKind::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StatusArg {
    Ok,
    Failed,
    #[value(alias = "cancelled")]
    Canceled,
    Interrupted,
    Running,
}

impl StatusArg {
    pub fn status(self) -> RecordStatus {
        match self {
            Self::Ok => RecordStatus::Completed,
            Self::Failed => RecordStatus::Failed,
            Self::Canceled => RecordStatus::Canceled,
            Self::Interrupted => RecordStatus::Interrupted,
            Self::Running => RecordStatus::Running,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportFormatArg {
    Csv,
    #[value(alias = "json")]
    Jsonl,
}

impl ExportFormatArg {
    pub fn format(self) -> bt_api::ExportFormat {
        match self {
            Self::Csv => bt_api::ExportFormat::Csv,
            Self::Jsonl => bt_api::ExportFormat::Jsonl,
        }
    }
}

/// btrecv：把参数透明转给 `bt recv`，无参即监听。
#[derive(Debug, Parser)]
#[command(
    name = "btrecv",
    version,
    about = "蓝牙接收助手：无参启动监听，参数与 bt recv 一致",
    long_about = None,
    disable_help_subcommand = true
)]
pub struct BtrecvCli {
    /// 输出单个 JSON 文档
    #[arg(long, global = true)]
    pub json: bool,
    /// 输出 NDJSON 事件行
    #[arg(long, global = true)]
    pub events: bool,
    #[command(subcommand)]
    pub command: Option<BtrecvCommand>,
    #[command(flatten)]
    pub direct: RecvArgs,
}

#[derive(Debug, Subcommand)]
pub enum BtrecvCommand {
    #[command(hide = true)]
    Recv(RecvArgs),
    #[command(hide = true)]
    Help,
    #[command(hide = true)]
    Version,
}
