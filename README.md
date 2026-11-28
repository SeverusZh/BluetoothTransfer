# 蓝牙传输

[![Version](https://img.shields.io/badge/version-2.0.1-blue)](CHANGELOG.md)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

跨平台蓝牙文件传输工具，Rust 重写版本。图形界面与命令行共用同一套后端，覆盖设备发现、配对、OPP 与私有助手通道发送、断点续传、接收、记录、迁移与自检。

- 仓库：https://github.com/SeverusZh/BluetoothTransfer
- 版本：2.0.1
- 许可证：MIT
- 工具链：固定 1.100.1，MSRV 1.95

## 项目定位

本仓库是原 Windows .NET 8 版本的 Rust 重写实现，产物为单一 workspace 下的 GUI 与 CLI，两者通过 `bt-api` 契约共用同一后端、同一配置与同一记录库，记录互通。

发送与接收使用两条通道：

- OPP：标准 OBEX 对象推送，兼容 Android 与常见蓝牙文件接收端，不要求对端安装本应用；传输为明文。
- 私有助手通道：自定义 RFCOMM 协议，要求收发两端都运行本应用，支持分片级断点续传与端到端 SHA-256 校验。

与原 .NET 版本的关系：旧实现归档在 `legacy/dotnet/`，只读参考，不参与构建、测试与发布；v1.x 发布产物来自旧实现，旧配置、记录与发送 journal 可用 `bt migrate` 迁移。

## 下载与安装

发布产物由 `.github/workflows/release.yml` 在推送 `v*` 标签时构建，版本号取自根 `Cargo.toml` 的 `workspace.package.version`。v2.0.1 资产在推送 `v2.0.1` 标签后生成。

| 平台 | 产物 | 文件名 |
| --- | --- | --- |
| Windows | 安装器 | `BluetoothTransfer-2.0.1-setup.exe` |
| Windows | 图形界面绿色包 | `BluetoothTransfer-2.0.1-windows-x64.zip` |
| Windows | cli-only 包 | `BluetoothTransfer-2.0.1-cli-windows-x64.zip` |
| Linux | deb 包 | `bluetooth-transfer_2.0.1_amd64.deb` |
| Linux | AppImage | `BluetoothTransfer-2.0.1-x86_64.AppImage` |

下载地址形如 `https://github.com/SeverusZh/BluetoothTransfer/releases/download/v2.0.1/<文件名>`：

- [BluetoothTransfer-2.0.1-setup.exe](https://github.com/SeverusZh/BluetoothTransfer/releases/download/v2.0.1/BluetoothTransfer-2.0.1-setup.exe)
- [BluetoothTransfer-2.0.1-windows-x64.zip](https://github.com/SeverusZh/BluetoothTransfer/releases/download/v2.0.1/BluetoothTransfer-2.0.1-windows-x64.zip)
- [BluetoothTransfer-2.0.1-cli-windows-x64.zip](https://github.com/SeverusZh/BluetoothTransfer/releases/download/v2.0.1/BluetoothTransfer-2.0.1-cli-windows-x64.zip)
- [bluetooth-transfer_2.0.1_amd64.deb](https://github.com/SeverusZh/BluetoothTransfer/releases/download/v2.0.1/bluetooth-transfer_2.0.1_amd64.deb)
- [BluetoothTransfer-2.0.1-x86_64.AppImage](https://github.com/SeverusZh/BluetoothTransfer/releases/download/v2.0.1/BluetoothTransfer-2.0.1-x86_64.AppImage)

### Windows

- 安装器：per-user 安装到 `%LOCALAPPDATA%\Programs\BluetoothTransfer`，写入 `HKCU`，不申请管理员权限；默认创建开始菜单项与卸载入口，可选资源管理器右键菜单、SendTo 快捷方式与开机自启。静默安装为 `BluetoothTransfer-2.0.1-setup.exe /S`。
- 绿色包：解压后直接运行 `bt-gui.exe`，包内附 `LICENSE`，仓库存在 `README.md` 时一并打入。
- cli-only 包：`bt.exe` 与 `btrecv.exe`，不链接 GUI 依赖，适合脚本与服务器环境。
- 升级只覆盖程序文件，保留 `%LOCALAPPDATA%\BlueToothTransfer` 与 `%APPDATA%\BlueToothTransfer` 下的配置、记录与日志。
- 图标：`bt-gui.exe` 内嵌应用图标，窗口、任务栏、资源管理器右键菜单与 SendTo 快捷方式使用同一图标。
- 无控制台窗口：`bt-gui.exe` 为 Windows GUI 子系统，双击启动不弹出命令行窗口；`bt.exe` 与 `btrecv.exe` 保持控制台子系统，保留标准输出与退出码。
- 界面自检：`bt-gui.exe --selftest` 没有控制台输出，默认把 JSON 报告写到 `%TEMP%\bt-gui-selftest.json`；可用 `--selftest --report <路径>` 指定报告文件。

### Linux

- deb 包：`sudo apt install ./bluetooth-transfer_2.0.1_amd64.deb`，安装 `/usr/bin/bt-gui`、`/usr/bin/bt`、`/usr/bin/btrecv`、桌面文件与图标。`postinst`/`postrm` 只尝试刷新桌面数据库与图标缓存，失败不阻断安装。
- AppImage：`chmod +x` 后直接运行；无 FUSE 环境使用 `APPIMAGE_EXTRACT_AND_RUN=1`。
- 运行依赖 BlueZ 提供的 `org.bluez` D-Bus 服务；图形界面还需要 X11 或 Wayland 会话。

### 校验和

发布页提供 `combined-sha256sums.txt`，逐行列出全部发布的产物，行内只写文件名；下载全部资产后可直接校验：

```bash
sha256sum -c combined-sha256sums.txt
```

构建作业另输出 `sha256sums-windows.txt` 与 `sha256sums-linux.txt`，按平台列出各自产物。

构建未签名，Windows SmartScreen 会提示未知发布者。

### 从源码构建

工具链版本固定在根 `rust-toolchain.toml`，rustup 会按该文件选择并安装 1.100.1：

```bash
git clone https://github.com/SeverusZh/BluetoothTransfer.git
cd BlueToothTransfer
cargo --version        # rustup 按 rust-toolchain.toml 安装并使用 1.100.1
```

本机若已有仓库内工具链目录 `.toolchain/`（不纳入版本控制，由使用者自行准备），先加载环境可省去重复下载：

```bash
. ./.toolchain/env.sh
```

构建 CLI 与接收薄壳：

```bash
cargo build --release -p bt-cli --no-default-features
# 产物：target/release/bt 与 target/release/btrecv
```

构建图形界面与完整版 `bt`：

```bash
cargo build --release -p bt-gui
cargo build --release -p bt-cli
# 产物：target/release/bt-gui 与 target/release/bt
```

Linux 构建 GUI 需要 `libxkbcommon`、`libx11`、`libxi`、`libgl1-mesa`、`libegl-mesa`、`libwayland`、`libudev`、`pkg-config` 等开发包。`cargo xtask msrv` 校验 workspace 声明的 MSRV 不低于依赖要求。

## 快速开始

### 图形界面

1. 运行 `bt-gui`，或完整版 `bt` 不带子命令，在设备页点击扫描并设置扫描时长。
2. 在设备列表选中目标设备，未配对的先配对；已配对设备可直接发送。
3. 在发送页拖入文件、选择文件夹或输入文本加入队列，查看进度与结果；接收方向在接收页开启监听。

无显示环境可运行 `bt-gui --selftest`，以退出码 0 结束表示界面依赖、页面状态与后端装配可用。

### 命令行

```bash
bt devices scan --seconds 8 --json
bt devices pair AA:BB:CC:DD:EE:FF --json
bt send file AA:BB:CC:DD:EE:FF ./report.pdf --json
bt recv --dir ~/Downloads/BluetoothReceive --no-ask --events
bt records list --direction send --limit 5 --json
```

- `bt --help` 列出全部命令与参数，`bt --version` 输出版本。
- `bt recv` 或 `btrecv` 不带参数即开始监听，Ctrl+C 停止并以 0 退出。
- 完整契约见 `docs/CLI.md`。

## 图形界面能力

| 页面 | 能力 |
| --- | --- |
| 设备 | 扫描与取消、设备列表、配对与配对口令回填、助手通道三态探测、收藏、别名、打开系统蓝牙设置 |
| 发送 | 单文件、多文件、文本、文件夹、剪贴板与拖放发起；单文件 zip 选项；通道 auto/assistant/opp；总进度与速率 |
| 队列 | 逐任务暂停、继续、取消、重试、移除，状态与错误展示 |
| 接收 | 启停监听、落盘目录、接收前询问与待决询问、进度与完成列表、SHA-256 校验结果 |
| 记录 | 按方向、通道、状态、设备与关键字筛选，复制、单条删除、清空、CSV 与 JSONL 导出 |
| 日志 | 最近事件、状态栏提示、日志落盘位置 |
| 设置 | 12 项普通配置、OPP 认证密码、迁移入口、自检入口 |

桌面集成：

- 托盘与通知：最小化到托盘、传输完成与失败通知、托盘菜单快捷入口。
- 单实例与外部收件箱：重复启动唤起已有实例；系统「发送到」与文件管理器把路径投递给运行中的实例。
- Windows 集成：安装器可选写入 5 个资源管理器静态 Verb（文件、文件夹、文件夹背景、驱动器、`.txt`）与 SendTo 快捷方式；Windows 11 需在「显示更多选项」中查看。
- Linux 集成：安装 `.desktop` 文件与图标；用户级右键菜单与开机自启由设置页写入 `~/.local/share` 与 `~/.config/autostart`。
- 窗口状态记忆与可选开机自启。

## 命令行

两个二进制：`bt` 提供完整命令集，`btrecv` 是只做接收的薄壳。数据写 stdout，诊断写 stderr。

### 命令矩阵

| 命令 | 用途 |
| --- | --- |
| `bt devices list/scan/pair/alias/favorite/detect` | 设备缓存、扫描、配对、别名、收藏、助手通道三态探测 |
| `bt send file/files/text/folder` | 顺序发送并等待终态，`--mode auto\|assistant\|opp`、`--zip`、`--name` |
| `bt recv`、`btrecv` | 接收服务，`--dir`、`--ask`/`--no-ask`、`--once` |
| `bt records list/delete/clear/stats` | 记录查询、单条删除、清空、统计 |
| `bt export csv\|jsonl` | 流式导出记录 |
| `bt config list/get/set` | 读写 12 项配置与 OPP 认证密码 |
| `bt resume list/run/cancel` | 按 BTJ1 journal 查询、恢复与取消待续传发送 |
| `bt migrate` | 迁移旧 .NET 数据，`--from`、`--dry-run` |
| `bt selftest` | 无硬件自检 |
| `bt repl` | 同一后端会话执行多条命令 |
| `bt gui` | 打开图形界面，cli-only 构建返回错误 |

### 输出模式

- `--json`：stdout 恰好一个 JSON 对象，不混入日志与进度；字段一律 snake_case，时间为 UTC RFC 3339。
- `--events`：stdout 为 NDJSON，每行一个事件，最后一行固定为 `type=result`，失败时为 `type=error`；事件类型包括 `job_progress`、`job_state_changed`、`receive_progress`、`receive_ask`、`receive_done`、`pair_prompt` 等。
- 参数解析失败时，带 `--json`/`--events` 同样输出结构化错误对象，退出码 2。

### 退出码

| 码 | 含义 |
| --- | --- |
| 0 | 成功，含 `selftest` 全部通过与接收被 Ctrl+C 停止 |
| 1 | 操作失败，含传输失败、自检未通过、续传条目全部跳过 |
| 2 | 用法错误，含未知命令、缺参数、地址非法、清空记录缺少 `--yes` |
| 3 | 已有实例或任务占用 |
| 4 | 权限或锁失败 |
| 5 | 目标未找到 |

`devices detect` 使用独立三态映射：`present=0`、`absent=1`、`unknown=5`。

### 密码处理

OPP 认证密码是第 13 项设置，不属于 12 项普通配置：

```bash
printf '%s' "$PASSWORD" | bt config set auth-password
```

- 值从 stdin 或 TTY 读取，TTY 下不回显；传入位置参数会被拒绝并返回 2。
- 密码不出现在 argv、日志、错误信息与 JSON 输出中；`bt config get auth-password` 永远返回用法错误。
- 空输入表示清除密码。

### 旧命令别名

以下旧命令名保留且不出现在帮助里：

| 旧名 | 等价命令 |
| --- | --- |
| `opp-scan` | `devices scan` |
| `opp-pair` | `devices pair` |
| `opp-send-file` | `send file` |
| `opp-send-files` | `send files` |
| `opp-send-text` | `send text` |
| `opp-send-folder` | `send folder` |
| `detect` | `devices detect` |
| `stats` | `records stats` |

`devices`、`records`、`config` 不带子命令时分别等价于 `devices list`、`records list`、`config list`。

## Agent Skill

`skills/bluetooth-transfer/SKILL.md` 是面向 Agent 的可执行指引，含命令矩阵、JSON 契约、退出码、错误恢复与安全边界；辅助脚本在 `skills/bluetooth-transfer/scripts/`。

加载方式：文件带 `name` 与 `description` 前置元数据，支持 Agent Skill 目录的运行时把 `skills/bluetooth-transfer/` 收入技能目录即可按需加载；不支持该机制的运行时直接读取该文件作为操作指引。

无蓝牙硬件的验证路径：

```bash
export BT_BACKEND=mock
export BT_HOME=$(mktemp -d)
bt devices list --json
```

- `BT_BACKEND=mock` 使用内存后端，不写磁盘、不访问蓝牙；固定设备为 `AA:BB:CC:00:00:01` Mock Phone（已配对、助手通道可用）与 `AA:BB:CC:00:00:02` Mock Tablet。
- `BT_HOME` 隔离便携根：配置在 `$BT_HOME/.config/BluetoothTransfer/`，数据在 `$BT_HOME/.local/share/BluetoothTransfer/`，默认接收目录为 `$BT_HOME/Downloads/BluetoothReceive`。
- 模拟后端不跨进程持久化，需要串联状态时用 `bt repl --json`。

## 轻量化指标

| 指标 | 上限 | 实测 |
| --- | --- | --- |
| 接收最终文件额外字节 | 0 | 0 |
| 半成品 sidecar | 64 字节 | 53 字节 |
| journal 单条目 | 256 字节 | 182 字节典型，253 字节最坏 |
| 配置全默认时 | 不落盘 | 不创建文件；单个非默认项 13 字节 |
| 导出 JSONL 单行 | 250 字节 | 205 字节 |
| 导出 CSV 单行 | 200 字节 | 表头 74 字节，数据 94 字节 |
| OPP 协议开销 | 0.03% 以内 | 0.00923% |
| 助手协议开销 | 0.01% 以内 | 0.00725% |
| cli-only 发布体积 | 8 MB | `bt` 3,609,904 字节，`btrecv` 3,352,048 字节 |
| GUI 单文件 | 20 MB | 11,176,216 字节 |
| deb 包 | 20 MB | 约 5.67 MB，字节数不可复现，随二进制与打包时间戳波动 |
| NSIS 安装器 | 15 MB | 本机无 makensis，未生成 |

体积实测方法与验证边界见 `docs/RELEASE.md`，协议与导出指标由单元测试固化，CLI 侧实测值见 `docs/ACCEPTANCE.md`。

## 构建与测试

质量门禁与 CI 使用同一组命令，工具链由 `rust-toolchain.toml` 固定为 1.100.1：

```bash
. ./.toolchain/env.sh          # 使用仓库内工具链目录时
cargo xtask msrv               # workspace MSRV 与依赖要求一致性
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
cargo build -p bt-cli --no-default-features
cargo run -p bt-gui -- --selftest
cargo test -p bt-cli           # CLI 端到端用例，使用临时 BT_HOME 与 BT_BACKEND=mock
```

CI 在 `.github/workflows/ci.yml` 中运行三个作业：`质量门禁 (ubuntu-latest)` 与 `质量门禁 (windows-latest)` 依次执行 MSRV 元数据检查、格式化、静态检查、测试、发布构建、纯 CLI 构建与 GUI 自检；`MSRV 构建` 用 1.95.0 执行 `cargo check --workspace --all-targets` 校验声明的 MSRV。

测试数量：`cargo test --workspace` 共 296 项通过、0 失败，1 项因缺少桌面会话 D-Bus 被忽略；9 个 doc-test 目标无用例。`cargo test -p bt-cli` 合计 61 项，其中 `crates/bt-cli/tests/` 下的端到端用例 47 项，使用临时 `BT_HOME` 与 `BT_BACKEND=mock`，不依赖蓝牙硬件。

## 平台与限制

- 支持 Windows x64 与 Linux x64；Linux 依赖 BlueZ 提供的 D-Bus 服务与 RFCOMM 套接字。
- OPP 为明文传输，不要在不可信链路发送敏感数据。
- iOS 不支持蓝牙文件接收；向 iPhone 发送文件需要对方运行本应用接收端或改用其他方式。
- 私有助手通道要求收发两端都运行本应用；普通 OPP 只要求接收端支持蓝牙文件接收。
- OPP 单包上限 65023 字节；助手通道单帧上限 1 MiB，默认分片 256 KiB。
- 接收端单实例：同一时刻只有一个接收会话占用 RFCOMM 通道。
- 接收文件名会被清洗为单段名称，拒绝 `..` 穿越；重名自动追加 ` (n)`。
- 单文件 zip 与文件夹打包走 ZIP32，源文件超过 4 GiB 时命令级拒绝。
- 配对 PIN 由 CLI 的 `--pin` 或 GUI 对话框回填到传输层，只用于本次配对，不进入日志、记录与 JSON。

## 未验证边界

以下路径在本仓库的开发环境中缺少硬件、系统服务或对应运行时，未验证，不以模拟结果代替结论：

1. 真实蓝牙适配器上的扫描、配对、SDP、RFCOMM 与 OPP 互操作。
2. Windows WinRT 运行时路径，只在 Linux 上做过交叉类型检查。
3. Windows 安装器的实际安装、升级、静默安装、卸载、右键菜单与 SendTo。
4. 图形界面的真实窗口渲染与交互，只验证了无窗口 `--selftest`。
5. AppImage 产物：已生成并运行 `--selftest`，未在真实桌面会话验证窗口、托盘与桌面集成。
6. deb 包的实际安装与卸载，只用 `dpkg-deb --info` 检查过产物结构。
7. 带真实旧数据的迁移，只验证了空目录与缺失目录。
8. 4 GiB 以上大文件的真实传输与磁盘满场景，只验证了 4 GiB 边界拒绝。
9. 退出码 3 的真实后端触发路径，mock 后端不跨进程 busy。
10. Ctrl+C 打断真实长传输与真实断线重连；续传语义用脚本化传输与 CLI 端到端用例验证。

完整清单见 `docs/ACCEPTANCE.md`，发布侧边界见 `docs/RELEASE.md`。

## 数据与迁移

数据目录：

```text
配置根/BluetoothTransfer/config.toml      仅写非默认值
数据根/BluetoothTransfer/records.db       SQLite 记录库
数据根/BluetoothTransfer/logs/            日志
数据根/BluetoothTransfer/journal/         BTJ1 断点续传索引 send-journal.bin
数据根/BluetoothTransfer/auth.token       OPP 认证密码，权限 0600
数据根/BluetoothTransfer/pending/         打包与续传临时产物
下载目录/BluetoothReceive/                 默认接收目录
```

配置根与数据根按平台拆分：Linux 使用 `$XDG_CONFIG_HOME` 与 `$XDG_DATA_HOME`，未设置时用 `$HOME/.config` 与 `$HOME/.local/share`；Windows 使用 `%APPDATA%` 与 `%LOCALAPPDATA%`。设置 `BT_HOME` 时切换为便携布局。

从旧 .NET 版本升级：

```bash
bt migrate --dry-run --json
bt migrate --from "%APPDATA%/BluetoothTransfer" --json
```

迁移覆盖旧配置、历史记录与发送 journal，重复执行幂等；省略 `--from` 时按平台配置目录推断。图形界面在设置页提供同一入口。

卸载保留策略：Windows 安装器卸载时询问是否同时删除用户数据，选择否即保留；Linux 下应用写入 `~/.local/share` 与 `~/.config/autostart` 的用户级右键菜单与开机自启项不由包管理器删除，可在设置页移除。

## 仓库结构

单一 Rust workspace，依赖方向单向：

```text
bt-core ← bt-api ← bt-store / bt-transport ← bt-engine ← bt-app ← bt-gui / bt-cli
                                                        ↑
                                                   bt-platform
```

| 路径 | 内容 |
| --- | --- |
| `crates/bt-core` | 传输层无关模型：设备标识、通道、配置、记录、错误、路径与哈希 |
| `crates/bt-api` | 冻结契约：`Backend` trait、事件模型与 DTO，GUI 与 CLI 唯一依赖的接口 |
| `crates/bt-store` | SQLite 记录库、TOML 配置、流式导出与旧格式迁移 |
| `crates/bt-transport` | 蓝牙抽象：Linux 走 BlueZ D-Bus 加 RFCOMM，Windows 走 WinRT RFCOMM 与配对代理 |
| `crates/bt-engine` | 助手协议状态机、OBEX 推送、顺序队列、journal、断点续传、打包与接收服务 |
| `crates/bt-app` | 装配根，`build_backend` 拼装 `Arc<dyn Backend>` |
| `crates/bt-platform` | 单实例、外部收件箱、Shell 集成、通知、剪贴板、对话框、自启与窗口状态 |
| `crates/bt-gui` | eframe/egui 界面、平台集成与无窗口自检入口 |
| `crates/bt-cli` | `bt` 完整命令集与 `btrecv` 接收薄壳 |
| `xtask` | `cargo xtask msrv` 等仓库任务 |
| `packaging/` | NSIS 安装器脚本、deb 与 AppImage 构建脚本、图标生成 |
| `skills/` | 面向 Agent 的 CLI 指引与脚本 |
| `docs/` | 架构、协议、CLI、验收、发布与评审记录 |
| `legacy/dotnet/` | 重构前的 Windows .NET 8 实现，只读参考，不参与 workspace 构建、测试与发布 |

协议字节级细节见 `docs/PROTOCOL.md`，架构与依赖约束见 `docs/ARCHITECTURE.md`。

## 许可证

MIT，见 `LICENSE`。
