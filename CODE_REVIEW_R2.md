# 代码审查报告（第二轮） — BluetoothTransfer

> 审查范围：`main` 分支全量源码，重点为 `547262b...HEAD`（提交 `6d1f79f feat:本地化和删除光暗主题功能`）。
> 审查日期：2026-07-30
> 基线：第一轮审查（CODE_REVIEW.md，HEAD = 67e6c79）→ 修复（547262b，19 项全部完成）。
> 方法：Standards / Spec 双轴并行审查。

---

## Standards（代码标准 / 气味基线）

本仓库无文档化编码标准，适用 Fowler 气味基线（判断性）。

### 中

| # | 类型 | 位置 | 说明 |
|---|---|---|---|
| S-M1 | 重复代码 | `RfcommChannel.cs:260-317` 与 `FileTransferService.cs:118-158` | 文件落盘逻辑（创建目录→清理文件名→去重→校验→写库→发事件）两处重复；RFCOMM 路径硬编码 `recvDir` 未复用 `AppConfig.RecvDirectory`。建议提取共用方法。 |
| S-M2 | 本地化遗漏（硬性） | `MainViewModel.cs:112, 127` | `StatusText = $"Connected: {e.Name}"` / `"Disconnected"` 仍为英文，与本次本地化提交不一致。 |
| S-M3 | 重复代码 | `MainViewModel.cs:297-321` | `ExportCsv` 与 `ExportJson` 仅 Filter/扩展名/调用不同，其余完全一致。建议提取通用导出方法。 |
| S-M4 | 重复代码 | `MainViewModel.cs:137-148` 与 `232-242` | 收发文本各构造 `TransferRecord`，字段赋值形状几乎相同。建议提取工厂方法。 |

### 低

| # | 类型 | 位置 | 说明 |
|---|---|---|---|
| S-L1 | 基本类型偏执 | `TransferRecord.cs:8-16` 及多处 | `"send"/"recv"`、`"text"/"file"`、`"ok"/"failed"`、`"ble"/"rfcomm"` 裸字符串散布，无编译期保护。建议改为枚举或常量。 |
| S-L2 | 投机泛化 | `AppConfig.cs:17` | `BleMtu` 属性已定义但无读取方；`BleService` 硬编码 `const int mtu = 180`。建议删除或接入。 |
| S-L3 | 无用 using | `App.xaml.cs:1-2` | `using System.Configuration; using System.Data;` 无引用。建议删除。 |

---

## Spec（规范符合性）

规范来源：README.md（功能特性）、CODE_REVIEW.md（第一轮问题）、FIX_PROGRESS.md（修复台账）。

### 高

| # | 位置 | 规范行 | 说明 |
|---|---|---|---|
| P-H1 | README.md:44 | "暗色 / 浅色主题切换" | 功能已删除，README 仍列出，误导用户。 |
| P-H2 | README.md:133, 136 | 项目结构树 | `ThemeService.cs` 和 `Themes/` 仍列于结构图，实际文件已不存在。 |

### 中

| # | 位置 | 说明 |
|---|---|---|
| P-M1 | `MainViewModel.cs:112, 127` | 本地化不完整：状态栏 "Connected:" / "Disconnected" 残留英文。 |
| P-M2 | `RfcommChannel.cs:305,316`；`BleGattServer.cs:156`；`FileTransferService.cs:73,80,153` | 写入 SQLite 并在 DataGrid 展示的字段（`PeerName="Remote Device"`、`Note="Checksum mismatch"` 等）仍为英文，用户可见。 |

### 低

| # | 位置 | 说明 |
|---|---|---|
| P-L1 | 所有 `Services/*.cs` 中 `LogEvent` 消息（约 40+ 处） | 日志面板面向用户，消息全英文。若"本地化"仅指 UI 控件文案则可接受，但与提交描述有落差。严格度低（日志保留英文便于排障）。 |

### 首轮修复抽查（H1–H4）

| 编号 | 验证结果 |
|---|---|
| H1 RFCOMM 启动 | ✅ `MainViewModel.cs:192,219` 调用 `ConnectToServerAsync`/`StartServerAsync`；`SendFileAsync` 返回 `Task<bool>` |
| H2 BLE 分片重组 | ✅ `FrameReassembler.cs` 存在；`BleService`/`BleGattServer` 接入；`FileTransferService` 订阅 `FileDataReceivedEvent` |
| H3 文件接收入库 | ✅ `RfcommChannel.cs:301` 调用 `AddRecord`；`MainViewModel.cs:158` 订阅 `FileReceivedEvent` |
| H4 端到端加密 | ✅ `CryptoService` 注入全链路；`KEY_EXCHANGE` 协商；`FrameFlags.Encrypted` 处理 |

---

## 总结

| 轴 | 发现数 | 最严重问题 |
|---|---|---|
| Standards | 7（中4 + 低3） | S-M1：文件落盘逻辑重复且 recvDir 硬编码不一致 |
| Spec | 7（高2 + 中2 + 低1 + 验证4✅） | P-H1/P-H2：README 仍宣称已删除的主题功能 |
