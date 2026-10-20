# 探索报告 01：Windows 资源管理器右键菜单（「用蓝牙发送」）可行性

> 任务：第 1 项探索（Slice D1），只做可行性分析与方案设计，**不写生产代码、不改注册表**。
> 仓库：`SeverusZh/BluetoothTransfer` @ `2f9dbfb`（2026-10-20，main）。
> 说明：报告以 git HEAD 的代码事实为准；写作期间 Slice C（单实例）正在工作区进行，
> 涉及它的差异在文中单独标注。**代码事实与任务描述/README 不一致处已在文中注明。**
> 检索时间：2026-10-20，引用的微软官方文档链接见文末。

---

## 1. 结论摘要

**结论：可行。** 用 Windows 自带的 Shell 扩展机制把「用蓝牙传输发送」加进文件/文件夹右键菜单，
不需要管理员权限、不需要安装服务，工作量可控（核心约 1.5–2.5 人日 + 0.5–1 人日收尾）。

但必须区分两条产品路径：

| 目标 | 结论 | 说明 |
|---|---|---|
| **Win10（1809+ / 2004+）主右键菜单、Win11「显示更多选项」** | ✅ 可行且推荐 | `HKCU\Software\Classes\...\shell` 静态 Verb（Static Verb），HKCU 免管理员，`%1` 直接传路径 |
| **Win11 新式一级菜单（不点「显示更多选项」）** | ⚠️ 可行但成本高 | 必须实现 COM `IExplorerCommand` + **应用身份**（MSIX 打包或 MSIX Sparse Package）；静态 Verb 不会出现在新菜单 |
| **Win11 新菜单里的「发送到」子菜单** | ✅ 可行（社区实测可用） | 只需在 `%APPDATA%\Microsoft\Windows\SendTo` 放一个快捷方式；「发送到」子菜单在 Win11 新菜单中仍然存在；**多选会把全部路径一次性传给一个实例** |
| **多选后一次性传入一个实例（静态 Verb）** | ⚠️ 受限 | 静态 Verb 的命令行方式本质上是 `CreateProcess`，多选时一般**逐项调用**（每个文件一个进程）；微软文档的 `MultiSelectModel=Player` 对静态 Verb 的实测效果不可靠。可靠做法是 SendTo 或 COM 扩展（`IDropTarget`/`IExplorerCommand`） |

**推荐落地方案（分三阶段）：**

1. **Phase 1（推荐先做，1–2 周内可发布）**：
   - `HKCU\Software\Classes` 静态 Verb 覆盖：`*`（文件）、`Directory`（文件夹）、`Directory\Background`（文件夹空白处）、`Drive`（驱动器），单选即触发；
   - 同时提供 `%APPDATA%\Microsoft\Windows\SendTo` 快捷方式，承担**多选**入口（Win10/Win11 新菜单都可见，零注册表风险）；
   - GUI 增加 `--send <path...>` / `--send-text <path...>` 命令行入口，并与 Slice C 单实例守卫协同：**二次启动把路径转发给已运行实例**，主实例激活窗口、把待发送项放进队列并提示选择目标设备；
   - 提供应用内「安装/卸载右键菜单」按钮（写 HKCU，无需管理员）或随包 `.reg`。
2. **Phase 2（可选增强）**：静态 Verb + `DropTarget`/`DelegateExecute`（COM `IDropTarget`）实现真正的「多选一次调用」，仍不进入 Win11 新菜单，但可去掉 SendTo 的双入口。
3. **Phase 3（可选，5–10 人日 + 签名/发布运维）**：`IExplorerCommand` + MSIX Sparse Package，进入 Win11 新式一级菜单。收益是 UX，成本主要在原生 COM DLL、打包签名、安装/卸载与商店外分发信任。

**关键约束：**

- 静态 Verb 通过 `CreateProcess` 激活，微软文档明确其**命令行长度约 2000 字符**，多选能力天然受限（[Choosing a Static or Dynamic Shortcut Menu Method](https://learn.microsoft.com/en-us/windows/win32/shell/shortcut-choose-method)）；
- SendTo 通过 Shell 的 Drop 机制激活，多选不受该 2000 字符限制，仅受 Windows 命令行 32767 字符上限约束（实践报告，见 [Stack Overflow #6440715](https://stackoverflow.com/questions/6440715) 的答案）；
- Win11 新菜单只接受带**应用身份**（package identity）的 `IExplorerCommand`，静态 Verb 一律落到「显示更多选项」（Shift+F10 / 菜单键）；这是微软 2021 年设计说明与现行文档的共同结论（[Windows Developer Blog](https://blogs.windows.com/windowsdeveloper/2021/07/19/extending-the-context-menu-and-share-dialog-in-windows-11/)、[Add a File Explorer context menu command to a packaged desktop app](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/integrate-packaged-app-with-file-explorer)）。

---

## 2. 背景与现状代码事实

### 2.1 用户诉求

右键点击文件/文件夹（可多选）→ 菜单出现「用蓝牙传输发送」→ 应用把选中对象推送到某台蓝牙设备。
用户期望的交互重点是「从资源管理器直接进入发送流程」，而不是先打开应用再选文件。

### 2.2 仓库现状（HEAD `2f9dbfb`）

| 现状 | 代码位置 | 对右键菜单的影响 |
|---|---|---|
| GUI 是 `WinExe`（无控制台窗口） | `src/BluetoothTransfer/BluetoothTransfer.csproj`（`<OutputType>WinExe</OutputType>`） | ✅ 适合做 Shell 目标，不会闪黑框 |
| `App.xaml` 使用 `StartupUri="MainWindow.xaml"`，**无任何命令行参数处理** | `src/BluetoothTransfer/App.xaml`、`App.xaml.cs` | ❌ 需要新增 `--send` 入口；`StartupUri` 会先建窗，参数入口需在 `OnStartup` 之前/之中接管 |
| 有独立 CLI `btcli`（`opp-send-file/files/text/folder`，支持 `--mode auto\|assistant\|opp`） | `src/BluetoothTransfer.Cli/` | ⚠️ 命令签名为 `<设备地址> <文件>`，**必须先知道设备地址**；`OutputType=Exe` 会弹控制台；不适合直接作为 Shell 命令 |
| 主窗口已支持**拖放文件/文件夹**发送（`OnDragOver`/`OnFileDrop` → `OppViewModel.SendDroppedAsync`） | `MainWindow.xaml.cs:64-80`、`OppViewModel.cs:677-702` | ✅ 可复用「路径 → 发送队列」的现成逻辑 |
| 发送前必须已选中设备：`SendDroppedAsync` 在 `SelectedDevice == null` 时只提示「请先在设备列表中选中目标设备」 | `OppViewModel.cs:678-683` | ❌ 右键发送没有设备上下文，需要新增「待发送项 + 选择设备」交互 |
| 文件/文件夹分类已存在：`Directory.Exists(p)` → folder（zip），否则 file | `OppViewModel.cs:684-699` | ✅ 复用 |
| 单实例守卫（Slice C，工作区进行中，未提交）：命名 Mutex + 命名事件，**只激活窗口、不携带参数** | `src/BluetoothTransfer.AppServices/SingleInstanceGuard.cs`（工作区新增） | ⚠️ 需要扩展「带载荷转发」（见 §5.2） |
| 应用图标已配置 | `BluetoothTransfer.csproj` `<ApplicationIcon>Assets\app.ico</ApplicationIcon>` | ✅ 静态 Verb 的 `Icon` 可直接指 exe `,0` |
| 目标框架 `net8.0-windows10.0.19041.0`、`SupportedOSPlatformVersion=10.0.19041.0` | 各 csproj | 支持 Windows 10 2004+ / Windows 11 |

> **代码事实修正（与任务描述）**：任务描述称「当前 App 用 StartupUri、无命令行参数处理」——HEAD 确实如此；
> 但**当前工作区**的 Slice C 已移除 `StartupUri`、改为 `OnStartup` 手动建窗并加入单实例守卫。
> 本报告 §5 的改造清单以「HEAD + Slice C 工作区改动」为基线，接口命名沿用 `SingleInstanceGuard`。

### 2.3 Windows Shell 机制要点（官方文档结论）

1. **静态 Verb**：在 `HKEY_CLASSES_ROOT\<ProgID>\shell\<verb>\command` 或 `HKCU\Software\Classes\...` 下
   写命令行模板，Shell 用 `CreateProcess` 激活；`%1` 是选中项占位符，含空格必须加引号。
   （[Extending Shortcut Menus](https://learn.microsoft.com/en-us/windows/win32/shell/context)、[Verbs and File Associations](https://learn.microsoft.com/en-us/windows/win32/shell/fa-verbs)）
2. **HKCU\Software\Classes 免管理员**：文档明确「因为 HKCR 是 HKCU+HKCU 的合并视图，自定义 Verb 可注册在
   `HKEY_CURRENT_USER\Software\Classes` 下，主要优势是不需要提升权限」。
   （[Creating Shortcut Menu Handlers](https://learn.microsoft.com/en-us/windows/win32/shell/context-menu-handlers)）
3. **多选与 `MultiSelectModel`**：`Single` / `Player` / `Document` 三选一；未指定时，COM 类实现默认 Player、
   其它（含静态 Verb）默认 **Document**。Document 表示「每个选中项创建一个顶层窗口」；Player 表示支持任意数量。
   Legacy Verb 的可见上限：Document 15 项、Player 100 项，超限 Verb 直接不显示。
   （[How to Employ the Verb Selection Model](https://learn.microsoft.com/en-us/windows/win32/shell/how-to-employ-the-verb-selection-model)）
4. **静态 Verb 的多选真实行为**：静态 Verb 的命令行激活方式（CreateProcess）在文档中被描述为「把选中文件和参数作为命令行传入」，
   但同时被列为限制项：命令行长度约 2000 字符、无法复用已运行进程。社区实测与问答表明，静态 Verb 在多选时
   **通常仍按项逐次调用**，`MultiSelectModel=Player` 并不能可靠地把全部路径合并进一次调用
   （[SuperUser #365704 的实测讨论](https://superuser.com/questions/365704/custom-multi-item-context-menu-entry-for-windows-explorer)、
   [Stack Overflow #6440715](https://stackoverflow.com/questions/6440715)）。**此点必须以 Windows 真机实测为准**，见 §8。
   若需要「多选一次调用」，官方推荐的可靠做法是 COM 的 `IDropTarget`（或 `IExplorerCommand`）：文档明确
   IDropTarget 不受 DDE/CreateProcess 的缓冲区限制，且能把整个选择以 data object 传入。
5. **Win11 新菜单**：扩展必须 `IExplorerCommand` + 应用身份（packaged 或 sparse package）；「显示更多选项」原样加载
   Windows 10 菜单，Shift+F10 / 菜单键同样打开旧菜单；新菜单按应用分组、最多 1 个 Verb 直接显示、多个 Verb 收进应用归属的浮出菜单。
   （[Windows Developer Blog 2021-07-19](https://blogs.windows.com/windowsdeveloper/2021/07/19/extending-the-context-menu-and-share-dialog-in-windows-11/)、
   [Add a File Explorer context menu command to a packaged desktop app](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/integrate-packaged-app-with-file-explorer)）

---

## 3. Win10 / Win11 呈现差异

| 场景 | Windows 10（2004+） | Windows 11（21H2+） |
|---|---|---|
| 静态 Verb（HKCU\Software\Classes\*\shell） | **主右键菜单直接显示** | 只在「**显示更多选项**」里（Shift+F10 / 菜单键 / 右键菜单底部入口） |
| 静态 Verb（Directory / Directory\Background / Drive） | 主菜单直接显示 | 同上，落在旧菜单 |
| SendTo 快捷方式（`shell:sendto`） | 「发送到」子菜单可见 | 「发送到」子菜单**仍在新菜单中**（社区实测；建议真机复核） |
| IExplorerCommand（有应用身份） | 可显示（Win7 起支持） | **新菜单一级可见**；多个 Verb 收进应用浮出菜单 |
| IExplorerCommand（无应用身份/未装包） | —（通常不可用） | 不进入新菜单 |
| 拖放到 exe / exe 快捷方式 | 可触发（命令行带路径） | 同左（拖到任务栏图标的行为跨版本差异大，不作为主入口） |

> **结论**：如果目标用户包含 Win11 且希望「右键第一眼就能看到」，只有 `IExplorerCommand + 身份` 一条路；
> 如果接受「显示更多选项 / 发送到」两级入口，静态 Verb + SendTo 是成本最低、兼容性最好的组合。
> 对 BluetoothTransfer 这种轻量免安装工具，**优先推荐后者**（免安装、免签名、免管理员）。

---

## 4. 方案对比

| 维度 | A. HKCU 静态 Verb | B. IExplorerCommand + MSIX Sparse Package | C. SendTo 发送到 | D. 拖放到 exe / 快捷方式 |
|---|---|---|---|---|
| Win10 呈现 | 主菜单 ✅ | 主菜单 ✅ | 发送到子菜单 ✅ | 需用户手动拖 ✅ |
| Win11 新菜单 | ❌ 落「显示更多选项」 | ✅ 一级菜单 | ✅ 发送到子菜单（社区实测） | ❌ 需拖到窗口/图标 |
| 权限要求 | **HKCU，免管理员** | 安装 MSIX 包需要签名/信任；注册包通常当前用户可装，但**签名证书**是硬门槛 | **免管理员**（写 %APPDATA% 用户目录） | 无 |
| 安装 / 卸载 | 写/删 4–6 个注册表键，可应用内一键完成 | 打包→签名→`Add-AppxPackage` 注册；升级/卸载走 MSIX；sparse 包还要维护清单/证书信任 | 复制/删除一个 `.lnk`；可脚本化 | 无安装；教用户建快捷方式 |
| 多选 | ⚠️ 逐项调用/命令行 2000 字符限制，不可靠；建议只用于单选 | ✅ `IShellItemArray` 拿全选，无限制 | ✅ **一次调用拿到全部路径**（32767 字符上限） | ✅ 一次调用多文件；但发现性差 |
| 图标 | ✅ Verb 的 `Icon` 值（`"exe",0`），复用 app.ico | ✅ `GetIcon` 返回图标资源 | ✅ 用 .lnk 的图标 | ✅ 用 exe 图标 |
| 显示文案/本地化 | ✅ `MUIVerb` 可中文 + 加速键 | ✅ `GetTitle` 动态返回，可按选择变化 | ✅ .lnk 名称（中文） | — |
| 条件显示 | ✅ 可用 `AppliesTo`（AQS）、`Extended`（仅 Shift 显示） | ✅ `GetState` 动态启用/隐藏 | ❌ 不区分文件类型 | — |
| 开发成本 | **低**（1 人日级） | **高**（原生 COM DLL + 打包 + 签名 + CI，5–10 人日） | **极低**（0.1 人日） | 无开发 |
| 维护成本 | 低（注册表键极少） | 高（证书、版本、Win11 行为变化） | 极低 | 无 |
| 安全风险 | 低：用户级注册表、仅激活已安装 exe；注意命令行注入与卸载残留 | 中：包身份/签名信任、COM 进程内加载（微软建议菜单构建路径必须快、重活放 Invoke）；签名私钥管理 | 低：用户目录写快捷方式，卸载需清理 | 低 |
| 主要风险 | Win11 新菜单不可见；多选不可靠；命令行引号/编码 | 签名与分发门槛；COM 注册失败导致资源管理器卡顿/崩溃风险；自签名证书用户信任成本 | 「发送到」在 Win11 新菜单的可见性需真机确认；用户习惯差异 | 无法作为产品主入口 |

**结论：A + C 组合是最优「性价比」方案；B 作为后续可选；D 作为兜底/文档补充。**

补充说明（A 的进阶形态）：静态 Verb 可以指定 `DropTarget`（COM `IDropTarget`）来接收整个选择，
从而绕开命令行 2000 字符限制和多选逐项调用问题；这仍属于「无应用身份的静态 Verb」，
不会进入 Win11 新菜单，但能实现可靠的「多选一次调用」。若后续不愿引入 MSIX，
又必须支持多选一键发送，这是折中方案（成本约 2–4 人日，需要一个原生 COM DLL 或 .NET COM Host）。
参考：[Choosing a Static or Dynamic Shortcut Menu Method](https://learn.microsoft.com/en-us/windows/win32/shell/shortcut-choose-method)。

---

## 5. 与现有应用的对接改造清单

### 5.1 命令行入口（必须）

**目标接口（建议）：**

```text
BluetoothTransfer.exe --send "C:\a\one.txt" "C:\data\docs" "C:\b\two.pdf"
BluetoothTransfer.exe --send-text "C:\notes\note.txt"
BluetoothTransfer.exe --send --kind=folder "C:\data\docs"     # 可选，显式声明
```

规则：

1. `--send` 后的**全部剩余参数**都按路径解析（允许空格、中文、emoji、UNC）；
2. 逐个路径做 `File.Exists` / `Directory.Exists` 判断：目录 → `TypeFolder`（zip 后发送），文件 → `TypeFile`；
3. `--send-text` 只接受文件：读取文本内容（UTF-8/UTF-8 BOM/UTF-16 自动探测），以 `TypeText` 发送；
   二进制/超大文件回退为按文件发送，并在状态栏/日志说明；
4. 路径去重、数量上限（建议 200，超出提示分批）、`>4GB`（OPP 上限）提前拒绝并提示；
5. 解析必须用 `Environment.GetCommandLineArgs()` / `StartupEventArgs.Args`，**不要自己拆分命令行字符串**；
6. 参数入口要先于建窗（HEAD 的 `StartupUri` 必须让位；Slice C 工作区已移除，方向正确）。

**改动点（HEAD 基线）：**

| 文件 | 改动 |
|---|---|
| `src/BluetoothTransfer/App.xaml` | 移除 `StartupUri`（Slice C 已做） |
| `src/BluetoothTransfer/App.xaml.cs` | 在 `OnStartup` 最前面解析 `e.Args`；构造 `PendingSendRequest`；先处理单实例转发，再建窗 |
| 新增 `src/BluetoothTransfer/Services/ShellSendRequest.cs`（建议） | 纯逻辑解析/校验 `--send`/`--send-text`（可单测，不依赖 WPF） |
| `src/BluetoothTransfer/MainWindow.xaml.cs` | 暴露 `HandleExternalSendRequest(PendingSendRequest)`（与 Slice C 的 `ActivateFromSecondInstance` 并列） |
| `src/BluetoothTransfer/ViewModels/OppViewModel.cs` | 新增 `EnqueueExternalSendAsync(IReadOnlyList<string> paths, ExternalSendKind kind)`；无设备时进入「待发送」状态；新增 `SendPendingExternalCommand` 或「选择设备后自动发送」逻辑 |
| `AppConfig.cs` | 可选：`LastDeviceAddr`（记住上次设备，供 `--send` 无交互自动发送的「快速通道」） |

### 5.2 单实例转发（与第 4 项 Slice C 协同）——**当前实现的最大缺口**

Slice C 的 `SingleInstanceGuard`（工作区新增）只做「Mutex 判定 + 命名事件激活」，
**不携带命令行参数**。右键菜单场景下，用户很可能在应用已运行时右键发送，二次进程必须把路径交给主实例。

推荐扩展（三选一，按成本从低到高）：

| 方案 | 做法 | 优点 | 风险/代价 |
|---|---|---|---|
| **命名管道（推荐）** | 主实例后台线程跑 `NamedPipeServerStream("Local\BluetoothTransfer.Gui.Pipe")` 循环；二次实例 `NamedPipeClientStream` 连接（2s 超时），写入 JSON `{kind, paths[], ts}`，关闭后退出 | 传输可靠、长度不受限、可回执失败 | 需处理管道 ACL（`PipeOptions.CurrentUserOnly` 可用）、主实例未建管道时的短重试 |
| **临时 inbox 文件 + 命名事件** | 二次实例把 JSON 写到 `%TEMP%\BluetoothTransfer\inbox\{guid}.json`，再 Set 现有命名事件；主实例收到激活后扫描 inbox | 改动最小、崩溃安全、不需要管道 | 事件语义被复用（激活 = 可能有载荷）；需要清理孤儿文件；并发多个二次实例时按文件名排序消费 |
| `WM_COPYDATA` | 主实例注册 message-only 窗口，二次实例 `FindWindow` + `SendMessage` | 经典、同步回执 | 需要 Win32 窗口句柄/消息循环；UIPI 下低权限进程向高权限进程发送会被拒（本应用一般同权限，可接受） |

**顺序要求：**

1. `OnStartup` 先解析参数得到 `PendingSendRequest`；
2. 建 `SingleInstanceGuard`；
3. 非主实例：先**转发载荷**，成功 → 退出码 0；失败 → 释放守卫后重新尝试成为主实例（当前 Slice C 已有「重试接管」逻辑，转发失败时也应走同一路径，**不能直接静默退出**，否则用户右键后没有任何反应）；
4. 主实例：激活窗口（已有）+ `Dispatcher.BeginInvoke` → `MainWindow.HandleExternalSendRequest(...)`。

> 建议同步在 `SingleInstanceGuardTests` 之外新增转发通道的单测（管道序列化/反序列化、超时、主实例未就绪重试），
> 真实跨进程行为仍需 Windows 验证。

### 5.3 右键后「选择目标设备」的交互

这是本功能真正的 UX 难点：右键菜单没有「设备」上下文。建议按优先级实现：

1. **预置队列 + 引导选设备（默认，最稳）**：
   - 主窗口被激活到前台，状态栏显示「已收到 3 个待发送项：请选择目标设备」；
   - 队列中显示 3 条 Pending 任务（复用 `OppSendQueue`/任务列表 UI，不自动开始）；
   - 若设备列表为空则自动触发一次扫描；
   - 用户选中设备后：提供「发送待发送项」按钮，或选择设备时自动开始（二选一，需产品定）；
   - 待发送项在用户取消/关闭窗口时丢弃并清理（不入 journal）。
2. **记住上次设备（快速通道，可选配置）**：
   - `AppConfig.LastDeviceAddr`（或复用「最近连接」排序第一名）；
   - 若该设备仍已配对且在设备列表中，`--send` 直接入队并开始，状态栏显示「正在向 X 发送」；
   - 未命中则回退方案 1。**默认关闭自动发送**更安全（避免用户误发到错误设备）。
3. **独立「选择设备」小窗**：右键后立即弹一个设备选择对话框，确认后入队发送。
   UX 最顺，但需要新窗口/新 ViewModel，工作量 +1 人日左右，建议 Phase 1.5 再做。

**实现要点：**`OppViewModel` 现有发送流程强依赖 `SelectedDevice` 且与 UI 线程耦合（`EnqueueAndStart`、`StartQueueIfNeeded` 都在 UI 线程增删 `ObservableCollection`）。
新增「外部待发送」时不要绕开它，建议：
- 保存 `_externalPending: List<OppSendJob>`；
- 在 `SelectedDevice` setter / `SendPendingExternalCommand` 中把 `_externalPending` 交给 `EnqueueAndStart`；
- 不改变现有 `SendDroppedAsync` 语义（它仍要求已选设备），外部入口只多一层「待发送缓冲」。

### 5.4 多文件 / 文件夹 / 文本处理矩阵

| 右键对象 | 菜单项 | 应用行为 |
|---|---|---|
| 单个文件（任意） | 用蓝牙传输发送 | `--send "<file>"` → TypeFile，按文件发送 |
| 多个文件（SendTo 入口） | 发送到 > 蓝牙传输 | 一次调用带全部路径 → N 条 TypeFile 任务顺序发送（Android OPP 同时只接受 1 个传输，队列天然适配） |
| 文件夹（单个/多选） | 用蓝牙传输发送 | `--send "<dir>"` → TypeFolder，自动 zip 后发送（现有逻辑） |
| 文件夹空白处 | 用蓝牙传输发送当前文件夹 | `--send "%V"` → 当前目录作为文件夹发送 |
| 驱动器根 | 用蓝牙传输发送 | `--send "%1"` → 盘符根目录 zip（大目录需二次确认） |
| `.txt`/`.md`/`.log` 等文本 | 用蓝牙发送文本内容 | `--send-text "<file>"` → 读内容为 TypeText（走 `.txt` 包装），失败回退按文件发送 |
| 15 选以上（静态 Verb） | — | 静态 Verb 不显示（Document 默认 15 项上限）；提示用户用「发送到」或减少选择（[官方 KB：超过 15 个文件时上下文菜单项消失](https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/context-menus-shortened-select-over-15-files)） |

### 5.5 路径合法性与失败提示

- **合法性**：存在性、去重、长度（`\\?\` 长路径支持需 app manifest `<longPathAware>`，当前未配置——建议评估）、UNC/网络路径（可能阻塞，需给「正在读取网络路径」提示或超时）、>4GB、无权限文件；
- **失败提示**：统一走 `EventBus` 日志 + 状态栏 + 必要时的 `MessageBox`；不得静默失败；
  - 「第二个实例转发失败」→ 回退为主实例启动（见 §5.2）；
  - 「没有可用蓝牙适配器 / 蓝牙已关闭」→ 提示并引导打开系统蓝牙设置；
  - 「设备未配对」→ 复用现有中文提示；
  - 「路径已被移动/删除」→ 状态栏列出失败项，不要阻断其余项；
- **安全**：
  - 命令模板中**所有路径和 exe 路径都必须加引号**；绝不用 `cmd.exe /c`、PowerShell 包装（注入风险 + 控制台闪窗 + 慢）；
  - 应用只接受「已存在路径」，不执行任何 Shell 命令；不因右键而自动开始发送（除用户显式开启的快速通道）；
  - 安装/卸载只写 `HKCU`，不写 `HKLM`，不请求提权。

---

## 6. 可直接落地的注册表示例（HKCU，不执行）

> 以下为**示例文本**，本报告不导入注册表。文件需保存为 **UTF-16 LE（带 BOM）** 才能在中文 Windows 上正确写入中文；
> 也可用 PowerShell `New-Item`/`New-ItemProperty` 或应用内 `Microsoft.Win32.Registry` 写入。
> 假设安装路径为 `C:\Program Files\BluetoothTransfer\BluetoothTransfer.exe`（请随安装位置替换；注意 .reg 中反斜杠要写成 `\\`、引号写成 `\"`）。

### 6.1 安装（单选，Win10 主菜单 / Win11 显示更多选项）

```reg
Windows Registry Editor Version 5.00

; ===== 文件（任意类型）=====
[HKEY_CURRENT_USER\Software\Classes\*\shell\BluetoothTransfer.Send]
@="用蓝牙传输发送"
"MUIVerb"="用蓝牙传输发送(&B)"
"Icon"="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\",0"
"MultiSelectModel"="Single"
"Position"="Top"

[HKEY_CURRENT_USER\Software\Classes\*\shell\BluetoothTransfer.Send\command]
@="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\" --send \"%1\""

; ===== 文件夹（选中文件夹时）=====
[HKEY_CURRENT_USER\Software\Classes\Directory\shell\BluetoothTransfer.Send]
@="用蓝牙传输发送"
"MUIVerb"="用蓝牙传输发送(&B)"
"Icon"="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\",0"
"MultiSelectModel"="Single"
"Position"="Top"

[HKEY_CURRENT_USER\Software\Classes\Directory\shell\BluetoothTransfer.Send\command]
@="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\" --send \"%1\""

; ===== 文件夹空白处（%V = 当前文件夹）=====
[HKEY_CURRENT_USER\Software\Classes\Directory\Background\shell\BluetoothTransfer.Send]
@="用蓝牙传输发送当前文件夹"
"MUIVerb"="用蓝牙传输发送当前文件夹(&B)"
"Icon"="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\",0"
"Position"="Top"

[HKEY_CURRENT_USER\Software\Classes\Directory\Background\shell\BluetoothTransfer.Send\command]
@="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\" --send \"%V\""

; ===== 驱动器 =====
[HKEY_CURRENT_USER\Software\Classes\Drive\shell\BluetoothTransfer.Send]
@="用蓝牙传输发送此驱动器"
"MUIVerb"="用蓝牙传输发送此驱动器(&B)"
"Icon"="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\",0"
"MultiSelectModel"="Single"
"Position"="Top"

[HKEY_CURRENT_USER\Software\Classes\Drive\shell\BluetoothTransfer.Send\command]
@="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\" --send \"%1\""

; ===== 文本文件：发送文本内容（仅 .txt；可复制此段扩展 .md/.log 等）=====
[HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\.txt\shell\BluetoothTransfer.SendText]
@="用蓝牙发送文本内容"
"MUIVerb"="用蓝牙发送文本内容(&T)"
"Icon"="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\",0"
"MultiSelectModel"="Single"
"Position"="Top"

[HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\.txt\shell\BluetoothTransfer.SendText\command]
@="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\" --send-text \"%1\""
```

说明：

- `MUIVerb` 是新版 Shell 使用的显示文本（Win10/11 旧菜单均识别），`@` 为默认值做兼容；两者都写最稳；
- `Icon` 指向 exe 的第 0 个图标（应用已配置 `Assets\app.ico`，无需额外资源）；
- `Position=Top` 让菜单位于旧菜单顶部（可选）；
- `SystemFileAssociations\.txt` 只影响 `.txt`；如需覆盖更多文本扩展，按同样结构注册 `.md`、`.log` 等，或改用 `AppliesTo` AQS 条件。

### 6.2 多选：两种可选写法

**（a）SendTo 快捷方式（推荐，Win10/Win11 新菜单的「发送到」子菜单均可用，一次拿到全部路径）**

```powershell
# 以当前用户运行；等价于把快捷方式放进 shell:sendto
$sendTo = [Environment]::GetFolderPath('SendTo')
$target = 'C:\Program Files\BluetoothTransfer\BluetoothTransfer.exe'
$lnkPath = Join-Path $sendTo '蓝牙传输.lnk'
$shell = New-Object -ComObject WScript.Shell
$lnk = $shell.CreateShortcut($lnkPath)
$lnk.TargetPath = $target
$lnk.Arguments = '--send'          # 拖入/选中的文件路径由 Shell 追加在参数后
$lnk.IconLocation = "$target,0"
$lnk.WorkingDirectory = Split-Path $target
$lnk.Description = '用蓝牙传输发送'
$lnk.Save()
```

卸载：`Remove-Item (Join-Path ([Environment]::GetFolderPath('SendTo')) '蓝牙传输.lnk')`。

**（b）静态 Verb + `MultiSelectModel=Player`（可选实验，不建议作为唯一多选入口）**

```reg
[HKEY_CURRENT_USER\Software\Classes\*\shell\BluetoothTransfer.SendMulti]
@="用蓝牙传输发送（多选）"
"MUIVerb"="用蓝牙传输发送（多选）(&M)"
"Icon"="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\",0"
"MultiSelectModel"="Player"
"Position"="Top"

[HKEY_CURRENT_USER\Software\Classes\*\shell\BluetoothTransfer.SendMulti\command]
@="\"C:\\Program Files\\BluetoothTransfer\\BluetoothTransfer.exe\" --send \"%1\""
```

> ⚠️ 风险：静态 Verb 多选的实际行为在不同 Windows 版本/选择数量下不一致（可能是逐项 `--send` 调用，
> 也可能是全部追加到命令行）。若采用此写法，应用侧必须实现「二次启动去抖合并」：
> 每个二次进程把路径转给主实例，主实例在收到第一项后等待 ~300–600ms 再统一入队，
> 否则会变成 N 次独立发送流程。**建议 Phase 1 只用（a）承担多选。**

### 6.3 卸载（删除键）

```reg
Windows Registry Editor Version 5.00

[-HKEY_CURRENT_USER\Software\Classes\*\shell\BluetoothTransfer.Send]
[-HKEY_CURRENT_USER\Software\Classes\Directory\shell\BluetoothTransfer.Send]
[-HKEY_CURRENT_USER\Software\Classes\Directory\Background\shell\BluetoothTransfer.Send]
[-HKEY_CURRENT_USER\Software\Classes\Drive\shell\BluetoothTransfer.Send]
[-HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\.txt\shell\BluetoothTransfer.SendText]
[-HKEY_CURRENT_USER\Software\Classes\*\shell\BluetoothTransfer.SendMulti]
```

> 应用内「安装/卸载右键菜单」按钮建议走 `HKCU\Software\Classes` 的直接 API 写入（`Registry.CurrentUser.CreateSubKey`），
> 删除时只删自己写的 Verb 键（`BluetoothTransfer.*` 前缀），不要动其它软件注册项；
> 卸载程序也应清理 SendTo 快捷方式。

### 6.4 建议的 exe 命令格式（引号/空格）

| 场景 | 模板 |
|---|---|
| 单选文件/文件夹 | `"C:\Program Files\BluetoothTransfer\BluetoothTransfer.exe" --send "%1"` |
| 文件夹空白处 | `"C:\...\BluetoothTransfer.exe" --send "%V"` |
| 文本内容 | `"C:\...\BluetoothTransfer.exe" --send-text "%1"` |
| SendTo | 快捷方式 Target=`"C:\...\BluetoothTransfer.exe"`，Arguments=`--send`，Shell 追加每个选中路径（自动加引号语义由 Shell 处理） |
| Win11 新菜单（Phase 3） | `Invoke(IShellItemArray*)` 中取每一项 `IShellItem::GetDisplayName(SIGDN_FILESYSPATH)`，内部走与 `--send` 相同的入口函数（不拼命令行） |

要点：

- **exe 路径、每个路径都加引号**；`%1`/`%V` 必须整体放进引号；
- 不要用 `cmd /c`、`powershell -Command`、`rundll32`；
- 静态 Verb 命令行 ≤ 约 2000 字符（微软文档），多选场景不要依赖它；
- 命令行总长上限 32767 字符（CreateProcess/SendTo 场景），应用应做数量/长度上限保护。

---

## 7. 工作量与风险估算

### 7.1 工作量（1 人，熟悉 WPF 的工程师）

| 工作项 | 估算 | 说明 |
|---|---|---|
| `--send`/`--send-text` 解析 + 校验（含单测） | 0.5 人日 | 纯逻辑，可 Linux harness 测试 |
| 单实例带载荷转发（管道或 inbox 文件） | 0.5–1 人日 | 含主实例未就绪重试、失败回退；Windows 跨进程测试 |
| GUI 待发送队列 + 设备选择引导 | 0.5–1 人日 | 复用 `OppSendQueue`；注意 UI 线程/`ObservableCollection` |
| 安装/卸载右键菜单（应用内按钮或 .reg + 说明） | 0.5 人日 | HKCU 写删 + 失败提示 |
| SendTo 快捷方式安装/卸载 | 0.1 人日 | PowerShell 或 COM `WScript.Shell` |
| 手工测试（Win10/Win11 × 单选/多选/文件夹/背景/中文路径/应用已运行） | 1 人日 | 必须真机 |
| **Phase 1 合计** | **约 3–4.5 人日**（含测试） | 不含发布/文档 |
| Phase 2：静态 Verb + `IDropTarget` COM（可靠多选） | +2–4 人日 | 原生 DLL/COM Host + 注册 |
| Phase 3：`IExplorerCommand` + MSIX Sparse Package | +5–10 人日 | 原生 COM DLL、清单、签名/证书、安装/升级/卸载、CI |

### 7.2 风险清单

| 风险 | 等级 | 缓解 |
|---|---|---|
| Win11 新菜单不显示（用户以为没生效） | 高（体验） | 文档/首次安装提示「Win11 请在『显示更多选项』或『发送到』中使用」；Phase 3 再进新菜单 |
| 静态 Verb 多选逐项调用导致 N 个进程/多次确认 | 高 | Phase 1 只用单选静态 Verb + SendTo 做多选；如需多选静态入口，应用侧去抖合并或上 COM |
| 命令行长度/数量上限 | 中 | 静态 Verb ≤2000 字符、SendTo ≤32767 字符；应用侧上限+提示 |
| 二次启动转发失败 → 右键无反应 | 中 | 转发失败回退为主实例启动（不能静默退出）；日志 |
| 路径含空格/中文/emoji/引号/UNC | 中 | 全量引号；`Environment.GetCommandLineArgs` 解析；真机用例覆盖 |
| 注册表残留/升级路径变化 | 中 | 应用内安装/卸载统一入口；只删自己的键；安装路径迁移时更新 Icon/command |
| 企业环境 Shell 策略/安全软件拦截 | 中低 | 只写 HKCU；不注入 Explorer；提供「不安装右键菜单」选项 |
| MSIX 签名与分发（Phase 3） | 高 | 需要受信证书/商店或企业分发；自签名需用户信任证书；提前评估 |
| COM 扩展导致资源管理器卡顿/崩溃 | 中（Phase 2/3） | 菜单构建路径零 IO、重活放 `Invoke`；异常吞噬兜底；灰度发布 |

### 7.3 必须 Windows 实测才能定论的项

1. 静态 Verb + `MultiSelectModel=Player` 在 Win10/Win11 上到底是「一次调用带全部路径」还是「逐项调用」；
2. Win11 新菜单「发送到」子菜单是否稳定显示用户自定义 SendTo 目标（不同版本/升级场景）；
3. `%V` 在 `Directory\Background` 的实际展开值（是否带尾随 `\`、根目录/驱动器上的表现）；
4. 15/100 项上限与「显示更多选项」中的实际可见性；
5. 中文/emoji/超长路径/UNC 路径在 `CreateProcess` 激活时的编码与引号行为；
6. Win11 右键新菜单刷新时机（安装后是否需要重启 Explorer）。

---

## 8. 推荐方案（最终）

**推荐：Phase 1 =「HKCU 静态 Verb（单选）+ SendTo 快捷方式（多选）+ GUI `--send` 入口 + 单实例带载荷转发 + 待发送队列/设备选择引导」。**

理由：

- 免管理员、免安装包、免签名，符合项目「轻量、免安装」定位；
- 覆盖 Win10 主菜单与 Win11「显示更多选项 / 发送到」，两代系统都能用；
- 复用现有发送队列、拖放分类逻辑，开发量最小；
- 多选走 SendTo，规避静态 Verb 多选不可靠与命令行长度限制；
- 与第 4 项单实例改造天然衔接（只差「载荷通道」一小步）；
- Win11 新式一级菜单作为可选 Phase 3，只有确认用户强需求且能接受签名/打包成本时再做。

**实施顺序建议：**

1. 先做 `--send` 解析与单实例转发（纯逻辑 + 可单测）；
2. 再做 GUI 待发送队列/设备选择；
3. 最后做注册表/SendTo 安装卸载入口 + 文档；
4. Win10/Win11 真机验证清单（§7.3）通过后再发布；
5. 观察用户反馈，决定是否投入 Phase 2（COM 多选）或 Phase 3（Win11 新菜单）。

---

## 9. 参考链接

微软官方：

- [Extending Shortcut Menus](https://learn.microsoft.com/en-us/windows/win32/shell/context)
- [Verbs and File Associations](https://learn.microsoft.com/en-us/windows/win32/shell/fa-verbs)
- [Creating Shortcut Menu Handlers](https://learn.microsoft.com/en-us/windows/win32/shell/context-menu-handlers)（含 HKCU\Software\Classes 免管理员说明、`IDropTarget` 多选说明）
- [How to Employ the Verb Selection Model](https://learn.microsoft.com/en-us/windows/win32/shell/how-to-employ-the-verb-selection-model)（`MultiSelectModel` / Document 15 / Player 100）
- [Choosing a Static or Dynamic Shortcut Menu Method](https://learn.microsoft.com/en-us/windows/win32/shell/shortcut-choose-method)（静态 Verb 的 CreateProcess 限制：2000 字符命令行、无法复用进程）
- [Best Practices for Shortcut Menu Handlers and Multiple Verbs](https://learn.microsoft.com/en-us/windows/win32/shell/verbs-best-practices)（多选只应看第一项/总数；Player 应乐观跳过不支持项）
- [Add a File Explorer context menu command to a packaged desktop app](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/integrate-packaged-app-with-file-explorer)（IExplorerCommand + app identity / sparse package / `windows.fileExplorerContextMenus`）
- [IExplorerCommand 接口](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-iexplorercommand)
- [Windows Developer Blog：Extending the Context Menu and Share Dialog in Windows 11](https://blogs.windows.com/windowsdeveloper/2021/07/19/extending-the-context-menu-and-share-dialog-in-windows-11/)（新菜单/显示更多选项/Shift+F10）
- [Context menus are shortened when more than 15 files are selected](https://learn.microsoft.com/en-us/troubleshoot/windows-client/shell-experience/context-menus-shortened-select-over-15-files)（Document 多选 15 项上限）

社区/实测参考（非官方，注意甄别）：

- [Stack Overflow #6440715：How to pass multiple filenames to a context menu shell command?](https://stackoverflow.com/questions/6440715)
- [SuperUser #365704：Custom multi-item context-menu entry for Windows Explorer](https://superuser.com/questions/365704/custom-multi-item-context-menu-entry-for-windows-explorer)（静态 Verb + `MultiSelectModel=Player` 实测未能合并为一次调用）
- [Microsoft Q&A（日）：Windows 11「送る」への対象の登録について](https://learn.microsoft.com/ja-jp/answers/questions/4040189/windows-11)（Win11 新菜单下 SendTo 目标可见性的用户实测）

---

## 10. 附：本报告未做 / 不能做的事

- 未修改任何注册表、未写生产代码、未运行 Windows 真机验证（本机为 Linux 交叉编译环境）；
- 所有 Win10/Win11 呈现差异、多选行为、SendTo 可见性均依据官方文档 + 社区实测，**落地前必须按 §7.3 在 Windows 真机复测**；
- 未评估商店上架（MSIX）的合规/审核要求，Phase 3 前需单独评估。
