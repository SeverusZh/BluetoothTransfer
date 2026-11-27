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

首轮交付只冻结契约与骨架，蓝牙与界面行为由后续任务补齐。

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

- 标识：JobId、AskId、PinRequestId、TransferId 为 32 字节 SHA-256。
- 状态：JobState、RecordStatus、AssistantState、Channel、Direction、TransferMode、ProtectionLevel。
- 数据：Device、JobSnapshot、SendItem、Record、Stats、DayStat、CopyPlan、ExportSummary、SelftestReport、MigrateReport、ConfigPatch。

事件集合：

Log、ScanStarted、ScanFinished、DeviceList、DeviceUpdated、PairPrompt、PairResult、AssistantStateChanged、JobAdded、JobProgress、JobStateChanged、QueueIdle、ReceiveListeningChanged、ReceiveProgress、ReceiveAsk、ReceiveDone、ConfigChanged、RecordsChanged。

错误模型：统一 BtError，带 BtErrorKind 分类，便于上层按类分支而不解析文本。

密码边界：配置共 13 项设置语义，Config 承载其中 12 项；第 13 项 OPP 认证密码不进入 Config 与配置文件，只经 set_auth_password 落到系统私密存储或权限 0600 的独立文件。

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

opp_chunk_size 的默认值取协议单包上限 65023。设计说明给定的 65536 超过同一份说明中的上限，两者不能同时成立，故以「取上限」实现，避免默认值被静默钳制。

助手通道分片上限由单帧 1 MiB 扣除 7 字节帧头与 12 字节 DATA 字段得到，即 1048557。

MockBackend 提供内存实现：可预置设备与事件脚本、可查询调用记录，供 GUI 与 CLI 在无硬件条件下开发与测试。

## 结果文件轻量化指标

| 指标 | 上限 | 说明 |
| --- | --- | --- |
| 接收最终文件额外字节 | 0 | 不追加尾部元数据，不写临时后缀残留 |
| 半成品 sidecar | 64 字节 | 实际 45 字节，魔数加传输标识加偏移加状态位 |
| journal 单条目 | 256 字节 | 竖线分隔文本，超长直接拒绝 |
| config 文件 | 只写非默认值 | 全默认时文件为空 |
| 导出 JSONL | 250 字节每行 | 紧凑字段集，空字段省略 |
| 导出 CSV | 200 字节每行 | 定长表头加转义 |
| OPP 协议开销 | 0.03% 以内 | 64 KiB 分片下的 OBEX 头占比 |
| 助手协议开销 | 0.01% 以内 | 256 KiB 分片下的 19 字节开销 |
| cli-only 发布体积 | 8 MB 以内 | 关闭 gui 特性的 bt 与 btrecv |
| GUI 单文件体积 | 20 MB 以内 | strip 与 opt-level z 之后 |
| NSIS 安装器 | 15 MB 以内 | 压缩后 |

测量方法：

- 文件与安装器体积用 stat 或 du -b 取字节数，发布前在 CI 里对产物断言。
- 接收额外字节用 sha256 对比发送端源文件与接收端落盘文件，两者必须一致。
- sidecar 与 journal 用 wc -c 或长度断言，单元测试已固化上限常量。
- 导出体积按行统计字节数，用 awk 或 Rust 断言单行上限。
- 协议开销等于线上字节数减去有效载荷字节数再除以有效载荷字节数，分片大小取配置默认值。
- 配置体积用文件字节数断言：全默认配置必须为空文件。

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
- cargo build --workspace
- cargo build -p bt-cli --no-default-features
- bt --version 与 bt --help 的端到端命令
- bt-gui --selftest 以退出码 0 结束
- cargo xtask --help 可运行
- 代码中不得出现日志类或步骤类注释，功能性注释保持简洁

## 版本与发布

- 整个工作区共用单一版本号，当前为 2.0.0，写在根 Cargo.toml 的 workspace.package 中。
- 发布标签形如 v2.0.0；标签必须与工作区版本一致，由 CI 校验。
- CI 矩阵覆盖 ubuntu-latest 与 windows-latest，执行格式化、静态检查、测试、发布构建与 GUI 自检。
- 发布产物：cli-only 压缩包、GUI 单文件可执行、NSIS 安装器；发布流程只允许在 CI 里执行。
- 契约版本记在 bt-api 的 CONTRACT_VERSION，破坏性变更必须同时递增版本并在本文档记录。

## 界面页面清单

- 设备页：扫描与取消、设备列表、收藏、别名、配对、助手通道探测、发起发送
- 发送页：单文件、多文件、文本、文件夹与 zip 队列，逐任务暂停恢复取消重试移除，总进度与速率
- 接收页：启停接收、落盘目录、接收询问开关、待决询问、接收进度
- 记录页：按方向、通道、状态、设备、时间与关键字筛选，复制、删除、清空、导出
- 设置页：12 项配置、OPP 认证密码、迁移入口、自检入口
- 状态与日志：最近事件、状态栏提示、日志落盘位置
- 托盘与单实例联动：最小化到托盘、通知、外部投递唤起

## 命令行矩阵

| 命令 | 用途 | 退出码 |
| --- | --- | --- |
| bt --version | 打印工作区版本 | 0 |
| bt --help | 打印命令与参数 | 0 |
| bt selftest | 逐项自检并输出结果 | 全部通过 0，否则 3 |
| bt send | 发送文件、文件夹或文本 | 待 T2 实现，当前 3 |
| bt recv | 启动接收服务 | 待 T2 实现，当前 3 |
| btrecv | 只做接收的薄壳 | 待 T2 实现，当前 3 |
| bt 无子命令 | 有 gui 特性时打开窗口，否则提示用法 | 打开失败 3，无界面构建 2 |

退出码约定：0 成功，1 自检未通过，2 用法错误，3 未实现或运行失败。

## 后续任务边界

为避免写冲突，按目录划分归属；共享文件只有根 Cargo.toml、rust-toolchain.toml 与本文件，改动需先协商。

T2 传输与协议

- 归属：crates/bt-transport 全部，crates/bt-engine 的 protocol 与 detect 子模块。
- 内容：BlueZ D-Bus 与 AF_BLUETOOTH 实现、WinRT 实现、OBEX 对象推送、助手帧状态机、配对与探测。
- 不改 bt-api 契约，不改 GUI 与 CLI。

T3 引擎与持久化

- 归属：crates/bt-engine 其余模块、crates/bt-store 全部、crates/bt-app 全部。
- 内容：顺序队列与调度、断点续传、journal、打包、接收服务、记录查询与统计、配置落盘、旧数据迁移、真实装配。
- 依赖 T2 的传输实现，可先用 bt-transport 的 mock 并行开发。

T4 界面与平台

- 归属：crates/bt-gui 全部、crates/bt-platform 全部、README.md。
- 内容：全部页面与交互、CJK 字体选择、托盘与通知、单实例、外部收件箱、Shell 集成、自启、窗口状态、README 重写。
- 只通过 bt-api 与 bt-app 取数据。

T5 归档与清理

- 归属：archive 目录与旧源码处置。
- 内容：把 src、tests、tools 与解决方案按功能对等清单归档，移除不再维护的构建脚本，保留历史可追溯。
- 不修改新工作区代码。

T6 CLI、发布与 Skill

- 归属：crates/bt-cli 全部、xtask 全部、.github/workflows、installers、skills。
- 内容：完整 CLI 命令与机器可读输出、退出码约定、xtask 打包任务、NSIS 与 CI 发布、面向 Agent 的 Skill 文档。
- 不修改核心引擎与 GUI 实现。

蓝牙硬件与 Windows 真机验收由 T2 与 T4 各自负责记录证据。
