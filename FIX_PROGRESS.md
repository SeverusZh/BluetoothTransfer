# 修复过程文档 — BluetoothTransfer

> 依据：`CODE_REVIEW.md`（第一轮，HEAD = 67e6c79）+ `CODE_REVIEW_R2.md`（第二轮，HEAD = 6d1f79f）
> 修复范围：第一轮 19 项（H1–H4 / M1–M9 / L1–L6）+ 第二轮 12 项（P-H1–P-L1 / S-M1–S-L3）
> 推进顺序：按审查报告「复查建议优先级」H → M → L
> 验证方式：每完成一批执行 `dotnet build`（无蓝牙硬件，功能以编译通过 + 代码自洽性为准）
> 基线：修复前构建 0 警告 0 错误。
> 结果：**Debug / Release 全解决方案构建均 0 警告 0 错误**（含 FeasibilityTest）。

## 决策记录

| 决策点 | 结论 | 说明 |
|---|---|---|
| H4 加密 | **完整实现端到端加密** | 在 BLE 控制通道做 ECDH P-256 密钥协商，对 RFCOMM/BLE 文件载荷做 AES-GCM 加解密并处理 `FrameFlags.Encrypted`（用户选定） |
| 修复范围 | **全部 19 项** | 按 H→M→L 推进（用户选定） |

## 总体设计（传输链路重构）

本次修复的核心是打通「定义了但未接通」的收发链路。为避免碎片化补丁引入不一致，对传输管线做协同重构：

1. **统一重组器 `FrameReassembler`（新增，`Services/FrameReassembler.cs`）**：BLE 收发两端（`BleService` 客户端 / `BleGattServer` 服务端）共用，按 `TaskId` 缓冲 `DATA` 分片，依据 `Offset` 有序写入；遇到 `FinalChunk`/`END` 收尾：
   - 若该 `TaskId` 存在 `type="file"` 的 `META` → 发布 `FileDataReceivedEvent`（二进制，交由 `FileTransferService` 落盘+写库）；
   - 否则 → 按 UTF-8 解码发布 `TextReceivedEvent`（顺带修复多分片文本被逐片覆盖的问题）。
   - 解密在重组阶段按帧 `FrameFlags.Encrypted` 进行。
2. **RFCOMM 通道 `RfcommChannel` 重写**：
   - 服务端（accept）与客户端（connect）均启动后台读循环 `ReadLoopAsync(socket)`，统一 `ProcessFrame(frame, socket)`。
   - 发送改为**流式**：SHA-256 增量哈希（`FileTransferService` 计算一次并传入）→ 发 `META` → 等待对端续传偏移 → 从偏移处分块流式发送，内存占用恒定（≈chunkSize）。压缩时先流式 deflate 到临时文件再流式发送，结束后清理。
   - **续传闭环**：接收端 `HandleMeta` 计算 `resumeOffset` 后经 socket 回送 `ACK`（`Offset=resumeOffset`）；发送端读循环收到 `ACK` 完成 `TaskCompletionSource`，`SendFileAsync` 据此从偏移续传（3s 超时降级为从头）。
   - 接收端复用一个可写 `FileStream`（`_openStreams`），`END`/异常/断连时统一释放；`ReceivedBytes` 取 `Math.Max` 上界。
   - `SendFileAsync` 返回 `Task<bool>`，真实反映发送成败。
   - `HandleEnd` 流式解压+落盘+增量校验后调用 `_storage.AddRecord` 并发布 `FileReceivedEvent`（H3）。
3. **加密 `CryptoService` 接入**：由 `MainViewModel` 创建单例，注入 `BleService`/`BleGattServer`/`RfcommChannel`/`FileTransferService`/`FrameReassembler`。
   - 密钥协商：BLE 连接成功后客户端发起 `KEY_EXCHANGE`（新增 `MsgType=0x09`），服务端派生会话密钥并经 RX notify 回送公钥，客户端派生会话密钥。
   - 文件 `DATA` 载荷按帧 AES-GCM 加密（每帧独立 nonce），置 `FrameFlags.Encrypted`；`META` 保持明文以便接收端建状态。校验和始终针对**未压缩、未加密的原始文件**。BLE 加密分片将明文分片缩小 28 字节（nonce12+tag16）以适配 MTU。
   - 优雅降级：`EncryptionEnabled` 但尚无会话密钥时发明文并告警，不阻断传输。
4. **资源释放统一**（M2/M3/M8）：连接失败、意外断连、GATT 服务端部分失败路径均释放已创建资源并复位内部状态。

### 已知简化 / 限制（ feasibility 阶段可接受）

- `CryptoService` 为进程级单例：同一进程同时作为 BLE 服务端与客户端跟不同对端协商时，后发起的密钥协商会覆盖前一个会话密钥。单对端典型用法不受影响。
- `RfcommChannel.Close()` 同时关闭客户端与服务端；「连接」与「广播」并用的边缘场景下会一并关闭。典型「要么广播接收、要么连接发送」用法不受影响。
- 加密范围按审查报告 H4 措辞限定为**文件载荷**；文本消息保持明文。

---

## 修复台账

状态图例：⬜ 待办 / 🔄 进行中 / ✅ 完成 / ⏸ 暂缓

### 高危（H）

| 编号 | 问题 | 状态 | 修复要点 / 涉及文件 | 验证 |
|---|---|---|---|---|
| H1 | RFCOMM 从未启动，大文件发送失败却记成功 | ✅ | `RfcommChannel.SendFileAsync` 返回 `Task<bool>`；`MainViewModel.ConnectAsync` 调 `ConnectToServerAsync`、`ToggleAdvertiseAsync` 调 `StartServerAsync`；`FileTransferService` 据返回值写 `ok/failed` | 构建通过 |
| H2 | BLE 接收把二进制当 UTF-8，无分片重组 | ✅ | 新增 `FrameReassembler`；`BleService.OnRxValueChanged`/`BleGattServer.HandleReceivedData` 接入；`FileDataReceivedEvent` → `FileTransferService.ReceiveFile`；`SendMetaAsync` 携带 taskId 使 META/DATA 关联 | 构建通过 |
| H3 | 文件接收未写库，`FileReceivedEvent` 无人订阅 | ✅ | `RfcommChannel.HandleEnd` 注入 `StorageService` 并 `AddRecord`；`MainViewModel.SubscribeEvents` 订阅 `FileReceivedEvent` 刷新 UI | 构建通过 |
| H4 | 端到端加密未实现 | ✅ | `KEY_EXCHANGE` 协商（`BleService` 客户端发起 / `BleGattServer` 服务端响应）+ 文件载荷 AES-GCM（`RfcommChannel`/`SendBinaryChunkedAsync`/`FrameReassembler`）+ `FrameFlags.Encrypted` | 构建通过 |

### 中危（M）

| 编号 | 问题 | 状态 | 修复要点 / 涉及文件 | 验证 |
|---|---|---|---|---|
| M1 | BLE 发送返回值被忽略，失败仍记成功 | ✅ | `FileTransferService` 检查 `SendMetaAsync` && `SendBinaryChunkedAsync` 返回，决定 `ok` | 构建通过 |
| M2 | `ConnectAsync` TX 查找失败泄漏 GATT 资源 | ✅ | TX 失败分支 `Dispose` service+device 并复位 `_service`；catch 中未连接时释放 | `BleService.cs` 构建通过 |
| M3 | 意外断连未清理 BLE 内部状态 | ✅ | 抽出 `CleanupConnection()`；`OnConnectionStatusChanged` 断连时释放资源、取消 `ValueChanged`/`ConnectionStatusChanged` 订阅、复位状态 | 构建通过 |
| M4 | 断点续传未闭环，偏移协商缺失 | ✅ | 接收端 `HandleMeta` 回送 `ACK(Offset)`；发送端 `_pendingResume` + `TaskCompletionSource` 等待并据此续传 | 构建通过 |
| M5 | `ReceivedBytes` 按当前分片覆盖未取上界 | ✅ | `state.ReceivedBytes = Math.Max(state.ReceivedBytes, offset+len)` | 构建通过 |
| M6 | 文件多次整文件读入内存（OOM） | ✅ | 流式 SHA-256（`ComputeSha256Async`，仅一次并由 `FileTransferService` 传入）+ 流式分块发送 + 接收端流式解压/落盘/增量校验；消除 3 次 `ReadAllBytes` | 构建通过 |
| M7 | `HandleData` 每分片新建/关闭 FileStream | ✅ | `_openStreams` 复用可写流，`HandleEnd`/断连 `CloseOpenStreams` 统一释放 | 构建通过 |
| M8 | `BleGattServer.StartAsync` 部分失败不清理 | ✅ | TX 失败分支 `StopAdvertising`+复位 `_serviceProvider`；catch 统一清理 | 构建通过 |
| M9 | CRC16 变体需在协议规约明确 | ✅ | `Crc16` 类注释固定为 CRC-16/CCITT-FALSE；README 协议约定补充 | 构建通过 |

### 低危（L）

| 编号 | 问题 | 状态 | 修复要点 / 涉及文件 | 验证 |
|---|---|---|---|---|
| L1 | `DeviceInfo` 无 INotifyPropertyChanged | ✅ | `DeviceInfo : INotifyPropertyChanged` + `SetProperty`，RSSI/连接态变更可刷新 | 构建通过 |
| L2 | `RelayCommand` 不校验 execute 为 null | ✅ | 构造 `?? throw new ArgumentNullException(nameof(execute))` | 构建通过 |
| L3 | `MainViewModel.LogText` 死字段 | ✅ | 移除 `_logText` 字段与 `LogText` 属性（已确认无 XAML 绑定） | 构建通过 |
| L4 | `TransferRecord.CreatedAt` 构造时取时间 | ✅ | 默认空串，`StorageService.AddRecord` 入库时取 `DateTime.Now` | 构建通过 |
| L5 | `ushort SeqNo` 超长传输回绕 | ✅ | `Frame.SeqNo` 注释明确为滚动计数、排序以 Offset 为准；README 补充 | 构建通过 |
| L6 | 同步 SQLite 位于 UI 线程 | ✅ | `LoadRecordsAsync` 经 `Task.Run` 后台查询、UI 线程更新集合；全部调用点改造 | 构建通过 |

---

## 变更日志

### 2026-07-31 — 广播降级修复（GUI + CLI）

> 触发：本机（Intel Wireless Bluetooth，`USB\VID_8087&PID_0026`）实测发现
> `BluetoothLEAdvertisementPublisher.Start()` 抛 `E_INVALIDARG`，导致启动广播整体失败。
> 经 WinRT 探针确认 GATT 服务端创建与 `StartAdvertising` 均正常，名称广播仅影响扫描列表是否显示本机名称。

#### 修复要点

- `Services/BleGattServer.cs`：移除 `toleratePublisherFailure` 开关（GUI/CLI 行为统一）。
  - 发布器启动异常/状态异常改为 `WARN` 级别降级提示，不再回滚 GATT 服务；
  - 新增 `IsNameAdvertised` 属性供 UI/CLI 展示降级状态；
  - 启动日志区分"广播名称正常"与"仅 GATT 服务广播"两种文案。
- `ViewModels/MainViewModel.cs`：广播启动后状态栏文案同步显示降级提示（"可按地址连接"）。
- `src/BluetoothTransfer.Cli/Commands.cs`：`serve` 横幅提示名称广播降级，便于测试时按地址连接。

#### 验证

- `dotnet build BluetoothTransfer.sln -c Debug/Release` → 0 警告 0 错误。
- 本机实测 `btcli serve --ble-only`：GATT + RFCOMM 正常启动，WARN 提示名称广播不可用后继续广播，超时后干净退出。

### 2026-07-31 — 第三轮审查修复（21 项）

> 依据：`CODE_REVIEW_R3.md`（第三轮审查，基线 HEAD = 7dba4b3）
> 修复范围：高 3 + 中 8 + 低 10
> 验证方式：`dotnet build`（Debug + Release）

#### 修复台账

| 编号 | 问题 | 修复要点 |
|---|---|---|
| R3-H1 | 空文件 BLE 发送静默丢失 | `SendBinaryChunkedAsync` 在 `data.Length==0` 时发送一个带 `FinalChunk` 的空 DATA 帧，接收端正常收尾落盘 |
| R3-H2 | GATT 回调无异常兜底 | `BleService.OnRxValueChanged` 整体 try/catch；`FrameReassembler.HandleData` 解密失败时记录日志并清理任务缓冲，异常不再逃逸 |
| R3-H3 | README 失实 | 功能特性/系统架构/前置条件/通信协议/开发状态全面改写为实际实现；删除暂停/队列/统计/别名/置顶/快捷键等未实现声明 |
| R3-M1 | BLE 帧长与 MTU 不匹配 | 接入 `GattSession.MaxPduSize` 动态计算载荷上限（扣除 3 字节 ATT 头，保守回退 MTU=180） |
| R3-M2 | RSSI 不刷新 | 已发现设备也持续发布 `DeviceDiscoveredEvent`（更新 RSSI/LastSeen） |
| R3-M3 | RFCOMM 帧长无上限 | 新增 512KB 上限，异常长度终止读取循环 |
| R3-M4 | 发送结果被忽略 | 三个发送入口按返回值给出"成功/部分失败/失败"状态 |
| R3-M5 | 同步命令无异常处理 | 扫描/导出/复制均就地捕获并记录日志，不再逃逸崩溃 |
| R3-M6 | 记录列表显示缺陷 | 新增 `CreatedAtDisplay/DirectionDisplay/TypeDisplay/StatusDisplay/ChannelDisplay`，DataGrid 绑定展示属性 |
| R3-M7 | 同名文件覆盖 | `ResolveDestPath` 冲突时追加递增序号 |
| R3-M8 | 续传数据损坏 | `HandleMeta` 校验 partial 文件存在/长度，无效状态删除旧文件并从 0 接收；新传输清理旧 partial 尾巴 |
| R3-L1 | 帧版本不校验 | `Frame.Deserialize` 拒绝 Version != 1 的帧 |
| R3-L2 | 广播状态误报 | `BluetoothLEAdvertisementPublisher.Start()` 后检查状态，非 Started/Waiting 时回滚并返回失败 |
| R3-L3 | RFCOMM 失败状态误导 | `ConnectAsync` 在 RFCOMM 失败时更新状态栏并输出警告 |
| R3-L4 | 广播半启动状态 | GATT 成功但 RFCOMM 失败时调用 `_gattServer.Stop()` 回滚 |
| R3-L5 | 本地化遗漏 | 日志 `(valid=...)` 改中文；BLE 服务端对端地址 `"remote"` 改空串 |
| R3-L6 | 死配置 | 删除 5 个未使用配置项（NotificationSound/AutoConnectLast/AutoCleanDays/AlwaysOnTop/GlobalHotkey） |
| R3-L7 | 死事件 | 删除 `ResumeOffsetEvent` 及发布点 |
| R3-L8 | 裸字符串 | `GetStats` 改用 `TransferConst.DirSend/DirRecv` |
| R3-L9 | 导出编码 | CSV/JSON 导出改为带 BOM 的 UTF-8 |
| R3-L10 | 资源泄漏 | `ConnectToServerAsync` 释放 `RfcommDeviceService`；`Close()` 清空接收/续传字典 |

#### 变更文件

**服务层**
- `Services/BleService.cs`：RSSI 持续更新、GATT 回调兜底、空文件 FinalChunk、`GattSession` 动态 MTU、会话释放。
- `Services/RfcommChannel.cs`：帧长上限、未知消息告警、续传状态校验与旧 partial 清理、4GB 上限前置、`RfcommDeviceService` 释放、`Close()` 清理字典。
- `Services/FrameReassembler.cs`：解密失败兜底与任务缓冲清理。
- `Services/Protocol.cs`：帧版本校验。
- `Services/BleGattServer.cs`：广播发布器状态检查、对端地址空串。
- `Services/FileTransferService.cs`：`ResolveDestPath` 唯一化、`SendFolderAsync` 返回成败、日志中文化。
- `Services/EventBus.cs`：删除 `ResumeOffsetEvent`。
- `Services/StorageService.cs`：`GetStats` 使用常量。
- `Services/ExportService.cs`：带 BOM UTF-8 导出。

**模型 / UI**
- `Models/TransferRecord.cs`：新增 5 个展示属性（时间/方向/类型/状态/通道中文化）。
- `Models/AppConfig.cs`：删除 5 个未使用配置项。
- `ViewModels/MainViewModel.cs`：发送结果状态、RFCOMM 失败提示、广播回滚、扫描/导出/复制异常兜底。
- `MainWindow.xaml`：DataGrid 绑定展示属性。

**文档**
- `README.md`：功能特性、架构图、前置条件、协议说明、开发状态与实际实现对齐。
- `CODE_REVIEW_R3.md`：第三轮审查报告（新增）。

#### 验证

- `dotnet build BluetoothTransfer.sln -c Debug` → 0 警告 0 错误。
- `dotnet build BluetoothTransfer.sln -c Release` → 0 警告 0 错误。
- 说明：本环境无蓝牙硬件且为 WPF GUI，仍以编译通过与收发链路代码自洽性为准。

### 2026-07-30 — 全量修复（19 项）

**新增文件**
- `src/BluetoothTransfer/Services/FrameReassembler.cs`：BLE 分片重组器（H2/H4）。

**协议层（`Services/Protocol.cs`）**
- `MsgType` 新增 `KEY_EXCHANGE = 0x09`（H4）。
- `Frame.SeqNo` 增加滚动计数/回绕语义注释（L5）。
- `Crc16` 增加 CRC-16/CCITT-FALSE 变体注释（M9）。

**RFCOMM 通道（`Services/RfcommChannel.cs`，重写）**
- 构造注入 `StorageService`/`CryptoService`。
- 服务端/客户端统一后台读循环 `ReadLoopAsync` + `ProcessFrame(frame, socket)`（H1）。
- `SendFileAsync` 返回 `Task<bool>`、流式发送、续传闭环（`_pendingResume`）、按帧加密（H1/M4/M6/H4）。
- `HandleMeta` 回送 `ACK(Offset)`（M4）；`HandleData` 流复用 + `Math.Max` + 解密（M5/M7/H4）。
- `HandleEnd` 流式解压/落盘/增量 SHA-256 + `AddRecord` + `FileReceivedEvent`（M6/H3）。
- 新增公共流式哈希 `ComputeSha256Async`；`Close`/断连清理开放流（M7）。

**BLE 客户端（`Services/BleService.cs`）**
- 构造注入 `CryptoService`/`FrameReassembler`；接收改走重组器（H2）。
- 连接成功发起 `KEY_EXCHANGE`，`OnRxValueChanged` 处理对端公钥（H4）。
- TX 查找失败/异常路径释放 GATT 资源（M2）；`CleanupConnection` + 断连状态复位（M3）。
- `SendMetaAsync(meta, taskId)` 关联 META/DATA（H2）；`SendBinaryChunkedAsync(..., encrypt)` 按帧加密并适配 MTU（H4）。

**BLE 服务端（`Services/BleGattServer.cs`）**
- 构造注入 `CryptoService`/`FrameReassembler`；接收改走重组器（H2）。
- `HandleKeyExchange` 服务端密钥协商响应（H4）；TX 创建失败/异常清理 provider（M8）。

**文件传输调度（`Services/FileTransferService.cs`）**
- 构造注入 `CryptoService`，订阅 `FileDataReceivedEvent` 接通原死代码 `ReceiveFile`（H2）。
- 发送校验返回值决定 `ok/failed`（H1/M1）；加密策略 `EncryptionEnabled && HasSessionKey`（H4）。
- 校验和一次性流式计算并传入 RFCOMM，消除重复整文件读取（M6）。
- `ReceiveFile` 扩展 channel/peer 参数。

**事件总线（`Services/EventBus.cs`）**
- 新增 `FileDataReceivedEvent`（H2）。

**视图模型（`ViewModels/MainViewModel.cs`）**
- 创建并注入 `CryptoService`/`FrameReassembler`；`ConnectAsync`/`ToggleAdvertiseAsync` 接通 RFCOMM 启动（H1）。
- 订阅 `FileReceivedEvent`（H3）与 `TransferProgressEvent`（进度条生效）。
- 移除死字段 `LogText`（L3）；`LoadRecordsAsync` 后台查询（L6）。

**模型 / 基础**
- `Models/DeviceInfo.cs`：实现 `INotifyPropertyChanged`（L1）。
- `Models/TransferRecord.cs` + `Services/StorageService.cs`：`CreatedAt` 入库时取时间（L4）。
- `ViewModels/ViewModelBase.cs`：`RelayCommand` 校验 `execute` 非空（L2）。

**文档**
- `README.md`：协议约定补充 CRC 变体、SeqNo 语义、加密握手说明（M9/L5/H4）。

**验证**
- `dotnet build BluetoothTransfer.sln -c Debug` → 0 警告 0 错误。
- `dotnet build BluetoothTransfer.sln -c Release` → 0 警告 0 错误。
- 说明：本环境无蓝牙硬件且为 WPF GUI，无法做实机收发回归；以上以编译通过与收发链路代码自洽性为准，建议具备硬件后按 README 流程做端到端验证。

---

### 2026-07-30 — 第二轮审查修复（12 项）

> 依据：`CODE_REVIEW_R2.md`（第二轮审查，双轴并行）
> 修复范围：Spec 轴 5 项（高2 + 中2 + 低1）+ Standards 轴 7 项（中4 + 低3）
> 验证方式：`dotnet build`（Debug + Release）

#### 修复台账

| 编号 | 轴 | 等级 | 问题 | 状态 | 修复要点 |
|---|---|---|---|---|---|
| P-H1 | Spec | 高 | README 仍宣称已删除的主题功能 | ✅ | 删除 README.md:44 "暗色/浅色主题切换" |
| P-H2 | Spec | 高 | README 项目结构仍列出已删除文件 | ✅ | 删除结构树中 `ThemeService.cs` 和 `Themes/` |
| P-M1 | Spec | 中 | 状态栏残留英文 "Connected:"/"Disconnected" | ✅ | 改为"已连接：…"/"已断开连接" |
| P-M2 | Spec | 中 | 数据库可见字段残留英文 | ✅ | "Remote Device"→"远程设备"、"Checksum mismatch"→"校验和不匹配"、"Transfer reported failure"→"传输报告失败" |
| P-L1 | Spec | 低 | 日志消息全英文 | ✅ | 全部 Services + ViewModel 日志消息本地化（约 50 处） |
| S-M1 | Standards | 中 | 文件落盘逻辑重复 + recvDir 硬编码 | ✅ | 提取 `FileTransferService.ResolveDestPath` 静态方法；`RfcommChannel` 注入 `AppConfig` 复用 `RecvDirectory` |
| S-M2 | Standards | 中 | 同 P-M1 | ✅ | 合并修复 |
| S-M3 | Standards | 中 | ExportCsv/ExportJson 高度雷同 | ✅ | 提取 `ExportRecords(filter, ext, export)` 通用方法 |
| S-M4 | Standards | 中 | 文本传输记录构造重复 | ✅ | 提取 `MakeTextRecord(direction, peerName, peerAddr, text)` 工厂方法 |
| S-L1 | Standards | 低 | 方向/类型/状态/通道为裸字符串 | ✅ | 新增 `TransferConst` 静态常量类，全量替换 |
| S-L2 | Standards | 低 | `AppConfig.BleMtu` 未被使用 | ✅ | 删除该配置项 |
| S-L3 | Standards | 低 | `App.xaml.cs` 无用 using | ✅ | 删除 `using System.Configuration; using System.Data;` |

#### 变更文件

**文档**
- `README.md`：删除主题功能描述与结构树中已删除文件（P-H1/P-H2）。

**模型（`Models/TransferRecord.cs`）**
- 新增 `TransferConst` 静态类：`DirSend/DirRecv/TypeText/TypeFile/StatusOk/StatusFailed/ChannelBle/ChannelRfcomm`（S-L1）。

**模型（`Models/AppConfig.cs`）**
- 删除未使用的 `BleMtu` 属性（S-L2）。

**RFCOMM 通道（`Services/RfcommChannel.cs`）**
- 构造注入 `AppConfig`；`HandleEnd` 改用 `_config.RecvDirectory` + `FileTransferService.ResolveDestPath`（S-M1）。
- 记录字段使用 `TransferConst` 常量；用户可见字段中文化（P-M2/S-L1）。
- 全部日志消息本地化（P-L1）。

**文件传输调度（`Services/FileTransferService.cs`）**
- 新增 `public static ResolveDestPath(recvDir, fileName)` 共用方法（S-M1）。
- `ReceiveFile` 改用 `ResolveDestPath`；记录字段使用常量；用户可见字段中文化（S-M1/P-M2/S-L1）。
- 发送记录使用常量（S-L1）。
- 日志消息本地化（P-L1）。

**BLE 客户端（`Services/BleService.cs`）**
- 重组器调用使用 `TransferConst.ChannelBle`（S-L1）。
- 全部日志消息本地化（P-L1）。

**BLE 服务端（`Services/BleGattServer.cs`）**
- 重组器调用使用 `TransferConst.ChannelBle`；`PeerName` 改为"远程设备"（S-L1/P-M2）。
- 全部日志消息本地化（P-L1）。

**分片重组器（`Services/FrameReassembler.cs`）**
- 日志消息本地化（P-L1）。

**视图模型（`ViewModels/MainViewModel.cs`）**
- 状态栏 "Connected:"/"Disconnected" 改中文（P-M1）。
- 提取 `MakeTextRecord` 工厂方法消除收发文本记录重复（S-M4）。
- 提取 `ExportRecords` 通用导出方法消除 ExportCsv/ExportJson 重复（S-M3）。
- 文本记录使用 `TransferConst` 常量（S-L1）。
- `RfcommChannel` 构造传入 `_config`（S-M1）。
- SafeAsync 错误日志本地化（P-L1）。

**App（`App.xaml.cs`）**
- 删除无用 `using System.Configuration; using System.Data;`（S-L3）。

#### 验证

- `dotnet build BluetoothTransfer.sln -c Debug` → 0 警告 0 错误。
- `dotnet build BluetoothTransfer.sln -c Release` → 0 警告 0 错误。
