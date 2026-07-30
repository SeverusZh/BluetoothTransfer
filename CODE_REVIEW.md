# 代码审查报告 — BluetoothTransfer

> 审查范围：`main` 分支当前提交（HEAD = 67e6c79），全量源码审查（无未提交改动）。
> 审查日期：2026-07-30
> 说明：本文件仅记录问题，不包含修复补丁。

## 概览

仓库整体骨架清晰（MVVM + 服务层 + 协议分帧），单元划分合理，SQLite 全量参数化、`orderBy` 白名单、CSV 转义等基础功扎实。但**收发链路存在多处「定义了但未接通」的断点**，导致 README 所宣称的核心能力（文件传输、断点续传、端到端加密、接收记录）在当前代码中并未真正生效。建议按 H 级缺陷优先闭环。

| 等级 | 数量 | 说明 |
|---|---|---|
| 高（H） | 4 | 核心功能不可用或与文档严重不符 |
| 中（M） | 9 | 正确性、资源泄漏、性能 |
| 低（L） | 6 | 代码质量、边界 |

---

## 高危（核心功能不可用 / 与文档严重不符）

### H1. RFCOMM 通道从未启动，大文件传输必然失败却被记录为成功
- `RfcommChannel.StartServerAsync` / `ConnectToServerAsync` 在整个 `src` 中**仅定义、从未被调用**（`Services/RfcommChannel.cs:25,43`）。`_clientSocket` 恒为 `null`。
- `FileTransferService.SendFileAsync` 对 `> 10KB` 的文件走 RFCOMM 路径（`Services/FileTransferService.cs:35,41`）。
- `RfcommChannel.SendFileAsync` 在 `_clientSocket == null` 时仅打印日志并 `return`（`Services/RfcommChannel.cs:235-239`），返回 `Task`（无成功/失败信号）。
- 随后 `FileTransferService` 不论是否真正发出，一律以 `Status="ok"` 写库并返回 `true`（`Services/FileTransferService.cs:61-75`）。
- **后果**：任何 >10KB 的文件「发送」既未发出，UI 与 SQLite 却显示成功。核心功能与状态一致性双重缺陷。

### H2. BLE 文件接收路径未实现，二进制被当作 UTF-8 文本，且无分片重组
- 发送端用 `MsgType.DATA` 发送文件字节（`Services/FileTransferService.cs:57-58`）。
- 接收端 `BleGattServer.HandleReceivedData`（`Services/BleGattServer.cs:140-144`）与 `BleService.OnRxValueChanged`（`Services/BleService.cs:190-193`）对 `DATA` 一律 `Encoding.UTF8.GetString(frame.Payload)` 并发布 `TextReceivedEvent`。
- **后果**：(1) 二进制载荷强制按 UTF-8 解码产生乱码；(2) 多分片文件无重组逻辑（重组仅在 `RfcommChannel` 实现，BLE 路径缺失），每片独立覆盖 `ReceivedText`。
- `FileTransferService.ReceiveFile`（`Services/FileTransferService.cs:96`，会落盘并写库）从未被任何接收路径调用——属于死代码。

### H3. 文件接收未写入数据库，`FileReceivedEvent` 无人订阅
- `RfcommChannel.HandleEnd` 完成落盘后只 `Publish(FileReceivedEvent)` 与 `LogEvent`，**未调用 `_storage.AddRecord`**（`Services/RfcommChannel.cs:224-225`）。
- `FileReceivedEvent` 在 `MainViewModel.SubscribeEvents` 中并未订阅（`ViewModels/MainViewModel.cs:103-167` 仅订阅 Device/Text/Log）。
- **后果**：经 RFCOMM 接收的文件不入库、不出现在「Transfer Records」列表、UI 不感知。与文本接收路径（`MainViewModel.cs:147` 有 `AddRecord`）行为不一致。

### H4. 端到端加密未实现（与 README 严重不符）
- `CryptoService`（`Services/CryptoService.cs`）定义了 ECDH P-256 / AES-GCM，但在整个 `src` 中**从未被实例化或调用**（仅类内自引用）。
- `AppConfig.EncryptionEnabled` 默认 `true`（`Models/AppConfig.cs:16`），但 `FileTransferService` 与 `RfcommChannel` 完全忽略该配置；`FrameFlags.Encrypted` 从未被设置或处理（`Services/Protocol.cs:21`）。
- README 称「端到端加密（ECDH P-256 密钥协商 + AES-GCM 加密）」「支持加密（AES-GCM）」，实际未接入任何收发路径。

---

## 中危（正确性 / 资源 / 性能）

### M1. BLE 发送返回值被忽略，失败仍记为成功
- `FileTransferService.SendFileAsync` 对 ≤10KB 的 BLE 路径：`await _ble.SendMetaAsync(meta)` 与 `await _ble.SendBinaryChunkedAsync(...)` 均返回 `Task<bool>`，但返回值被丢弃（`Services/FileTransferService.cs:57-58`），随后一律 `Status="ok"`（`Services/FileTransferService.cs:61-75`）。
- 与 H1 同源：`_txChar==null` 或 `WriteValueAsync` 非 Success 时仍记成功。

### M2. `BleService.ConnectAsync` 在 TX 特征查找失败时泄漏 GATT 资源
- `_service = services.Services[0]`（`Services/BleService.cs:111`）在 TX 检查**之前**赋值。
- TX 特征缺失分支（`Services/BleService.cs:116-120`）直接 `return false`，既未 `bleDevice.Dispose()` 也未 `_service.Dispose()`。
- 对照同方法中 service 缺失分支（`Services/BleService.cs:107`）有 `bleDevice.Dispose()`，处理不一致。失败后 `_service` 字段残留非空，后续状态不一致。

### M3. 意外断连未清理 BLE 服务内部状态
- `OnConnectionStatusChanged`（`Services/BleService.cs:173-180`）在 `Disconnected` 时只发布事件，不释放 `_service`/`_connectedDevice`/`_rxChar`，也不取消 `ValueChanged` 订阅。
- **后果**：掉线后 `_ble.IsConnected`（基于 `_connectedDevice != null`）仍为 `true`；GATT 资源与事件订阅泄漏。`Disconnect()` 是唯一清理入口，但掉线不会调用它。

### M4. 断点续传未闭环，偏移量协商缺失
- 接收端 `HandleMeta`（`Services/RfcommChannel.cs:137-166`）据本地 `TransferState.ReceivedBytes` 计算 `resumeOffset` 并 `Publish(ResumeOffsetEvent)`，但该事件**无人订阅**，也未回传发送端。
- 发送端 `FileTransferService.SendFileAsync` 调用 `_rfcomm.SendFileAsync(...)` 时 `startOffset` 恒为默认 `0`（`Services/FileTransferService.cs:41`）。
- **后果**：若双方已记录偏移不一致，续传会写出错位数据；当前因 `startOffset` 恒为 0，续传实际从不触发——README 宣称的「断点续传」未真正生效。

### M5. `HandleData` 的 `ReceivedBytes` 按当前分片覆盖，未取上界
- `state.ReceivedBytes = frame.Offset + frame.Payload.Length`（`Services/RfcommChannel.cs:182`）。出现重复/乱序分片时（`Offset` 字段的存在暗示允许）该值会回退，导致进度倒退且续传偏移错误。应取 `Math.Max(state.ReceivedBytes, offset+len)`。

### M6. 文件被多次整文件读入内存（OOM 风险）
- `RfcommChannel.SendFileAsync`：`payload = await File.ReadAllBytesAsync(filePath)`（`Services/RfcommChannel.cs:246-252`）已整文件入内存，紧接着 `var checksum = ComputeSha256(await File.ReadAllBytesAsync(filePath))`（`Services/RfcommChannel.cs:254`）**第二次**从磁盘整文件读取。
- `FileTransferService.SendFileAsync` 又 `Checksum = ComputeSha256(await File.ReadAllBytesAsync(filePath))`（`Services/FileTransferService.cs:71`）**第三次**读取。
- `HandleEnd` 又 `File.ReadAllBytes(state.PartialPath)`（`Services/RfcommChannel.cs:202`）整文件入内存做校验/落盘。
- 对 RFCOMM 定位的大文件场景：单次发送存在 3 次整文件磁盘读 + 多次整文件常驻内存，易触发 OOM 与性能劣化。应改为流式哈希与流式发送。

### M7. `RfcommChannel.HandleData` 每个分片都新建/关闭 FileStream
- `Services/RfcommChannel.cs:178` 每收到 4KB 即 `new FileStream(...OpenOrCreate...)` + `Seek` + `Write` + `Dispose`。大文件（4KB 分片）等于数万次开闭句柄。应复用一个可写流，结束/出错时统一释放。

### M8. `BleGattServer.StartAsync` 部分失败时不清理已创建的 provider
- TX 特征创建失败（`Services/BleGattServer.cs:49-53`）直接 `return false`，已赋值的 `_serviceProvider` 未 `StopAdvertising` / 清理。后续若再次 `StartAsync` 会重复创建，且 `Stop()` 也无法清理到（`_serviceProvider` 仍指向旧对象）。对照 RX/META 失败仅记录日志继续，错误处理策略不统一。

### M9. CRC16 实现需在协议规约中明确变体
- `Crc16` 用多项式 `0x1021`、初值 `0xFFFF`、输出不反转（`Services/Protocol.cs:118-142`）。序列化/反序列化同一算法，内部自洽。但若与外部/异构对端互通，需在协议文档固定变体（CRC-CCITT/False vs XModem 等），否则跨端校验失败。属规约层面提醒，非代码 bug。

---

## 低危 / 代码质量

### L1. `DeviceInfo` 无 INotifyPropertyChanged，列表项实时更新不生效
- `DeviceDiscoveredEvent` 处理（`ViewModels/MainViewModel.cs:110`）直接改 `existing.Rssi`，但 `DeviceInfo`（`Models/DeviceInfo.cs`）无属性变更通知，`ListView` 绑定该行的 RSSI 不会刷新。

### L2. `RelayCommand` 构造不校验 `execute` 为 null
- `ViewModels/ViewModelBase.cs:28-32` 未对 `execute` 做 null 检查。当前调用均非 null，风险低。

### L3. `MainViewModel.LogText` 为死字段
- 声明并生成属性（`ViewModels/MainViewModel.cs:27,42`），但实际日志走 `LogLines`（`ObservableCollection`），`LogText` 从未被读写。

### L4. `TransferRecord.CreatedAt` 默认值在对象构造时取 `DateTime.Now`
- `Models/TransferRecord.cs:6`。多数收发路径未显式赋值，依赖默认值；若对象构造到入库之间有耗时操作，时间戳偏早。当前影响小。

### L5. `ushort SeqNo` 在超长传输时回绕
- `Services/BleService.cs:239`、`Services/RfcommChannel.cs:290` 的 `seq++` 为 `ushort`，65535 片后回绕到 0。RFCOMM 大文件（4KB 分片）约 256MB 触发；低概率，建议协议侧明确序号语义。

### L6. 同步 SQLite 调用位于 UI 线程
- `MainViewModel` 构造与 `LoadRecords`（每次收发后调用）在 UI 线程同步执行 SQLite 查询（`ViewModels/MainViewModel.cs:100,275-280`）。数据量大时可能卡顿，建议异步化或后台线程。

---

## 复查建议优先级
1. 闭环 H1/H2/H3：打通 RFCOMM 启动、文件接收重组与落库、`FileReceivedEvent` 订阅——这是「文件传输」能否真正跑通的关键。
2. H4/M1：明确加密与发送成功判定的契约（要么实现，要么从 README/配置中下沉标注「未实现」）。
3. M2/M3/M8：统一 GATT/RFCOMM 资源在失败与断连路径上的释放。
4. M6/M7：流式化大文件读写，消除重复整文件读入。
