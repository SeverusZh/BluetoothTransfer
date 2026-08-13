# 🔵 BluetoothTransfer

<div align="center">

**轻量级 Windows 桌面应用：通用蓝牙推送 + Windows 双向互传**

[![Version](https://img.shields.io/badge/version-1.4.0-0B6BCB)](https://github.com/SeverusZh/BluetoothTransfer/releases)
[![.NET](https://img.shields.io/badge/.NET-8-512BD4?logo=dotnet)](https://dotnet.microsoft.com/)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%202004%2B%20%2F%20Windows%2011-0078D4?logo=windows)](https://www.microsoft.com/windows)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-184%20passed-success)](#开发与测试)

*📤 把文件推给任何蓝牙设备 · 🔄 Windows 之间断点续传 · 🛡️ SHA-256 校验 · 🧰 免安装接收端*

</div>

---

## ✨ 是什么

- 通过标准蓝牙 **OBEX Object Push Profile（OPP）** 把文件、文本、文件夹推送给任意支持「蓝牙文件接收」的设备（Android 手机/平板、Windows 电脑、功能机等），**接收端无需安装任何软件**；
- 两台 Windows 电脑之间可走**接收助手通道**（完整 GUI 内置接收面板，或独立极简接收端 `btrecv`）：断点续传、SHA-256 校验、可管理的传输队列；发送前自动探测——助手在线走私有通道，离线自动回退 OPP。

> ℹ️ v1.2 起聚焦「通用推送 + 接收助手」双通道；v1.3 引入 Windows 对 Windows 断点续传通道；**v1.4 全面加固**：续传可靠性修复、队列并发修复、协议输入硬化、安全默认值（接收默认询问）、可测试性改造与 184 项自动化测试。

## 📖 目录

- [功能特性](#-功能特性)
- [快速开始](#-快速开始)
- [CLI 命令参考](#-cli-命令参考)
- [配置项](#-配置项)
- [Windows 兼容性说明](#-windows-兼容性说明)
- [协议限制（需知悉）](#-协议限制需知悉)
- [发布产物](#-发布产物-v140)
- [技术栈与架构](#-技术栈与架构)
- [开发与测试](#-开发与测试)
- [许可](#-许可)

## 🎯 功能特性

### 📤 通用推送（OPP）

| 能力 | 说明 |
|---|---|
| **发送形态** | 单文件直接推送；文本自动包装为 `.txt`（默认 `bt-note.txt`）；文件夹自动压缩为 `.zip`；单文件可选 `--zip`；剪贴板文本/图片直推；文件/文件夹**拖入窗口即推送** |
| **发现与配对** | 扫描支持 OPP 的经典蓝牙设备（可只显示已配对）；应用内发起系统配对（ConfirmOnly / PIN）；失败可一键打开 Windows 蓝牙设置 |
| **自动重试** | 临时失败（连接失败 / 超时 / 对端忙 0xC3 / 断开）按**指数退避**自动重试（默认 3 次，3/6/12s，可配置），失败记录含「已重试 N 次」与失败阶段 |
| **发送队列** | 多文件/文件夹统一排队顺序推送（Android OPP 同时只接受一个传输）；支持取消全部、失败任务单点重发 |
| **传输体验** | 实时进度条 + 速率（KB/s）与剩余时间（ETA）；系统托盘常驻 |

### 🔄 接收助手（btrecv · Windows ⇄ Windows）

- **断点续传** — 私有分片协议（自定义 RFCOMM UUID）：中断后从**已确认偏移**继续，接收端保留半成品（`.btpart` + `.btpart.meta`），重连自动续传；完成时校验 SHA-256，不匹配自动清空重传
- **真正的传输队列** — 任务支持单独**暂停 / 继续 / 移除 / 重试**；接收端忙时自动退避等待；发送端显示通道列（OPP / 助手）
- **自动探测 + 手动覆盖** — 发送前 SDP 探测对端是否运行助手：在线走私有通道，离线自动回退 OPP；可在 GUI/CLI 强制指定通道
- **极简接收端** — `btrecv` 单 exe：CLI 内核（`btrecv run [--dir 目录] [--no-ask]`）+ 极简 GUI 壳；**默认每次接收前询问**（防止已配对设备直接落盘）；先用 OPP 把 `btrecv` 包推过去，解压即用（自包含无需安装 .NET）
- **链路加密可配置** — 助手通道与 OPP 通道共用 `OppProtectionLevel` 配置，`encrypt` 启用链路加密（RFCOMM 加密由发送端请求、系统协商）；应用层仍无端到端加密

### 🖥️ 完整 GUI 内置接收

- 主窗口「接收助手」面板：开始/停止监听、当前传输进度、本会话完成列表、保存目录（默认 `下载\BluetoothReceive`）、每次接收前询问
- **两台主机运行完整 GUI 即可互传**（或一方用完整 GUI、一方用 `btrecv`）；接收记录写入同一 SQLite（direction=recv），含对端蓝牙地址与名称
- 与 `btrecv` 二选一运行（同一服务 UUID）

### ⭐ 设备管理

- 设备列表展示（名称/地址/配对状态），支持**收藏置顶**、**别名显示**、按最近连接排序
- **收藏夹**：收藏持久化在本地 SQLite，启动/扫描后自动出现在列表；对从未发送过的设备也可收藏
- 已连接设备自动记录最近连接时间

### 📊 记录与统计

- 所有推送自动写入 SQLite（时间/方向/类型/对端/名称/大小/状态/通道/SHA-256/失败原因）
- 记录列表（GUI）/ `btcli records`（CLI）查询与筛选
- 导出 CSV / JSON，统计汇总，**清空记录**（GUI 按钮带确认，或 `btcli records clear [--yes]`）
- 1.0/1.1 旧记录（`channel=ble/rfcomm`、`direction=recv`）只读兼容，不会被新版本覆盖

## 🚀 快速开始

### 一、GUI 推送

1. 启动应用，点击「扫描」发现设备（需先在本机 Windows 设置中与对端配对）
2. 选中目标设备（可点击星标收藏）
3. 发送文本 / 选择文件 / 选择文件夹 / 剪贴板，或直接把文件拖入窗口
4. 接收端（如 Android 手机）保持屏幕点亮，在通知栏点「接受文件」

### 二、两台 Windows 主机互传

1. 双方都运行本应用（或一方运行本应用、一方运行接收助手 `btrecv`）
2. 接收方在「接收助手」面板点「开始监听」（默认保存到 `下载\BluetoothReceive`）
3. 发送方扫描/收藏到对方设备后直接发送——自动走接收助手通道（断点续传 + SHA-256 校验）
4. 未检测到助手时自动回退 OPP 通用推送

### 三、CLI（`btcli`）

```powershell
btcli opp-scan --seconds 5
btcli opp-pair 00:11:22:33:44:55 --pin 1234
btcli opp-send-file 00:11:22:33:44:55 C:\tmp\photo.jpg
btcli opp-send-files 00:11:22:33:44:55 a.pdf b.pdf --zip
btcli opp-send-text 00:11:22:33:44:55 "你好，这是蓝牙推送的文本" --name note.txt
btcli opp-send-folder 00:11:22:33:44:55 C:\data\docs
```

### 四、接收助手引导（Windows 对 Windows）

1. 发送端先通过 OPP 把 `btrecv-selfcontained.zip` 推给目标电脑（GUI「发送文件」或 `btcli opp-send-file <地址> <包路径>`）
2. 对端解压、双击运行 `btrecv`（进入 GUI「监听中」，无需安装 .NET）
3. 发送端重新扫描，设备出现绿色「助手」徽标即代表探测成功；或运行 `btcli detect <地址>`
4. 之后默认走可续传的私有通道；对端未运行时自动回退 OPP
5. 接收助手**默认每次接收前询问**（内置接收默认 `ReceiveAsk=true`，btrecv 默认询问，`--no-ask` 关闭）

## ⌨️ CLI 命令参考

| 分类 | 命令 | 说明 |
|---|---|---|
| 设备 | `btcli opp-scan [--seconds 秒] [--paired-only] [--json]` | 扫描支持 OPP 的经典蓝牙设备 |
| 设备 | `btcli opp-pair <地址> [--pin 1234]` | 发起配对（无 PIN 走系统确认流程） |
| 发送 | `btcli opp-send-file <地址> <文件> [--zip]` | 推送文件（`--zip` 先打包再推送） |
| 发送 | `btcli opp-send-files <地址> <文件1> [文件2 ...] [--zip]` | 批量推送并汇总成功/失败 |
| 发送 | `btcli opp-send-text <地址> <文本> [--name 文件名]` | 文本包装为 .txt 后推送 |
| 发送 | `btcli opp-send-folder <地址> <文件夹>` | 文件夹压缩为 .zip 后推送 |
| 发送 | `btcli opp-send-file\|files\|text\|folder ... [--mode auto\|assistant\|opp]` | 选择通道：`auto` 助手优先失败回退 OPP / `assistant` 强制助手 / `opp` 强制 OPP（默认 opp） |
| 设备 | `btcli detect <地址>` | 探测对端是否运行接收助手 |
| 发布 | `btcli package-receiver [--root 仓库根]` | 一键发布接收助手（自包含 + 框架依赖） |
| 记录 | `btcli records [--direction send\|recv] [--type text\|file] [--status ok\|failed] [--peer 地址] [--search 关键词] [--limit 数量] [--json]` | 查询传输记录 |
| 记录 | `btcli records clear [--yes]` | 清空全部传输记录（交互确认，--yes 跳过） |
| 记录 | `btcli export <csv\|json> <输出文件>` | 导出全部记录 |
| 统计 | `btcli stats [--json]` | 收发统计 |
| 设备 | `btcli devices [--json] [--sort last\|name\|favorite]` | 已记录设备 |
| 设备 | `btcli devices favorite <地址> [--unset]` | 收藏/取消收藏设备 |
| 设备 | `btcli devices alias <地址> <别名>` | 设置设备别名（留空清除） |
| 配置 | `btcli config` / `btcli config set <key> <value>` | 查看/修改配置 |
| 测试 | `btcli selftest` | 无硬件自检（OBEX、存储、重试策略、zip、设备管理） |
| 交互 | `btcli repl` | 交互式会话 |

## ⚙️ 配置项

| 键 | 默认 | 说明 |
|---|---|---|
| `OppChunkSize` | 32768 | OPP 分片大小（字节） |
| `OppConnectTimeout` | 30 | 连接超时（秒） |
| `OppSendTimeout` | 300 | 发送超时（秒） |
| `PushTextFileName` | bt-note.txt | 文本推送默认文件名 |
| `OppAuthPassword` | （空） | 设备要求 OBEX 认证时的密码 |
| `OppNameUseBom` | false | Name 头是否带 UTF-16 BOM（默认无 BOM，兼容 Android/Windows） |
| `OppProtectionLevel` | auto | 连接保护级别：auto / plain / encrypt；**OPP 与助手通道共用**，encrypt 启用链路加密（应用层仍无端到端加密） |
| `OppRetryCount` | 3 | 失败自动重试次数（0 表示不重试） |
| `OppRetryDelaySeconds` | 3 | 重试基础间隔（秒），按 1x/2x/4x 指数退避 |
| `TransferMode` | auto | 发送通道：auto（探测助手，否则 OPP）/ assistant / opp |
| `ReceiveDirectory` | 下载\BluetoothReceive | 内置接收助手保存目录 |
| `ReceiveAsk` | **true** | 内置接收助手每次接收前询问（默认开启的安全加固）；设为 false 直接自动接收 |

## 🪟 Windows 兼容性说明

- **发送端**：实现路径与微软官方文档 *RFCOMM Scenario: Send File as a Client* 一致（`RfcommServiceId.ObexObjectPush` + `StreamSocket`），并与 32feet.NET 的 `ObexWebRequest` 逐字节对齐；Windows 10 2004+ / Windows 11 可行，内置/外置 USB 蓝牙适配器均可
- **接收端**：由系统「蓝牙文件传输向导」（fsquirt）承载；若 Win11 找不到入口，运行 `fsquirt` 或检查注册表 `DisableFsquirt`
- 出站 RFCOMM 仅支持**已配对**设备（Windows 蓝牙栈限制）；连接默认明文，要求加密时可 `config set OppProtectionLevel encrypt`
- 连接前进行 SDP 预检：确认服务声明 OPP（0x1105），不满足时给出明确错误并写入失败记录

## ⚠️ 协议限制（需知悉）

- **iPhone（iOS）不支持蓝牙文件接收协议，无法通过 OPP 推送**
- OPP 为明文传输：无端到端加密、无接收端校验回传；需要链路层加密时使用 `OppProtectionLevel=encrypt`
- **OPP 通道无断点续传**：由自动重试缓解；Windows 对 Windows 走接收助手（btrecv）私有通道时支持断点续传与 SHA-256 校验
- 单文件上限 4GB（OBEX 协议约束），建议不超过 2GB
- Android 同时只接受一个 OPP 传输：对端已有待确认传输时新推送会被拒绝（0xC3 Forbidden），应用会自动重试

## 📦 发布产物（v1.4.0）

每个产物提供两种形态：**含运行时**（自包含单文件，目标机无需安装 .NET 8）与**不含运行时**（框架依赖，目标机需安装 .NET 8 桌面运行时，体积更小）。

| 产物 | 说明 |
|---|---|
| `btrecv-cli-only.zip` | 接收助手纯 CLI 自包含（约 6MB，解压即用），OBEX 引导首选 |
| `btrecv-selfcontained.zip` / `btrecv-frameworkdependent.zip` | 接收助手完整 GUI（含运行时 / 不含运行时） |
| `BluetoothTransfer-selfcontained.zip` / `BluetoothTransfer-frameworkdependent.zip` | 完整发送端 GUI（内置接收助手，含运行时 / 不含运行时） |

本地构建：`tools/publish-receiver.ps1 -Zip`（btrecv 多形态）、`tools/publish-gui.ps1 -Zip`（完整 GUI 两形态）。

## 🧱 技术栈与架构

| 层 | 技术 |
|---|---|
| 语言 / 运行时 | C# + .NET 8 |
| GUI | WPF（MVVM 组合式分层），SVG 矢量图标（`AppIcons.xaml` 同源，`tools/IconGenerator` 生成 PNG/ICO） |
| 蓝牙 | WinRT `Windows.Devices.Bluetooth.Rfcomm`（OPP 客户端 / 助手服务端） |
| 协议 | 自研 OBEX 客户端（CONNECT/PUT/DISCONNECT，两阶段 PUT，UTF-16BE 无 BOM Name 头）；私有分片续传协议（`BluetoothTransfer.Core`，HELLO/OFFER/DATA/ACK/DONE，SHA-256 校验） |
| 存储 | Microsoft.Data.Sqlite + System.Text.Json |
| 系统托盘 | Hardcodet.NotifyIcon.Wpf |

**解决方案（6 个项目）**：`BluetoothTransfer`（WPF GUI）· `BluetoothTransfer.AppServices`（服务层/模型/接口，手工依赖注入）· `BluetoothTransfer.Core`（助手协议核心）· `BluetoothTransfer.Cli`（btcli）· `BluetoothTransfer.Receiver`（btrecv）· `BluetoothTransfer.Tests`（xUnit）

## 🧪 开发与测试

```powershell
dotnet build BluetoothTransfer.sln -c Release
dotnet test BluetoothTransfer.sln -c Release
btcli selftest
```

- **xUnit 单元测试（184 个）**：OBEX 编解码/流程、重试策略、速率跟踪（受控时钟）、发送队列（含暂停/继续竞态回归）、zip 打包、存储/导出/清空/迁移、设备收藏/别名、配置迁移、助手协议帧/消息/内存传输/断线续传/客户端/服务端/接收端集成、CSV 注入防护、控制器并发守卫（手工依赖注入 + fake）
- 构建基线：**0 警告 / 0 错误**

## 📄 许可

[MIT](LICENSE) © SeverusZh
