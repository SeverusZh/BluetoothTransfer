# 蓝牙传输

[![Version](https://img.shields.io/badge/version-2.0.0-blue)](CHANGELOG.md)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

跨平台蓝牙文件传输工具，Rust 重写版本。图形界面与命令行共用同一套后端，覆盖设备发现、配对、OPP 与私有助手通道发送、断点续传、接收、记录、迁移与自检。

- 仓库：https://github.com/SeverusZh/BluetoothTransfer
- 版本：2.0.0
- 许可证：MIT

## 功能

- 设备：扫描、已配对过滤、配对、收藏、别名、助手通道三态探测。
- 发送：单文件、多文件、文本、文件夹自动打包、单文件 zip、通道 auto/assistant/opp、顺序队列、逐任务暂停/继续/取消/重试/移除。
- 续传：失败或中断的发送按 journal 恢复，私有助手通道支持分片级续传。
- 接收：常驻监听、单活跃会话、落盘目录、接收前询问、SHA-256 校验、重名避让、半成品保留。
- 记录：方向/类型/状态/对端/关键词筛选、统计、单条删除、清空、复制、CSV 与 JSONL 导出。
- 设置：12 项普通配置加 1 项 OPP 认证密码；密码只存私密位置，不进配置文件、不回显。
- 桌面：托盘与通知、单实例与外部收件箱、Windows 资源管理器右键菜单与 SendTo、Linux .desktop 集成、窗口状态记忆、可选开机自启。
- CLI：全部能力可脚本化，`--json` 稳定结构，`--events` 输出 NDJSON，退出码区分语义。
- 迁移与自检：旧 .NET 数据一次性迁移，无蓝牙硬件可运行自检。

## 架构

单一 Rust workspace，依赖方向单向：

```text
bt-core ← bt-api ← bt-store / bt-transport ← bt-engine ← bt-app ← bt-gui / bt-cli
                                                        ↑
                                                   bt-platform
```

- `bt-api` 是 GUI 与 CLI 唯一依赖的冻结契约，含 `Backend` trait 与事件模型。
- `bt-app` 是唯一装配点，GUI 与 CLI 通过 `build_backend` 取 `Arc<dyn Backend>`。
- `bt-engine` 负责协议、队列、journal、断点续传、接收与探测；`bt-store` 负责 SQLite 与配置；`bt-transport` 封装 BlueZ 与 WinRT。
- `bt-platform` 提供托盘、通知、剪贴板、单实例等平台能力，由 bt-gui 与 bt-cli 按需调用。

细节见 `docs/ARCHITECTURE.md`、字节级协议见 `docs/PROTOCOL.md`、CLI 契约见 `docs/CLI.md`。

## 安装

### Windows

使用 CI 发布的 NSIS 安装器。默认按用户安装，包含开始菜单项、卸载入口与可选资源管理器右键菜单；升级保留配置、记录与接收目录。

### Linux

使用 CI 发布的 `.deb` 包或 AppImage。包内包含 `bt`、`btrecv` 与 `bt-gui`，并安装 `.desktop` 文件。

### 从源码构建

开发机若提供仓库内工具链，先加载环境；没有该目录时使用系统 Rust 1.95 及以上版本（最低版本由 eframe/egui 0.37 的 `rust-version` 决定）：

```bash
git clone https://github.com/SeverusZh/BluetoothTransfer.git
cd BlueToothTransfer
. ./.toolchain/env.sh
```

构建 CLI：

```bash
cargo build --release -p bt-cli --no-default-features
# 产物：target/release/bt 与 target/release/btrecv
```

构建 GUI 与完整版 `bt`：

```bash
cargo build --release -p bt-gui
cargo build --release -p bt-cli
# 产物：target/release/bt-gui 与 target/release/bt
```

系统已安装 Rust 1.95 及以上版本时可跳过 `.toolchain/env.sh`；`cargo xtask msrv` 可校验声明与依赖要求一致。

## 使用

### 图形界面

运行 `bt-gui`，或完整版 `bt` 不带子命令。页面包括设备、发送、接收、记录、日志与设置：

- 设备页：扫描与取消、设备列表、收藏、别名、配对、助手通道探测。
- 发送页：文本、文件、文件夹、剪贴板与拖放发起，任务进度、暂停、继续、取消、重试。
- 接收页：开始/停止、落盘目录、询问开关、进度与完成列表。
- 记录页：筛选、搜索、复制、删除、清空、导出。
- 设置页：12 项配置、OPP 认证密码、迁移、自检。

无显示环境可运行 `bt-gui --selftest` 验证依赖与状态构建。无蓝牙硬件可运行 `bt selftest`；硬件相关检查标记为环境跳过，不影响通过。

### 命令行

常用流程：

```bash
bt devices scan --seconds 8 --json
bt devices pair AA:BB:CC:DD:EE:FF --json
bt send file AA:BB:CC:DD:EE:FF ./report.pdf --json
bt records list --direction send --limit 5 --json
bt recv --dir ~/Downloads/BluetoothReceive --no-ask --events
bt selftest --json
```

- `bt --help` 查看全部命令与参数。
- 配对需要 PIN 时由终端提示输入，或使用 `bt devices pair <ADDR> --pin <PIN>`；PIN 不进入日志、记录与 JSON。
- `bt recv` 或 `btrecv` 无参数即开始监听，Ctrl+C 停止并以 0 退出。
- 需要机器可读进度时追加 `--events`，按行解析 NDJSON，最后一行是 `type=result`。
- 无硬件环境用 `BT_BACKEND=mock` 验证契约，用 `BT_HOME` 隔离数据目录；模拟设备为 `AA:BB:CC:00:00:01` 与 `AA:BB:CC:00:00:02`。

完整命令矩阵、JSON schema、事件类型与退出码见 `docs/CLI.md`。面向 Agent 的可执行指引见 `skills/bluetooth-transfer/SKILL.md`。

## 协议与限制

- OPP 走标准 OBEX 对象推送，兼容 Android 与常见蓝牙文件接收端；传输为明文，不要在不可信链路发送敏感数据。
- iOS 不支持 OPP 接收；向 iPhone 发送文件需要对方运行本应用接收端或改用其他方式。
- 私有助手通道要求收发两端都运行本应用；普通 OPP 只要求接收端支持蓝牙文件接收。
- OPP 单包上限 65023 字节；助手通道单帧上限 1 MiB，默认分片 256 KiB。
- 配对 PIN 由 CLI 的 `--pin` 或 GUI 对话框回填到传输层，只用于本次配对，不进入日志、记录与 JSON。
- 接收端单实例：同一时刻只有一个接收会话占用 RFCOMM 通道。
- 接收文件名会被清洗为单段名称，拒绝 `..` 穿越；重名自动追加 ` (n)`。

## 数据与体积

数据目录：

```text
配置根/BluetoothTransfer/config.toml      仅写非默认值
数据根/BluetoothTransfer/records.db       SQLite 记录库
数据根/BluetoothTransfer/logs/            日志
数据根/BluetoothTransfer/journal/         BTJ1 断点续传索引 send-journal.bin
数据根/BluetoothTransfer/auth.token       OPP 认证密码，权限 0600
下载目录/BluetoothReceive/                 默认接收目录
数据根/BluetoothTransfer/pending/         打包与续传临时产物
```

设置 `BT_HOME` 时使用便携布局：配置在 `$BT_HOME/.config/BluetoothTransfer/`，数据在 `$BT_HOME/.local/share/BluetoothTransfer/`，默认接收目录为 `$BT_HOME/Downloads/BluetoothReceive`。

轻量化指标与上限：

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
| .deb | 20 MB | 约 5.67 MB（字节数不可复现，随二进制与打包时间戳波动；方法见 docs/RELEASE.md） |
| NSIS 安装器 | 15 MB | 本机无 makensis，未生成 |

体积实测方法与最终数字见 `docs/RELEASE.md` 与 `docs/ARCHITECTURE.md`；协议与导出指标由单元测试固化，CLI 侧实测值见 `docs/ACCEPTANCE.md`。

## 开发与测试

```bash
. ./.toolchain/env.sh
export CARGO_TARGET_DIR=$PWD/target/t4   # 可选，隔离并行构建

cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p bt-cli --no-default-features
```

CLI 端到端测试不依赖蓝牙硬件：

```bash
cargo test -p bt-cli
```

CI 在 Ubuntu 与 Windows 运行格式化、静态检查、测试、发布构建与 GUI 自检。蓝牙真机验收需要记录设备型号、系统版本与操作步骤；本仓库不把模拟结果当作真机结论。

## 迁移

从旧 .NET 版本升级：

```bash
bt migrate --from "%APPDATA%/BluetoothTransfer" --json
bt migrate --dry-run --json
```

迁移覆盖旧配置、历史记录与发送 journal，重复执行幂等。图形界面在设置页提供同一入口。

## 旧版实现归档

重构前的 Windows .NET 8 实现在 `legacy/dotnet/`，只读参考，不参与构建、测试与发布。旧版数据用 `bt migrate` 迁移，详见 `legacy/README.md`。

## 许可证

MIT，见 `LICENSE`。
