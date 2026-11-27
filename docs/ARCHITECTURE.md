# 蓝牙传输架构

## 目标与范围

把现有 Windows 专用 .NET WPF 应用改写为跨平台 Rust 应用，实现现有全部功能并补齐原应用缺失的能力。功能域如下：

- 设备发现、配对、收藏、别名
- OPP 发送：单文件、多文件、文本、文件夹与单文件 zip
- 私有助手通道发送与断点续传
- 剪贴板与拖放发起
- 顺序队列与逐任务控制
- 接收服务与接收询问
- 记录、统计、筛选、删除、复制、导出
- 日志落盘
- 设置
- 托盘与通知
- 单实例与外部收件箱
- Windows 资源管理器集成
- Linux 桌面集成
- CLI
- 迁移工具
- 自检

当前交付为 2.0.0 重构版本：GUI、CLI、引擎、存储与平台集成均已接线，真实蓝牙与 Windows 路径的验证边界见下文。

## crate 依赖图与职责边界

    bt-core            纯逻辑模型，无操作系统依赖
      ↑
    bt-api             冻结契约，GUI 与 CLI 只依赖它
      ↑        ↑
    bt-store  bt-transport
      ↑        ↑
      bt-engine        协议状态机、队列、续传、journal、打包、探测
          ↑
        bt-app         装配根，唯一把其余 crate 拼成后端的位置
        ↑     ↑
    bt-gui   bt-cli
    bt-platform        平台集成，被 bt-gui 与 bt-cli 按需调用

依赖方向单向，禁止回指与循环。硬性约束：

- bt-core 不依赖任何其他工作区 crate，也不做文件系统与网络调用。
- bt-api 只依赖 bt-core 与 crossbeam-channel，不得依赖 bt-engine 或 bt-store。
- bt-gui 不得直接依赖 bt-engine，只能通过 bt-api 契约与 bt-app 装配根取后端。
- bt-store 与 bt-transport 互不依赖，由 bt-engine 组合。
- bt-app 是唯一的装配点，CLI 与 GUI 通过它获得 Arc 化的后端。

## bt-api 契约摘要

线程与调用模型：

- 后端一律 Send 加 Sync，装配后以 Arc 共享。
- 慢操作只排队并立即返回，进度与结果经事件回传，不得阻塞调用线程。慢操作包括扫描、配对、探测、入队、启停接收、导出、迁移与自检。
- 快查询同步返回缓存：配置、设备快照、任务快照、接收会话。
- 记录类读写同步返回结果：records、stats、delete_record、clear_records、copy_plan、export。
- 事件经 fn subscribe 返回独立通道；GUI 每帧轮询，CLI 阻塞带超时。

核心类型：

- 标识：JobId、AskId、PinRequestId 为进程内单调递增的 u64；TransferId 为整文件 SHA-256 的 32 字节值。
- 状态：JobState、RecordStatus、AssistantState、Channel、Direction、TransferMode、ProtectionLevel。
- 数据：Device、JobSnapshot、SendItem、Record、Stats、DayStat、CopyPlan、ExportSummary、SelftestReport、MigrateReport、ConfigPatch。

事件集合：

Log、ScanStarted、ScanFinished、DeviceList、DeviceUpdated、PairPrompt、PairResult、AssistantStateChanged、JobAdded、JobProgress、JobStateChanged、QueueIdle、ReceiveListeningChanged、ReceiveProgress、ReceiveAsk、ReceiveDone、ConfigChanged、RecordsChanged。

错误模型：统一 BtError，带 BtErrorKind 分类，便于上层按类分支而不解析文本。

密码边界：配置共 13 项设置语义，Config 承载其中 12 项；第 13 项 OPP 认证密码不进入 Config 与配置文件，只经 set_auth_password 落到系统私密存储或权限 0600 的独立文件。

配对应答：confirm 类请求走 answer_prompt，PIN 类请求走 answer_pin；传输层的 PromptHandler 返回 Accept、Reject 或携带 PIN 的应答，PIN 只用于本次配对，不进入日志、记录与 JSON。

配置项与默认值：

| 字段 | 默认 | 约束 |
| --- | --- | --- |
| opp_chunk_size | 65023 | 4096 至 65023 |
| opp_connect_timeout_s | 30 | 1 至 3600 |
| opp_send_idle_timeout_s | 300 | 1 至 86400 |
| opp_retry_count | 3 | 0 至 10 |
| opp_retry_delay_s | 3 | 0 至 300 |
| opp_protection | auto | auto、plain、encrypt |
| opp_name_bom | false | 布尔 |
| assistant_chunk_size | 262144 | 4096 至 1048557 |
| push_text_file_name | bt-note.txt | 清洗后的单段文件名 |
| receive_dir | 平台下载目录下的 BluetoothReceive | 非空 |
| receive_ask | true | 布尔 |
| mode | auto | auto、assistant、opp |

opp_chunk_size 的默认值取协议单包上限 65023；设计说明曾给出超过上限的默认值，两者不能同时成立，故以「取上限」实现，避免默认值被静默钳制。

助手通道分片上限由单帧 1 MiB 扣除 7 字节帧头与 12 字节 DATA 字段得到，即 1048557。

MockBackend 提供内存实现：可预置设备、事件脚本与配对交互类型，可查询调用记录，供 GUI 与 CLI 在无硬件条件下开发与测试。PIN 应答在 MockBackend 中只记录长度，不保存明文。

## 结果文件轻量化指标

| 指标 | 上限 | 实测 | 说明 |
| --- | --- | --- | --- |
| 接收最终文件额外字节 | 0 | 0 | bt-app e2e 断言 16 字节载荷落盘 16 字节 |
| 半成品 sidecar | 64 字节 | 53 字节 | 魔数 4 加传输标识 32 加偏移 8 加总长 8 加状态 1 |
| journal 单条目 | 256 字节 | 182 字节典型，253 字节最坏 | BTJ1 二进制条目，超长拒绝 |
| config 文件 | 只写非默认值 | 全默认 0 字节；mode=opp 13 字节 | TOML 空文件不落盘 |
| 导出 JSONL | 250 字节每行 | 内容 205 字节，含换行 206 字节 | 一条文本记录 |
| 导出 CSV | 200 字节每行 | 表头 74 字节，数据行 94 字节 | 一条含 CJK 名称的记录 |
| OPP 协议开销 | 0.03% 以内 | 0.00923% | 65023 字节分片，6 字节 OBEX 头 |
| 助手协议开销 | 0.01% 以内 | 0.00725% | 262144 字节分片，19 字节帧开销 |
| cli-only bt | 8 MB 以内 | 3,607,088 字节 | 关闭 gui 特性的 release 构建 |
| cli-only btrecv | 8 MB 以内 | 3,350,000 字节 | 同上 |
| GUI 单文件 bt-gui | 20 MB 以内 | 11,173,528 字节 | strip、LTO、opt-level=z |
| NSIS 安装器 | 15 MB 以内 | 未生成 | 本机无 makensis 与 Windows 链接器 |
| .deb | 20 MB 以内 | 约 5.60 MB（多次重建 5,603,710–5,603,912 字节） | 含 GUI、cli-only bt 与 btrecv；dpkg-deb 打包时间戳可致数 KB 差异 |

测量方法：

- 文件与安装器体积用 stat 取字节数；发布工作流对 GUI、CLI 与安装器产物做上限断言。
- 接收额外字节由 bt-app 的接收端到端测试断言落盘长度等于载荷长度；协议不写尾部元数据。
- sidecar 与 journal 由 bt-engine 的 size_budget 测试构造典型与最坏条目并断言上限。
- 导出体积在同一 REPL 会话发送一条文本记录后执行 export，用 wc -c 与 awk 统计字节。
- 协议开销按 (帧头 + DATA 字段开销) / 分片大小计算，分片取默认配置。
- 配置体积用全默认配置不落盘、仅写一个非默认值后 stat 验证。

## 续传并发与 journal

- journal 条目认领时写入状态 sending、持有者 PID 与启动标识、6 分钟租约。进程存活判定可靠的平台优先看进程是否仍存活，无法判定时退回租约。
- 发送进度回调每 60 秒调用一次 heartbeat：一次加锁内续租到当前时间加 6 分钟并更新已确认偏移。心跳只在有进度时触发；Linux 与 Windows 以进程存活判定为准，无进度也不会被误接管，其它平台在超过 6 分钟无进度时仍可能按租约被接管，边界见 docs/PROTOCOL.md。
- 用户取消把条目转为 paused 并保留自产载荷；只有 `bt resume cancel`、传输完成、abandon 规则（载荷缺失、内容变化、超过尝试上限）或 7 天 TTL 清理会删除条目。
- CLI 只在 `send`、`recv` 与 `gui` 流程启用 journal 自动恢复；查询与配置命令打开后端时不触发发送，续传由 `resume run` 显式执行。

## 平台策略

Linux：

- 设备发现、配对、SDP 与适配器状态走 BlueZ 的 D-Bus 接口。
- RFCOMM 客户端与服务端使用 AF_BLUETOOTH 与 SO_BT 绑定通道。
- 无 BlueZ 服务时返回 Unsupported，界面给出安装与启用提示。

Windows：

- 使用 windows crate 的 WinRT 绑定：BluetoothLEDevice、RfcommDeviceService、BluetoothDevice 配对代理。
- 资源管理器集成通过注册表写入右键菜单，卸载时精确回滚。
- 安装器为 NSIS，由 CI 在 Windows 运行器上生成。

验证边界：

- 本轮开发机为 WSL2，没有蓝牙硬件，/sys/class/bluetooth 不存在，系统 D-Bus 上没有 org.bluez，因此真实设备发现、配对、RFCOMM 收发与 OPP 互操作都无法在本机验证。
- 没有 Windows 运行环境，WinRT 路径只能做交叉编译与单元测试，功能验收需要在真实 Windows 机器上执行。
- 本机可验证的部分：全部 crate 的构建、单元测试、CLI 端到端命令、GUI 无窗口自检、协议编解码与数据格式。
- 蓝牙行为验收必须列明设备型号与操作步骤，不以「模拟通过」代替真机结论。

## 质量门禁

提交前必须全部通过：

- cargo fmt --all --check
- cargo clippy --workspace --all-targets -- -D warnings
- cargo test --workspace
- cargo build --workspace --release
- cargo build -p bt-cli --no-default-features
- cargo test -p bt-cli 的端到端用例
- bt --version 与 bt --help 的端到端命令
- bt-gui --selftest 以退出码 0 结束
- cargo xtask --help 可运行
- 代码中不得出现日志类或步骤类注释，功能性注释保持简洁

## 版本与发布

- 整个工作区共用单一版本号，当前为 2.0.0，写在根 Cargo.toml 的 workspace.package 中。
- MSRV 当前为 1.95，与 `clippy.toml` 的 `msrv`、README 和 CI 的 MSRV 构建任务一致；`cargo xtask msrv` 校验 workspace 声明不低于依赖要求。
- 发布标签形如 v2.0.0；标签必须与工作区版本一致，由 CI 校验。
- CI 矩阵覆盖 ubuntu-latest 与 windows-latest，执行格式化、静态检查、测试、发布构建与 GUI 自检。
- 发布产物：cli-only 压缩包、GUI 单文件可执行、NSIS 安装器、deb 与 AppImage；发布流程只允许在 CI 里执行。
- CLI 的 --version、GUI 标题版本、安装器脚本与 CI 版本校验都读取同一份 workspace.package.version；变更记录写入 CHANGELOG.md。
- 契约版本记在 bt-api 的 CONTRACT_VERSION，破坏性变更必须同时递增版本并在本文档记录。

## 界面页面清单

- 设备页：扫描与取消、设备列表、收藏、别名、配对、助手通道探测、发起发送
- 发送页：单文件、多文件、文本、文件夹与 zip 队列，逐任务暂停恢复取消重试移除，总进度与速率；单文件 zip 选项只对单个文件生效，多文件时提示后按原样发送
- 队列页：任务列表、状态与错误、暂停恢复取消重试移除、总进度
- 接收页：启停接收、落盘目录、接收询问开关、待决询问、接收进度
- 记录页：按方向、通道、状态、设备与关键字筛选，复制、删除、清空、导出
- 设置页：12 项配置、OPP 认证密码、迁移入口、自检入口
- 状态与日志：最近事件、状态栏提示、日志落盘位置
- 托盘与单实例联动：最小化到托盘、通知、外部投递唤起

## 命令行矩阵

| 命令 | 用途 | 退出码 |
| --- | --- | --- |
| `bt --version`、`bt --help` | 版本与命令帮助 | 0 |
| `bt devices list/scan/pair/alias/favorite` | 设备发现、配对、收藏与别名 | 成功 0；操作失败 1；参数 2；占用 3；权限 4；未找到 5 |
| `bt devices detect` | 助手通道三态探测 | present 0、absent 1、unknown 5 |
| `bt send file/files/text/folder` | 顺序发送并等待终态 | 成功 0；任一失败或取消 1；用法 2 |
| `bt recv`、`btrecv` | 接收服务；`btrecv` 无参直接监听 | Ctrl+C 停止 0；启动失败 1/3/4 |
| `bt records list/delete/clear/stats` | 记录查询、单条删除、清空与统计 | 成功 0；记录不存在 5；清空缺少 --yes 2 |
| `bt export csv\|jsonl` | 流式导出记录 | 成功 0；目标不可写 1/5 |
| `bt config list/get/set` | 查看与修改 12 项配置 | 成功 0；未知键或非法值 2/5 |
| `bt resume list/run/cancel` | 按 BTJ1 journal 查询、恢复与取消 | 成功 0；有条目但全部跳过 1 |
| `bt migrate` | 迁移旧版 .NET 数据 | 成功 0；来源不存在 5 |
| `bt selftest` | 无硬件自检 | 全部通过 0，否则 1 |
| `bt repl` | 同一后端会话执行多条命令 | 0 |
| `bt gui` 或无子命令 | 打开图形界面 | 成功 0；无显示或无 gui 特性 1/2 |
| `opp-scan` 等旧命令名 | 隐藏别名转发到新命令 | 与原命令一致 |

退出码约定：0 成功，1 操作失败，2 用法错误，3 已有实例或忙碌，4 权限或锁失败，5 未找到。配对 PIN 由 `answer_pin` 从 CLI 或 GUI 回填到传输层；`--pin` 不进入日志、JSON 与记录。

## 构建与验证入口

开发机与 CI 使用同一组命令：

- `. ./.toolchain/env.sh` 加载仓库内工具链；系统 Rust 1.95 及以上也可直接构建。MSRV 由根 `Cargo.toml` 的 `workspace.package.rust-version` 声明，依据是依赖声明的最高 `rust-version`（当前来自 eframe/egui 0.37 的 1.95），`cargo xtask msrv` 在本地与 CI 校验该一致性。
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo build --workspace --release`
- `cargo build -p bt-cli --no-default-features`
- `cargo run -p bt-gui -- --selftest`
- `cargo test -p bt-cli` 运行 CLI 端到端测试，使用临时 BT_HOME 与 BT_BACKEND=mock，不依赖蓝牙硬件。

旧 .NET 实现归档在 `legacy/dotnet/`，只读参考且不参与构建。真实蓝牙、Windows 安装器与 GUI 窗口的未验证边界见 `docs/ACCEPTANCE.md` 与 `docs/RELEASE.md`。
