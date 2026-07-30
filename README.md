# BluetoothTransfer

轻量级 Windows 桌面应用，通过蓝牙在设备间快速传输文本与文件，并配套完整的传输记录管理。

## 功能特性

### 核心传输
- **文本传输** — 发送文本框内容到对端，接收端一键复制到剪贴板
- **文件传输** — 单文件 / 多文件批量 / 文件夹递归传输（保留目录结构）
- **拖拽发送** — 拖拽文件或文本到窗口即可加入发送队列
- **双通道传输** — BLE（控制/文本/小文件）+ 经典蓝牙 RFCOMM（大文件加速）

### 传输体验
- 实时进度条、速度、剩余时间、已传大小
- 传输暂停 / 继续 / 取消
- 传输队列管理（顺序发送、优先级、重试）
- 接收文件后「打开所在目录」或「直接打开」
- 接收文本自动复制到剪贴板（可配置开关）

### 设备管理
- 周边蓝牙设备扫描，列表展示（名称/地址/信号强度）
- 设备别名 / 备注管理
- 收藏 / 常用设备置顶与一键连接
- 自动连接上次设备（启动即连）
- 多设备并发连接（同时与多端传输）

### 安全与完整性
- 端到端加密（ECDH P-256 密钥协商 + AES-GCM 加密）
- 文件校验（SHA-256），接收端自动验证完整性
- 断点续传 — 中断后重连自动从偏移量续传
- 可选 GZip/Deflate 压缩后再传输（大文件）

### 记录与统计
- 所有收发行为自动写入 SQLite
- 记录搜索 / 筛选（类型、方向、设备、时间区间）
- 记录排序（时间、大小、状态）
- 导出 CSV / JSON
- 自动清理 / 手动删除
- 传输统计仪表盘（总次数、总流量、按设备分布）

### 体验增强
- 系统托盘常驻 + 最小化到托盘
- 传输完成系统通知（可配声音）
- 窗口置顶 + 全局快捷键唤起
- 对端离线检测与自动重连
- 蓝牙适配器状态检测与开关引导

## 技术栈

| 层 | 技术 |
|---|---|
| 语言 / 运行时 | C# + .NET 8 |
| GUI 框架 | WPF（MVVM 模式） |
| 蓝牙 BLE | WinRT `Windows.Devices.Bluetooth.Advertisement` + `Windows.Devices.Bluetooth.GenericAttributeProfile` |
| 蓝牙经典 (RFCOMM) | WinRT `Windows.Devices.Bluetooth.Rfcomm` |
| 加密 | `System.Security.Cryptography`（ECDH P-256 / AES-GCM / SHA-256） |
| 存储 | Microsoft.Data.Sqlite (SQLite) + System.Text.Json |
| 系统托盘 | Hardcodet.NotifyIcon.Wpf |
| 发布 | 自包含单文件发布（trim），免装运行时 |

## 系统架构

```
┌──────────────────────────────────────────────┐
│                UI 层 (WPF / MVVM)             │
│ 设备面板│发送区│接收区│记录面板│统计│设置│日志 │
└───────────────┬──────────────────────────────┘
                │ 命令 / 数据绑定
┌───────────────▼──────────────────────────────┐
│              应用服务层 (App Core)             │
│ 设备管理│会话状态机│队列调度│安全│事件总线     │
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
- Windows 11（当前仅支持该平台）
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
  -p:PublishSingleFile=true -p:PublishTrimmed=true `
  --self-contained true
```

发布产物位于 `src/BluetoothTransfer/bin/Release/net8.0-windows10.0.19041.0/win-x64/publish/`。

## 项目结构

```
BluetoothTransfer/
├── src/BluetoothTransfer/        # 主应用
│   ├── Models/                   # 数据模型（TransferRecord/DeviceInfo/AppConfig）
│   ├── Services/                 # 核心服务层
│   │   ├── BleService.cs         # BLE 扫描/广播/连接
│   │   ├── BleGattServer.cs      # BLE GATT 服务端
│   │   ├── RfcommChannel.cs      # RFCOMM 流式通道
│   │   ├── Protocol.cs           # 统一分帧协议（CRC/加密/续传）
│   │   ├── FileTransferService.cs# 文件传输调度
│   │   ├── CryptoService.cs      # ECDH + AES-GCM 加解密
│   │   ├── StorageService.cs     # SQLite 持久化
│   │   ├── ExportService.cs      # CSV/JSON 导出
│   │   ├── EventBus.cs           # 事件总线
│   │   └── TransferState.cs      # 传输状态机
│   ├── ViewModels/               # MVVM ViewModel
│   ├── App.xaml / MainWindow.xaml
│   └── BluetoothTransfer.csproj
├── tests/FeasibilityTest/        # BLE + RFCOMM 可行性验证
├── BluetoothTransfer.sln
└── README.md
```

## 通信协议

双通道协同设计：

- **BLE GATT** — 控制通道：设备发现、连接握手、ECDH 密钥协商、传输元数据交换、文本/小文件传输、控制信令（暂停/取消/心跳/续传偏移）
- **RFCOMM** — 数据通道：大文件批量流式传输，较大分块（4KB），ACK 窗口流量控制

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

SQLite 数据库，`transfer_records` 表包含：时间、方向、类型、文件名/摘要、大小、对端设备、状态、SHA-256 校验值、传输通道、本地路径等字段；`devices` 表管理设备别名、收藏、最近连接时间。

## 开发状态

## 许可

[MIT](LICENSE)
