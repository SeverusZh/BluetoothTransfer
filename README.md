# BluetoothTransfer

轻量级 Windows 桌面应用，通过蓝牙在设备间快速传输文本与文件，并配套完整的传输记录管理。

## 功能特性

### 核心传输
- **文本传输** — 发送文本框内容到对端，接收端一键复制到剪贴板
- **文件传输** — 单文件 / 多文件批量 / 文件夹递归传输
- **拖拽发送** — 拖拽文件或文件夹到窗口即可发送
- **双通道传输** — BLE（控制/文本/小文件）+ 经典蓝牙 RFCOMM（大文件）

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

当前为可行性阶段：核心收发链路（BLE 文本 / 小文件、RFCOMM 大文件、端到端加密、压缩、断点续传、记录入库与导出）已实现并通过编译验证。仓库暂未配备自动化单元测试，蓝牙相关功能建议在具备蓝牙硬件的 Windows 11 机器上按「快速开始」流程做端到端验证。

## 许可

[MIT](LICENSE)
