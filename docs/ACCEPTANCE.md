# 功能对等验收清单

本文档逐条列出 Rust 版需要对齐的功能、验证方法与当前状态。状态含义：

- 已验证：命令或测试实际执行并通过，附可复现命令。
- 契约已验证：在 `BT_BACKEND=mock` 下验证了 CLI 契约、JSON 与退出码；真实蓝牙路径未验证。
- 未验证：缺少硬件、系统服务或对应实现，注明原因。
- 平台层已验证：GUI 无窗口自检、平台单元测试与打包脚本可重复执行；窗口渲染、托盘宿主面板与真机安装未验证。

环境边界：本轮开发机为 WSL2，无蓝牙硬件，`/sys/class/bluetooth` 不存在，D-Bus 上没有 `org.bluez`，无显示服务器。因此真实扫描、配对、RFCOMM、OPP 互操作与 WinRT 路径均不能在本机验证；GUI 无法在本机渲染。真机验收必须记录设备型号、系统版本与操作步骤。

通用验证入口：

```bash
. ./.toolchain/env.sh
cargo fmt --all --check
cargo clippy -p bt-cli --all-targets -- -D warnings
cargo test -p bt-cli
cargo build -p bt-cli --no-default-features
```

CLI e2e 用例位于 `crates/bt-cli/tests/e2e.rs` 与 `crates/bt-cli/tests/pair_pin.rs`，默认使用临时 `BT_HOME` 与 `BT_BACKEND=mock`。

## 设备

| 编号 | 功能 | 验证方法 | 状态 |
| --- | --- | --- | --- |
| DEV-01 | 扫描设备并展示名称、地址、配对状态 | `bt devices scan --seconds 1 --json`；e2e `devices_scan_reports_devices_and_events` | 契约已验证；真实蓝牙未验证 |
| DEV-02 | 仅已配对过滤 | `bt devices scan --paired-only --json` | 契约已验证 |
| DEV-03 | 设备缓存列表与三种排序 | `bt devices list --json --sort last\\|name\\|favorite`；e2e `devices_list_and_sort` | 契约已验证 |
| DEV-04 | 配对与配对交互 | `bt devices pair <ADDR> --pin <PIN>`；e2e `devices_alias_favorite_and_pair_work_in_one_session`、`pair_pin.rs`；bt-app e2e `pin_pairing_reaches_the_transport_handler` | 契约已验证：PIN 经 answer_pin 到传输层；真实蓝牙 PIN 未验证 |
| DEV-05 | 收藏与取消收藏 | `bt devices favorite <ADDR> [--unset]` | 契约已验证 |
| DEV-06 | 别名设置与清除 | `bt devices alias <ADDR> [ALIAS...]` | 契约已验证 |
| DEV-07 | 助手通道三态探测与退出码 | `bt devices detect <ADDR>`；e2e `devices_detect_uses_three_state_exit_codes`；单元测试 `detect_maps_three_states_to_exit_codes` | 契约已验证；absent 为单元验证，真实探测未验证 |
| DEV-08 | 打开系统蓝牙设置 | GUI/平台能力，非 CLI 命令 | 入口已接线；无显示环境未验证 |

`--pin` 或 GUI 对话框输入的 PIN 经 bt-api 的 `answer_pin` 传到 bt-app 与 bt-transport；Linux BlueZ 的 RequestPinCode 与 Windows ProvidePin 使用该值，不再固定回填 0000。PIN 不进入日志、记录与 JSON。

## 发送

| 编号 | 功能 | 验证方法 | 状态 |
| --- | --- | --- | --- |
| SND-01 | OPP 单文件发送 | `bt send file <ADDR> <PATH> --json`；e2e `send_file_and_folder_and_zip` | 契约已验证；真实 OPP 未验证 |
| SND-02 | 多文件顺序发送与汇总 | `bt send files <ADDR> <PATH>...`；e2e 同上 | 契约已验证 |
| SND-03 | 文本发送 | `bt send text <ADDR> <TEXT>`；e2e `send_text_returns_jobs_and_summary` | 契约已验证 |
| SND-04 | 文件夹自动打包发送 | `bt send folder <ADDR> <DIR>`；e2e `send_file_and_folder_and_zip`、`send_folder_rejects_zip32_limit_with_unsupported_error` | 契约已验证（引擎打包与 ZIP32 命令级预检）；真实 OPP 发送未验证 |
| SND-05 | 单文件 zip | `bt send file <ADDR> <PATH> --zip`；e2e `send_file_and_folder_and_zip` 校验名称与 pending 清理、`zip_rejects_4gib_source_and_leaves_no_artifact` 校验 4 GiB 上限；e2e `zip_cancel_keeps_payload_and_resume_run_finishes_it` 校验取消保留、续传完成清理与显式取消清理；e2e `send_rejects_user_source_inside_pending` 校验 pending 内源文件被拒绝且不删除；引擎 `enqueue_rejects_user_file_inside_pending`、journal `complete_keeps_payload_when_produced_flag_is_false` 校验显式 provenance；GUI `canceled_zip_job_keeps_payload_for_resume` 校验同一语义 | 契约已验证（含 4 GiB 拒绝、取消保留、显式 provenance 与终态清理；真实 OPP 未验证） |
| SND-06 | 通道模式 auto/assistant/opp | `--mode`；e2e 校验 assistant 通道与记录 | 契约已验证；真实助手通道未验证 |
| SND-07 | 助手通道探测与发送 | `devices detect` + `send --mode assistant`；bt-app e2e `assistant_send_end_to_end_uses_journal_and_records_completed` | 协议与引擎已用脚本化传输验证；真实 RFCOMM 未验证 |
| SND-08 | 断点续传 | `bt resume list/run/cancel`；e2e `resume_list_run_and_cancel_use_the_real_journal`、`resume_run_recovers_paused_entries_and_prunes_missing_payloads` | 契约已验证：BTJ1 journal 计划、paused 恢复与 abandon 清理；真实断线续传未验证 |
| SND-09 | 顺序队列与逐任务暂停/继续/取消/重试/移除 | 引擎队列单元测试与 GUI 队列页动作 | 已接线；无长传输可验证真实暂停与重试 |
| SND-10 | Ctrl+C 取消发送并保留续传信息 | e2e `cancel_keeps_resume_entry_and_resume_run_finishes_it`、`zip_cancel_keeps_payload_and_resume_run_finishes_it`：脚本化传输取消后 journal 保持 paused、自产 zip 保留，CLI `resume list/run` 可继续并在完成后清理；e2e `cli_sigint_keeps_zip_payload_for_resume` 用真实 CLI 二进制、SIGINT 与停顿链路复现同一路径；引擎单测 `cancel_preserves_paused_journal_entry`、`cancel_keeps_produced_zip_until_journal_terminal_state`、`completed_produced_zip_is_removed_with_journal_entry`；GUI `canceled_zip_job_keeps_payload_for_resume` | 契约已验证（含真实进程 SIGINT）；真实蓝牙长传输未验证 |
| SND-11 | 进度与速率 | `--events` 的 `job_progress`；e2e `send_events_are_ndjson_with_result_line` | 契约已验证 |
| SND-12 | 剪贴板与拖放发起 | GUI 发送页动作与平台剪贴板测试 | 已接线；无显示环境未验证真实剪贴板 |

## 接收

| 编号 | 功能 | 验证方法 | 状态 |
| --- | --- | --- | --- |
| RCV-01 | 接收服务启停与目录配置 | `bt recv --dir <DIR> --json`；e2e `btrecv_without_args_listens_and_stops_cleanly` | 契约已验证；真实接收未验证 |
| RCV-02 | Ctrl+C 干净退出 0 | 同上，SIGINT 后断言退出码 0 与 `stopped=true` | 已验证 |
| RCV-03 | 接收前询问 | `--ask`/`--no-ask`；非交互默认拒绝并提示 | 未验证（mock 无真实传输，需真机） |
| RCV-04 | 单次接收后退出 | `--once` | 契约已接；mock 无传输，未验证 |
| RCV-05 | SHA-256 校验、重名处理、半成品清理 | 引擎接收单元测试与 bt-app 接收 e2e | 引擎路径已验证；真实链路未验证 |
| RCV-06 | btrecv 无参数直接监听 | `btrecv`；SIGINT 测试 | 已验证（旧版缺陷已修复） |
| RCV-07 | btrecv 帮助与版本 | `btrecv --help`、`btrecv --version`；e2e `btrecv_help_and_version` | 已验证 |

## 记录与导出

| 编号 | 功能 | 验证方法 | 状态 |
| --- | --- | --- | --- |
| REC-01 | 记录列表 | `bt records list --json`；e2e `records_export_and_stats_flow_in_one_session` | 契约已验证 |
| REC-02 | 方向、类型、状态、对端、关键词筛选与分页 | `--direction/--type/--status/--peer/--search/--limit/--offset`；e2e `records_filters_and_missing_delete` | 契约已验证 |
| REC-03 | 单条删除与不存在提示 | `bt records delete <ID>`；退出 5 | 契约已验证 |
| REC-04 | 清空与数量准确 | `bt records clear --yes`；缺少 --yes 退出 2 | 契约已验证 |
| REC-05 | 统计 | `bt records stats --json` | 契约已验证 |
| REC-06 | CSV/JSONL 导出 | `bt export csv|jsonl <OUT> --json`；e2e 校验 CSV 表头与行数 | 契约已验证；真实大库未验证 |
| REC-07 | 记录复制计划 | bt-api copy_plan 与 bt-platform 剪贴板执行 | 单元测试已验证计划生成；真实剪贴板未验证 |
| REC-08 | 记录与 GUI 共享 | 同一 bt-app 后端与数据目录；GUI selftest 读取 MockBackend 记录 | 结构已验证；真实持久化共享未在窗口环境验证 |

## 配置与数据

| 编号 | 功能 | 验证方法 | 状态 |
| --- | --- | --- | --- |
| CFG-01 | 12 项配置查看与修改 | `bt config list/get/set`；e2e `config_flow_reports_effective_values` | 契约已验证 |
| CFG-02 | 越界值归一化 | `config set opp_chunk_size 999999` 后实际值 65023 | 已验证 |
| CFG-03 | OPP 认证密码只从 stdin/TTY 读取且不回显 | e2e `password_never_appears_in_argv_like_output_or_json` | 已验证 |
| CFG-04 | 密码不进配置文件 | 真实后端设置密码后检查：`auth.token` 权限 0600、`config.toml` 不含密码 | 已验证（2026-11-27 本机） |
| CFG-05 | 设置持久化 | 真实后端 `config set mode opp` 后新进程 `config get mode` 返回 opp，配置文件只含非默认值 | 已验证 |
| DAT-01 | 迁移旧 .NET 数据 | `bt migrate [--from DIR] --json`；e2e `migrate_reports_warnings_for_missing_legacy_root`；真实后端空目录返回 0 与警告，缺失目录返回 5 与可操作提示 | 部分验证：空/缺失目录已验证；带旧数据的真实迁移未验证（无旧数据样本） |
| DAT-02 | 日志落盘与级别 | bt-gui LogStore 单元测试与 GUI selftest；CLI 日志走 stderr | 日志落盘已验证；CLI 不污染 --json |

## 自检、平台与打包

| 编号 | 功能 | 验证方法 | 状态 |
| --- | --- | --- | --- |
| SYS-01 | 无硬件自检 | `bt selftest --json`；e2e `selftest_reports_checks`；真实后端无 BlueZ 时 transport/engine 标记 `skipped=true` | 已验证：mock 与真实后端均退出 0 |
| SYS-02 | 单实例与外部收件箱 | bt-platform 单元测试；e2e `second_instance_forwards_message_to_primary`；`single_instance_busy_maps_to_exit_3` 校验占用映射 3 | 单实例与载荷转发已验证；无显示环境未验证窗口激活 |
| SYS-03 | 托盘、通知、通知点击 | bt-platform 托盘与通知单元测试 | 调用路径已验证；无托盘宿主与通知守护进程，未验证实际显示 |
| SYS-04 | Windows 资源管理器集成 | NSIS 脚本静态检查 | 脚本覆盖注册表项；无 Windows 运行环境，未验证 |
| SYS-05 | Linux 桌面集成 | .desktop、AppRun 与打包脚本静态检查 | 文件存在且脚本可重复执行；未在桌面环境验证 |
| SYS-06 | GUI 页面与交互 | 设备、发送、队列、接收、记录、日志、设置 | 页面状态与动作已接线；无显示环境未验证真实交互 |
| SYS-07 | GUI selftest | `cargo run -p bt-gui -- --selftest` | 已验证：16 项检查退出 0；环境不满足项以 skipped 标记，不计入失败 |
| SYS-08 | Windows 安装器 | NSIS 构建与安装/卸载/升级 | 脚本曾用 NSIS 编译；未在 Windows 安装/卸载/升级 |
| SYS-09 | GitHub Actions 发布 | release.yml 静态检查 | 版本校验、矩阵与产物断言已接线；未在 GitHub 实际运行 |

## CLI 契约专项

| 编号 | 要求 | 验证方法 | 状态 |
| --- | --- | --- | --- |
| CLI-01 | 全部命令支持 `--json`，stdout 单对象 | e2e 各用例用 `serde_json::from_slice` 整体解析 | 已验证 |
| CLI-02 | 长任务 `--events` 输出 NDJSON，末行 result | e2e `send_events_are_ndjson_with_result_line`、`devices_scan_reports_devices_and_events` | 已验证 |
| CLI-03 | 退出码 0/1/2/3/4/5 | e2e `usage_errors_exit_2`、`send_errors_have_stable_codes_and_shapes`、detect 三态、`permission_errors_exit_4_with_permission_kind`；`exit_code_for` 单测覆盖全部错误分类 | 已验证（3 依赖真实占用未触发；4 用只读目录与导出目标实测） |
| CLI-04 | 诊断走 stderr，数据走 stdout | e2e 解析 stdout 为 JSON，stderr 不参与 | 已验证 |
| CLI-05 | 密码不入 argv/日志/JSON | e2e 密码用例 | 已验证 |
| CLI-06 | 隐藏兼容别名 | e2e `hidden_compat_aliases_work` | 已验证 |
| CLI-07 | `bt --version`、`--help`、逐命令 `--help` | e2e `help_and_version_are_available` | 已验证 |
| CLI-08 | btrecv 透明转发参数 | `btrecv --help`、无参监听、`btrecv recv` | 已验证 |
| CLI-09 | 默认人类可读输出 CJK 对齐 | 人工检查表格宽度 | 已验证（人工） |
| CLI-10 | 未知命令/缺参数退出 2，带 --json 或 --events 时输出错误对象 | e2e `usage_errors_exit_2`、`usage_errors_are_json_when_requested`、`events_parse_errors_emit_ndjson_error` | 已验证 |
| CLI-15 | REPL 内错误与帮助输出结构化对象 | e2e `repl_json_reports_errors_as_objects`、`repl_events_reports_errors_as_ndjson`、`repl_help_emits_structured_rows` | 已验证 |
| CLI-16 | 查询命令不触发 journal 续传 | `Command::auto_resumes_journal` 单测覆盖命令矩阵；bt-app `backend_can_open_without_auto_resuming_the_journal`、`backend_auto_resume_enqueues_journal_entries`；e2e `read_only_commands_do_not_touch_the_resume_journal` 与真实后端 `real_backend_resumes_only_for_transfer_commands` | 契约已验证（含真实 CLI 二进制的只读/发送对照） |
| CLI-11 | 数据根隔离 | `BT_HOME` 便携布局：配置与数据分居 `.config` 与 `.local/share` | 已验证 |
| CLI-12 | 模拟后端可用 | `BT_BACKEND=mock`，由 bt-app 装配层选择 MockBackend | 已验证 |
| CLI-13 | REPL 同一会话状态 | e2e 记录、配置、别名流程 | 已验证 |
| CLI-14 | cli-only 构建行为 | `cargo build -p bt-cli --no-default-features`；`bt gui` 退出 1，无子命令退出 2 并在 `--json`/`--events` 下输出结构化错误 | 已验证 |

## 体积指标

| 指标 | 上限 | 当前状态 |
| --- | --- | --- |
| cli-only 发布体积 | 8 MB | 已验证：`bt` 3,609,904 字节、`btrecv` 3,352,048 字节（本机 release，无 GUI 特性） |
| GUI 单文件 | 20 MB | 已验证：`bt-gui` 11,175,704 字节 |
| .deb | 20 MB | 已验证：重建实测约 5.67 MB（本次重建 5,670,756 字节，方法与波动说明见 docs/RELEASE.md） |
| NSIS 安装器 | 15 MB | 未生成：本机无 makensis 与 Windows 链接器 |
| 接收最终文件额外字节 | 0 | 已验证：bt-app e2e 断言落盘长度等于载荷 |
| 半成品 sidecar | 64 字节 | 已验证：53 字节 |
| journal 单条目 | 256 字节 | 已验证：典型 182 字节，最坏 253 字节 |
| 导出 JSONL 单行 | 250 字节 | 已验证：内容 205 字节 |
| 导出 CSV 单行 | 200 字节 | 已验证：表头 74 字节，数据 94 字节 |
| 配置全默认时文件为空 | 0 字节 | 已验证：全默认不落盘，单个非默认值 13 字节 |

## 未验证项清单

1. 真实蓝牙适配器上的扫描、配对、SDP、RFCOMM 与 OPP 互操作；无硬件与 BlueZ。
2. 真实助手通道发送、断线重连与 journal 续传；无对端接收端与硬件。
3. 真实接收链路上的 SHA-256 校验、重名策略、7 天半成品清理与路径攻击面；引擎单元测试已覆盖，真机未验证。
4. Windows WinRT 路径、Windows 安装器、资源管理器集成；无 Windows 环境。
5. GUI 窗口渲染与真实交互、托盘宿主面板与桌面通知；无显示环境，只验证了无窗口自检与平台调用路径。
6. 真实迁移旧 .NET 数据；迁移实现与空目录/缺失目录已验证，但缺少旧数据样本做端到端验证。
7. 退出码 3 的真实后端触发路径；mock 不跨进程 busy。
8. Ctrl+C 打断真实长传输；取消后的 journal 保留已用脚本化传输与 CLI e2e 验证，真实长传输未验证。
9. 真实断线后的续传：journal 的 list/run/cancel 与 abandon 规则已用 mock 后端和真实 BTJ1 文件验证；接收端半成品偏移与真实链路重连未验证。
10. 4 GiB 以上的真实大文件传输与磁盘满；4 GiB 边界与只读目录权限拒绝已有单元/e2e 验证。
