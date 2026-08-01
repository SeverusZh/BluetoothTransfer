# BluetoothTransfer

轻量级 Windows 桌面应用，通过蓝牙在设备间快速传输文本与文件，并配套完整的传输记录管理。

## 功能特性

### 核心传输
- **文本传输** — 发送文本框内容到对端，接收端一键复制到剪贴板
- **文件传输** — 单文件 / 多文件批量 / 文件夹递归传输
- **拖拽发送** — 拖拽文件或文件夹到窗口即可发送
- **双通道传输** — BLE（控制/文本/小文件）+ 经典蓝牙 RFCOMM（大文件）
- **通用推送（1.1）** — 仅发送端运行本应用，即可向任意支持“蓝牙文件接收”（OBEX OPP）的设备推送文件，接收端无需安装或运行任何软件

### 传输体验
- 实时进度条（发送进度百分比）
- 接收文本自动复制到剪贴板（可配置开关）
- 系统托盘常驻：关闭窗口最小化到托盘，托盘菜单可恢复 / 退出

### 设备管理
- 周边蓝牙设备扫描，列表展示（名称/地址/信号强度）
- 本机广播名称可配置
- 已连接设备自动记录最近连接时间

### 安全与完整性
- 端到端加密（ECDH P-256 密钥协商 + AES-GCM 加密）
- 文件校验（SHA-256），接收端自动验证完整性
- 断点续传 — 中断后重连自动从偏移量续传
- 可选 Deflate 压缩后再传输

### 记录与统计
- 所有收发行为自动写入 SQLite
- 传输记录列表（时间 / 方向 / 类型 / 对端 / 名称 / 大小 / 状态 / 通道）
- 导出 CSV / JSON
- 手动刷新记录

### 通用推送模式（1.1）
- **适用场景** — 发送端电脑（内置或外置 USB 蓝牙适配器均可）直接向 Android 手机/平板、Windows 电脑、功能机等推送文件
- **工作原理** — 通过标准蓝牙 OBEX Object Push Profile（OPP，服务 UUID `0x1105`），接收端使用系统原生“蓝牙文件接收”功能即可
- **内容形态** — 单文件直接推送；文本自动包装为 `.txt`；文件夹自动压缩为 `.zip` 后推送（默认文件名 `bt-note.txt`）
- **设备管理** — 应用内扫描 OPP 设备、发起配对（ConfirmOnly / PIN 两种流程），失败可引导打开 Windows 蓝牙设置手动配对
- **限制说明（协议固有限制，需知悉）**
  - **iPhone（iOS）不支持蓝牙文件接收协议，无法通过 OPP 推送**
  - OPP 为明文传输：无端到端加密、无接收端校验回传、无断点续传；需要这些能力时请使用 1.0 双端互联模式
  - 接收端需先配对；Android 接收时通常需要在手机上确认“接受文件”
  - 单文件上限 4GB（OBEX 协议约束），建议不超过 2GB
  - 未配对 / 设备不支持 OPP / 接收端拒绝 / 超时等失败场景均会写入传输记录（channel=opp）便于排查
  - Android 同时只接受一个 OPP 传输：若对端已有未完成/待确认的传输，新推送会被拒绝（0xC3 Forbidden），请先在对端完成或取消上一次传输

#### Windows 兼容性说明（1.1.1）
- Windows 作为发送端：实现路径与微软官方文档 *RFCOMM Scenario: Send File as a Client* 一致（`RfcommServiceId.ObexObjectPush` + `StreamSocket`），并与 32feet.NET 的 `ObexWebRequest`（Windows 蓝牙栈 OPP 客户端）逐字节对齐，Windows 10 2004+ / Windows 11 上可行。
- Windows 作为接收端：由系统"蓝牙文件传输向导"（fsquirt）承载，接收端无需本应用；若 Win11 设置页找不到"通过蓝牙发送或接收文件"入口，可运行 `fsquirt` 或检查注册表 `DisableFsquirt`。
- 出站 RFCOMM 仅支持**已配对**设备（Windows 蓝牙栈限制）；OPP 连接默认使用与 Android 真机验证一致的明文（PlainSocket），部分接收端（如 Windows 向导）可能要求加密，可用 `btcli config set OppProtectionLevel encrypt` 强制加密（auto/plain/encrypt）。
- 双 Windows 主机互传属于真机验收项：A 机本应用发送 → B 机系统向导接收；B 机本应用发送 → A 机系统向导接收。发布前需按测试矩阵执行。

## 技术栈

| 层 | 技术 |
|---|---|
| 语言 / 运行时 | C# + .NET 8 |
| GUI 框架 | WPF（MVVM 模式） |
| 蓝牙 BLE | WinRT `Windows.Devices.Bluetooth.Advertisement` + `Windows.Devices.Bluetooth.GenericAttributeProfile` |
| 蓝牙经典 (RFCOMM) | WinRT `Windows.Devices.Bluetooth.Rfcomm` |
| 蓝牙 OBEX/OPP（1.1） | 自研 OBEX 客户端（CONNECT/PUT/DISCONNECT，两阶段 PUT：头包→Body 分片；UTF-16BE 无 BOM Name 头，头长度字段按 OBEX 规范含头 ID），基于 `RfcommServiceId.ObexObjectPush` |
| 加密 | `System.Security.Cryptography`（ECDH P-256 / AES-GCM / SHA-256） |
| 存储 | Microsoft.Data.Sqlite (SQLite) + System.Text.Json |
| 系统托盘 | Hardcodet.NotifyIcon.Wpf |
| 发布 | 自包含单文件发布（trim），免装运行时 |

## 系统架构

```
┌──────────────────────────────────────────────┐
│                UI 层 (WPF / MVVM)             │
│ 设备面板│发送区│接收区│记录面板│日志           │
└───────────────┬──────────────────────────────┘
                │ 命令 / 数据绑定
┌───────────────▼──────────────────────────────┐
│              应用服务层 (App Core)             │
│ 设备管理│传输调度│安全│事件总线                │
└───────┬───────────────────────┬──────────────┘
        │                       │
┌───────▼───────────┐   ┌──────▼────────────────┐
│   蓝牙传输层       │   │   存储层               │
│ BLE GATT           │   │  SQLite 记录库 / 设备库│
│ RFCOMM 流式        │   │  配置 JSON             │
│ 分帧/CRC/重组/续传  │   │  接收文件落盘管理       │
└───────────────────┘   └───────────────────────┘
```

## 快速开始

### 前置条件
- Windows 10 版本 2004 及以上 / Windows 11
- 蓝牙适配器（支持 BLE + 经典蓝牙）
- [.NET 8 SDK](https://dotnet.microsoft.com/zh-cn/download/dotnet/8.0) （含 Windows 桌面工作负载）

### 构建与运行

```bash
# 克隆仓库
git clone https://github.com/SeverusZh/BluetoothTransfer.git
cd BluetoothTransfer

# 还原依赖
dotnet restore

# 构建
dotnet build -c Release

# 运行
dotnet run --project src/BluetoothTransfer -c Release
```

### 自包含单文件发布

```bash
dotnet publish -r win-x64 -c Release `
  -p:PublishSingleFile=true `
  --self-contained true
```

发布产物位于 `src/BluetoothTransfer/bin/Release/net8.0-windows10.0.19041.0/win-x64/publish/`。

## 命令行工具（btcli）

项目附带一个与桌面应用共享同一套核心服务的 CLI（`src/BluetoothTransfer.Cli`），
覆盖全部核心功能，主要用于脚本自动化与双机真实交互测试（不依赖 WPF）。

### 构建

```bash
dotnet build src/BluetoothTransfer.Cli -c Release
```

运行（Debug 构建产物）：

```bash
.\src\BluetoothTransfer.Cli\bin\Debug\net8.0-windows10.0.19041.0\btcli.exe --help
```

### 命令一览

| 类别 | 命令 | 说明 |
|---|---|---|
| 扫描 | `btcli scan [--seconds 5] [--json]` | 扫描周边 BLE 设备（名称/地址/RSSI） |
| 服务端 | `btcli serve [--name 名称] [--ble-only] [--timeout 秒] [--json]` | 启动 BLE 广播 + RFCOMM 服务端，实时显示收到的文本/文件 |
| 连接 | `btcli connect <地址> [--peer 名称] [--no-rfcomm]` | 连接对端并保持会话（可实时接收对端数据） |
| 发送 | `btcli send-text <地址> <文本>` | 发送文本（BLE） |
| 发送 | `btcli send-file <地址> <文件> [--compress\|--no-compress] [--encrypt\|--no-encrypt] [--chunk 字节]` | 发送文件（>10KB 自动走 RFCOMM，小文件走 BLE） |
| 发送 | `btcli send-folder <地址> <文件夹> [同上]` | 递归发送文件夹内全部文件 |
| 通用推送 | `btcli opp-scan [--seconds 秒] [--paired-only] [--json]` | 扫描支持 OPP（蓝牙文件接收）的经典蓝牙设备 |
| 通用推送 | `btcli opp-pair <地址> [--pin 1234]` | 发起配对（无 PIN 走系统确认流程） |
| 通用推送 | `btcli opp-send-file <地址> <文件>` | 向任意支持 OPP 的设备推送文件（无需对方运行本应用） |
| 通用推送 | `btcli opp-send-text <地址> <文本> [--name 文件名]` | 文本包装为 .txt 后推送 |
| 通用推送 | `btcli opp-send-folder <地址> <文件夹>` | 文件夹压缩为 .zip 后推送 |
| 记录 | `btcli records [--direction send\|recv] [--type text\|file] [--status ok\|failed] [--peer 地址] [--search 关键词] [--limit 数量] [--json]` | 查询传输记录 |
| 清空记录 | `btcli records clear [--yes]` | 清空全部传输记录（交互确认，--yes 跳过确认） |
| 导出 | `btcli export <csv\|json> <输出文件>` | 导出全部记录 |
| 统计 | `btcli stats [--json]` | 收发统计 |
| 设备 | `btcli devices [--json]` | 已记录设备 |
| 配置 | `btcli config` / `btcli config set <key> <value>` | 查看/修改配置（RecvDirectory、AutoCopyClipboard、CompressionEnabled、EncryptionEnabled、RfcommChunkSize、OppChunkSize、OppConnectTimeout、OppSendTimeout、PushTextFileName、OppAuthPassword、OppNameUseBom、OppProtectionLevel） |
| 自检 | `btcli selftest` | 无硬件自检：分帧/CRC、ECDH+AES-GCM、Deflate、存储与导出、OBEX 编解码与假传输流程 |
| 交互 | `btcli repl` | 交互式会话，一条进程内完成扫描/连接/收发/查询，便于模拟真实交互 |

所有读写记录的命令支持 `--db <路径>` 指定独立数据库，避免测试污染真实记录。

### 双机交互测试示例

```bash
# 机器 A（接收端）
btcli serve --name TestPC

# 机器 B（发送端）
btcli scan --seconds 5
btcli connect A0E9F1D27B3C --peer TestPC
btcli send-text A0E9F1D27B3C "hello from CLI"
btcli send-file A0E9F1D27B3C C:\tmp\doc.pdf --compress --encrypt

# 机器 A 上可实时看到收到的文本与文件落盘路径，再核对记录：
btcli records --direction recv
```

### 通用推送示例（1.1，无需接收端运行本应用）

```bash
# 扫描支持蓝牙文件接收的设备（手机需开启蓝牙，电脑端建议先配对）
btcli opp-scan --seconds 8

# 未配对设备先配对（可选 --pin；Android 手机需在手机上确认）
btcli opp-pair 00:11:22:33:44:55

# 推送文件 / 文本 / 文件夹
btcli opp-send-file 00:11:22:33:44:55 C:\tmp\photo.jpg
btcli opp-send-text 00:11:22:33:44:55 "你好，这是蓝牙推送的文本" --name note.txt
btcli opp-send-folder 00:11:22:33:44:55 C:\tmp\docs

# 核对记录（channel=opp）
btcli records --direction send --json
```

`repl` 模式可在同一会话里完成全流程（`scan` → `serve`/`connect` → `send-text`/`send-file` → `records`），
并实时打印对端发来的文本、文件路径与传输进度，适合模拟真实用户交互。

## 项目结构

```
BluetoothTransfer/
├── src/BluetoothTransfer/        # 主应用（WPF 桌面端）
│   ├── Models/                   # 数据模型（TransferRecord/DeviceInfo/AppConfig）
│   ├── Services/                 # 核心服务层
│   │   ├── BleService.cs         # BLE 扫描/广播/连接
│   │   ├── BleGattServer.cs      # BLE GATT 服务端
│   │   ├── RfcommChannel.cs      # RFCOMM 流式通道
│   │   ├── Protocol.cs           # 统一分帧协议（CRC/加密/续传）
│   │   ├── ObexPacket.cs         # OBEX 包/头编解码（1.1）
│   │   ├── IObexTransport.cs     # OBEX 传输抽象（1.1）
│   │   ├── SocketObexTransport.cs# RFCOMM StreamSocket 传输（1.1）
│   │   ├── MemoryObexTransport.cs# 内存假传输（测试/自检，1.1）
│   │   ├── ObexClient.cs         # OBEX CONNECT/PUT/DISCONNECT 状态机（1.1）
│   │   ├── OppDiscoveryService.cs# OPP 设备发现与配对（1.1）
│   │   ├── OppPushService.cs     # OPP 推送编排（文本/文件夹打包，1.1）
│   │   ├── FileChecksum.cs       # 文件 SHA-256 助手（1.1）
│   │   ├── FileTransferService.cs# 文件传输调度
│   │   ├── CryptoService.cs      # ECDH + AES-GCM 加解密
│   │   ├── StorageService.cs     # SQLite 持久化
│   │   ├── ExportService.cs      # CSV/JSON 导出
│   │   ├── EventBus.cs           # 事件总线
│   │   └── TransferState.cs      # 传输状态机
│   ├── ViewModels/               # MVVM ViewModel（MainViewModel + OppViewModel 1.1）
│   ├── App.xaml / MainWindow.xaml
│   └── BluetoothTransfer.csproj
├── src/BluetoothTransfer.Cli/    # 命令行工具（btcli，与服务层共享源码）
│   ├── Program.cs / CliApp.cs    # 入口与命令分发
│   ├── CliSession.cs             # 服务装配与事件输出
│   ├── Commands.cs               # 全部命令实现 + selftest + repl
│   └── BluetoothTransfer.Cli.csproj
├── tests/FeasibilityTest/        # BLE + RFCOMM 可行性验证
├── tests/BluetoothTransfer.Tests/# xUnit 单元测试（OBEX 编解码/状态机/推送流程，无硬件）
├── BluetoothTransfer.sln
└── README.md
```

## 通信协议

双通道协同设计：

- **BLE GATT** — 控制通道：设备发现、连接握手、ECDH 密钥协商、传输元数据交换、文本/小文件传输、心跳/续传偏移信令
- **RFCOMM** — 数据通道：大文件批量流式传输，较大分块（4KB），ACK 回传续传偏移

应用层统一分帧格式：

```
| Ver(1) | Flags(1) | MsgType(1) | TaskID(4) | SeqNo(2) | TotalLen(4) | Offset(4) | ChunkLen(2) | Payload | CRC16(2) |
```

支持加密（AES-GCM）、压缩（Deflate）、断点续传（Offset 字段）。

协议约定：
- **CRC16** 固定为 **CRC-16/CCITT-FALSE**（多项式 `0x1021`、初值 `0xFFFF`、输入/输出均不反转、异或输出 `0x0000`）。与异构对端互通时须固定该变体。
- **SeqNo** 为 16 位无符号滚动计数（mod 65536），超长传输后会回绕；仅用于诊断，分片有序写入与续传一律以 **Offset** 为准。
- **加密**：BLE 连接建立后经 `KEY_EXCHANGE` 控制帧完成 ECDH P-256 密钥协商，文件 `DATA` 载荷按帧 AES-GCM 加密并置 `Flags.Encrypted`（`META` 保持明文）；校验和始终针对未压缩、未加密的原始文件。

## 传输记录存储

SQLite 数据库，`transfer_records` 表包含：时间、方向、类型、文件名/摘要、大小、对端设备、状态、SHA-256 校验值、传输通道、本地路径等字段；`devices` 表记录设备名称与最近连接时间（表结构预留别名、收藏字段）。

## 开发状态

**v1.0**：核心收发链路（BLE 文本 / 小文件、RFCOMM 大文件、端到端加密、压缩、断点续传、记录入库与导出）已实现并通过编译验证。

**v1.1**：新增通用推送模式（OBEX OPP）——仅发送端运行本应用即可向任意支持“蓝牙文件接收”的设备推送文件，覆盖 Android 手机/平板、Windows 电脑、功能机等，内置/外置蓝牙适配器均适用。配套新增 `opp-*` CLI 命令、GUI「通用推送」面板、OBEX 协议层与 xUnit 自动化单元测试（`dotnet test`），离线自检 `btcli selftest` 全部通过。

**v1.1.1**：GUI 视觉升级（SVG 图标 + 统一主题样式），新增传输记录清空（GUI「清空」按钮 / `btcli records clear [--yes]`），并按微软官方 RFCOMM 示例与 32feet.NET 实现补充 Windows 兼容性增强（OPP 连接保护级别 auto/plain/encrypt 自适应、SDP 服务预检）。应用图标矢量源位于 `src/BluetoothTransfer/Assets/app.svg`（与 `AppIcons.xaml` 同源），打包产物由 `tools/IconGenerator` 生成（`dotnet run --project tools/IconGenerator`）。

蓝牙相关功能建议在具备蓝牙硬件（内置或外置 USB 适配器）的 Windows 10 2004+ / Windows 11 机器上按「快速开始」流程做端到端验证。真机验证覆盖：内置/外置适配器 × Android/Windows 接收端；小文件（1KB）/ 大文件（1MB、100MB）；中文与 emoji 文件名；文本推送；文件夹 zip；未配对、接收端拒绝、无 OPP 能力设备、传输中取消与超时等异常场景；以及对 1.0 双端互联全流程的回归验证。

## 许可

[MIT](LICENSE)
