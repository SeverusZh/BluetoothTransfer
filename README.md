# BluetoothTransfer

轻量级 Windows 桌面应用：**通用蓝牙推送**。发送端（本应用）通过标准蓝牙 OBEX Object Push Profile（OPP）把文件、文本、文件夹推送给任意支持"蓝牙文件接收"的设备（Android 手机/平板、Windows 电脑、功能机等），**接收端无需安装或运行任何软件**。

> v1.2 起聚焦"通用推送"单一模式。1.0 的双端互联（BLE/RFCOMM 互联收发、端到端加密、断点续传）已移除——该方向在 Windows 上缺乏实际意义，且 OPP 的固有限制（明文、无续传）由自动重试等工程手段缓解，见下文「协议限制」与「未来开发点」。

## 功能特性

### 通用推送（OPP）
- **发送形态** — 单文件直接推送；文本自动包装为 `.txt`（默认 `bt-note.txt`）；文件夹自动压缩为 `.zip` 后推送；单文件可选 `--zip` 打包；剪贴板文本/图片直接推送；文件/文件夹拖入窗口即推送
- **设备发现与配对** — 扫描支持 OPP 的经典蓝牙设备（可只显示已配对），应用内发起系统配对（ConfirmOnly / PIN），失败可引导打开 Windows 蓝牙设置
- **自动重试** — 可重试的临时失败（连接失败 / 超时 / 对端忙 0xC3 / 断开）按指数退避自动重试（默认 3 次，3/6/12s，可配置），最终失败记录含"已重试 N 次"与失败阶段
- **发送队列** — 多文件/文件夹统一排队顺序推送（Android OPP 同时只接受一个传输），支持取消全部、失败任务单点重发
- **传输体验** — 实时进度条 + 速率（KB/s）与剩余时间（ETA）；系统托盘常驻

### 接收助手（btrecv，v1.3）
- **断点续传** — 两台 Windows 电脑之间走私有分片协议（自定义 RFCOMM UUID）：中断后从已确认偏移继续，接收端保留半成品（`.btpart` + `.btpart.meta`），重连自动续传，完成时校验 SHA-256，不匹配自动清空重传
- **真正的传输队列** — 队列任务支持单独**暂停 / 继续 / 移除 / 重试**；接收端忙时自动按指数退避等待；发送端显示通道列（OPP / 助手）
- **自动探测 + 手动覆盖** — 发送前 SDP 探测对端是否运行助手：在线走私有通道，离线自动回退 OPP 通用推送；可在 GUI/CLI 强制指定通道
- **极简接收端** — `btrecv` 单 exe：CLI 内核（`btrecv run [--dir 目录] [--ask]`）+ 极简 GUI 壳（状态/进度/完成列表/保存目录/每次询问开关），默认自动接收；引导时先用 OPP 把 `btrecv` 包推过去，解压即用（自包含无需安装 .NET）
- **明文说明** — 私有通道 v1 为明文，无应用层加密；需要链路加密时可用 Windows 侧配置

### 设备管理
- OPP 设备列表展示（名称/地址/配对状态），支持**收藏置顶**、**别名显示**、按最近连接排序
- 已连接设备自动记录最近连接时间（SQLite）

### 记录与统计
- 所有推送自动写入 SQLite（时间/方向/类型/对端/名称/大小/状态/通道/SHA-256/失败原因）
- 记录列表（GUI）/ `btcli records`（CLI）查询与筛选
- 导出 CSV / JSON，统计汇总，**清空记录**（GUI 按钮带确认，或 `btcli records clear [--yes]`）
- 1.0/1.1 旧记录（`channel=ble/rfcomm`、`direction=recv`）只读兼容，可查询/导出，不会被新版本覆盖

## 快速开始

### GUI
1. 启动应用，点击「扫描」发现设备（需先在本机 Windows 设置中与对端配对）
2. 选中目标设备（可点击星标收藏）
3. 发送文本 / 选择文件 / 选择文件夹 / 剪贴板，或直接把文件拖入窗口
4. 接收端（如 Android 手机）保持屏幕点亮，在通知栏点「接受文件」

### CLI（`btcli`）
```powershell
btcli opp-scan --seconds 5
btcli opp-pair 00:11:22:33:44:55
btcli opp-send-file 00:11:22:33:44:55 C:\tmp\photo.jpg
btcli opp-send-files 00:11:22:33:44:55 a.pdf b.pdf --zip
btcli opp-send-text 00:11:22:33:44:55 "你好，这是蓝牙推送的文本" --name note.txt
btcli opp-send-folder 00:11:22:33:44:55 C:\data\docs
```

### 接收助手引导（Windows 对 Windows）
1. 发送端先通过 OPP 把 `btrecv-selfcontained.zip` 推给目标电脑（GUI「发送文件」或 `btcli opp-send-file <地址> <包路径>`）
2. 对端解压、双击运行 `btrecv`（进入 GUI「监听中」，无需安装 .NET）
3. 发送端重新扫描，设备出现绿色「助手」徽标即代表探测成功；或运行 `btcli detect <地址>`
4. 之后默认自动走可续传的私有通道；对端未运行时自动回退 OPP

## CLI 命令参考

| 分类 | 命令 | 说明 |
|---|---|---|
| 设备 | `btcli opp-scan [--seconds 秒] [--paired-only] [--json]` | 扫描支持 OPP 的经典蓝牙设备 |
| 设备 | `btcli opp-pair <地址> [--pin 1234]` | 发起配对（无 PIN 走系统确认流程） |
| 发送 | `btcli opp-send-file <地址> <文件> [--zip]` | 推送文件（`--zip` 先打包再推送） |
| 发送 | `btcli opp-send-files <地址> <文件1> [文件2 ...] [--zip]` | 批量推送并汇总成功/失败 |
| 发送 | `btcli opp-send-text <地址> <文本> [--name 文件名]` | 文本包装为 .txt 后推送 |
| 发送 | `btcli opp-send-folder <地址> <文件夹>` | 文件夹压缩为 .zip 后推送 |
| 发送 | `btcli opp-send-file|files|text|folder ... [--mode auto\|assistant\|opp]` | 选择通道（默认 opp；auto 探测助手、assistant 强制助手、opp 强制 OPP） |
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

## 配置项

| 键 | 默认 | 说明 |
|---|---|---|
| `OppChunkSize` | 32768 | OPP 分片大小（字节） |
| `OppConnectTimeout` | 30 | 连接超时（秒） |
| `OppSendTimeout` | 300 | 发送超时（秒） |
| `PushTextFileName` | bt-note.txt | 文本推送默认文件名 |
| `OppAuthPassword` | （空） | 设备要求 OBEX 认证时的密码 |
| `OppNameUseBom` | false | Name 头是否带 UTF-16 BOM（默认无 BOM，兼容 Android/Windows） |
| `OppProtectionLevel` | auto | 连接保护级别：auto / plain / encrypt |
| `OppRetryCount` | 3 | 失败自动重试次数（0 表示不重试） |
| `OppRetryDelaySeconds` | 3 | 重试基础间隔（秒），按 1x/2x/3x 退避 |
| `TransferMode` | auto | 发送通道：auto（探测助手，否则 OPP）/ assistant / opp |

## Windows 兼容性说明

- Windows 作为发送端：实现路径与微软官方文档 *RFCOMM Scenario: Send File as a Client* 一致（`RfcommServiceId.ObexObjectPush` + `StreamSocket`），并与 32feet.NET 的 `ObexWebRequest` 逐字节对齐，Windows 10 2004+ / Windows 11 可行，内置/外置 USB 蓝牙适配器均可。
- Windows 作为接收端：由系统"蓝牙文件传输向导"（fsquirt）承载；若 Win11 找不到入口，运行 `fsquirt` 或检查注册表 `DisableFsquirt`。
- 出站 RFCOMM 仅支持**已配对**设备（Windows 蓝牙栈限制）；连接默认明文（PlainSocket，与 Android 真机验证一致），接收端要求加密时可 `config set OppProtectionLevel encrypt`。
- 连接前进行 SDP 预检：确认服务声明 OPP（0x1105），不满足时给出明确错误并写入失败记录。

## 协议限制（需知悉）

- **iPhone（iOS）不支持蓝牙文件接收协议，无法通过 OPP 推送**
- OPP 为明文传输：无端到端加密、无接收端校验回传；需要链路层加密时使用 `OppProtectionLevel=encrypt`
- **OPP 通道无断点续传**：OBEX OPP 无续传语义，接收端失败即丢弃，由自动重试缓解；Windows 对 Windows 走接收助手（btrecv）私有通道时支持断点续传与 SHA-256 校验
- 单文件上限 4GB（OBEX 协议约束），建议不超过 2GB
- Android 同时只接受一个 OPP 传输：对端已有待确认传输时新推送会被拒绝（0xC3 Forbidden），应用会自动重试

## 未来开发点（暂不实现）

- **极简接收助手模式**：已在 v1.3 实现（见「接收助手（btrecv）」）。后续可能的增强：队列跨重启持久化、应用层加密、接收端 Linux/macOS 支持等。

## 技术栈

| 层 | 技术 |
|---|---|
| 语言 / 运行时 | C# + .NET 8 |
| GUI | WPF（MVVM，单页），SVG 矢量图标（`Assets/app.svg` 与 `AppIcons.xaml` 同源，`tools/IconGenerator` 生成 PNG/ICO） |
| 蓝牙 | WinRT `Windows.Devices.Bluetooth.Rfcomm`（OPP 客户端 / 助手服务端） |
| 协议 | 自研 OBEX 客户端（CONNECT/PUT/DISCONNECT，两阶段 PUT，UTF-16BE 无 BOM Name 头）；私有分片续传协议（`BluetoothTransfer.Core`，HELLO/OFFER/DATA/ACK/DONE，SHA-256 校验） |
| 存储 | Microsoft.Data.Sqlite + System.Text.Json |
| 系统托盘 | Hardcodet.NotifyIcon.Wpf |
| 接收端 | `btrecv` 单 exe（CLI 内核 + 极简 WPF 壳），自包含/框架依赖双发布形态 |

## 开发与测试

```powershell
dotnet build BluetoothTransfer.sln -c Release
dotnet test BluetoothTransfer.sln -c Release
btcli selftest
```

- xUnit 单元测试（125 个）：OBEX 编解码/流程、重试策略、速率跟踪、发送队列、zip 打包、存储/导出/清空、设备收藏/别名、配置迁移、助手协议帧/消息/内存传输/落盘续传/客户端/服务端/接收端集成
- 真机验收矩阵：内置/外置适配器 × Android/Windows 接收端；单文件（1KB/1MB/100MB）、文本、文件夹、`--zip`；中文/emoji 文件名；手机忙 0xC3 自动重试；批量队列；拖放与剪贴板；取消与超时

## 开发状态

**v1.3**：接收助手（btrecv）与断点续传。新增共享协议库 `BluetoothTransfer.Core`（帧/消息/内存传输/落盘续传/SHA-256 校验/客户端/服务端/RFCOMM）；`btrecv` 接收端（CLI 内核 + 极简 GUI 壳，自包含单 exe）；发送端 SDP 自动探测 + `TransferMode` 手动覆盖，助手离线自动回退 OPP；队列暂停/继续/移除与通道列；GUI 列表悬停滚轮与日志自动跟随；CLI 新增 `detect`、`--mode`、`package-receiver`。

**v1.2.1**：修复 Windows 对端识别。设备扫描不再只依赖 AEP"可发现"枚举，而是对每个已配对设备直接做 SDP 查询，命中 OBEX Object Push（0x1105）即识别——Windows 主机打开"通过蓝牙发送或接收文件 → 接收文件"后即可被扫描到并正常推送（此前已配对但未开启可发现的 Windows 对端扫不到）。

**v1.2**：通用推送单模式。自动重试、发送队列、速率/ETA、设备收藏/别名/最近排序、拖放、剪贴板发送、单文件 zip 打包、配置清理与旧数据兼容；GUI 单页化；CLI 新增 `opp-send-files`、`devices favorite/alias`。1.0 双端互联已移除。

## 许可

[MIT](LICENSE)
