# 独立审查与验收报告

审查对象：`refactor/rust-rewrite` @ `2a2da9f`，基线 `4525848`，工作树干净。审查只读源码与构建产物，未修改任何源码、未执行 git 写命令；唯一写入为本文件。

## 1 审查范围与环境限制

审查覆盖：`docs/`、`README.md`、`CHANGELOG.md`、`skills/`、`crates/` 全部 9 个 crate 与 xtask、`packaging/`、`.github/workflows/`、`legacy/dotnet/` 归档。方法为独立运行构建、静态检查、测试、CLI 契约矩阵、GUI 自检、打包脚本、迁移夹具与静态代码审查。

本机环境限制，以下不是被测实现缺陷，但决定可验证边界：

| 条件 | 实测 |
| --- | --- |
| 蓝牙硬件 | 无，`/sys/class/bluetooth` 不存在 |
| BlueZ | 无 `bluetoothctl`/`bluetoothd`，系统 D-Bus 无 `org.bluez` |
| Windows | 无 Windows 运行环境；无 MSVC 链接器，`lib.exe` 不存在；无 wine |
| NSIS | PATH 无 `makensis`；`target/t3/nsis/root/usr/bin/makensis` 存在 NSIS 3.10 解包副本（gitignored） |
| AppImage | PATH 无 `appimagetool`；无法生成 AppImage |
| 显示 | 存在 WSLg 的 `DISPLAY=:0`/`WAYLAND_DISPLAY=wayland-0`，但 Mesa EGL 报错，无法人眼确认窗口 |
| 其他 | `dpkg-deb`、`python3` 可用；`sqlite3` CLI 不可用，迁移夹具用 Python 构造 |

默认 `target/` 构建缓存可用；fmt/clippy/test 命中源码未变化的缓存，测试用例本身重新执行。

## 2 独立复现结果表

| 项目 | 命令 | 退出码 | 关键结果 |
| --- | --- | --- | --- |
| 工具链 | `rustc -V` / `cargo -V` | 0 | rustc 1.96.0，cargo 1.96.0 |
| 格式化 | `cargo fmt --all --check` | 0 | 无输出 |
| 静态检查 | `cargo clippy --workspace --all-targets -- -D warnings` | 0 | 无警告 |
| 测试 | `cargo test --workspace` | 0 | 246 passed，0 failed，1 ignored；ignored 为 `tray::linux_runtime_tests::starts_and_owns_status_notifier_name`，原因“需要桌面会话的 D-Bus” |
| 发布构建 | `cargo build --workspace --release` | 0 | `bt` 11,676,568 B，`btrecv` 3,347,696 B，`bt-gui` 11,166,104 B |
| cli-only 发布 | `cargo build -p bt-cli --no-default-features --release` | 0 | `bt` 3,597,360 B，`btrecv` 3,345,136 B；`cargo tree` 无 bt-gui |
| cli-only 调试 | `cargo build -p bt-cli --no-default-features` | 0 | 无 GUI 依赖 |
| GUI 自检 | `cargo run -p bt-gui -- --selftest` | 0 | 16 项检查全 ok；读码确认构造 `UiState`、渲染 7 个页面、配对对话框、GUI zip、收件箱、PIN、日志，不是只打印 ok |
| Windows 检查 | `cargo check -p bt-transport/-p bt-platform/-p bt-engine --target x86_64-pc-windows-msvc` | 0 | 三者通过 |
| Windows 检查 | `-p bt-store/-p bt-app/-p bt-cli/-p bt-gui --target ...` | 101 | 全部因 `libsqlite3-sys` 的 C 构建脚本找不到 `lib.exe` 失败，不是 Rust 代码类型错误 |
| CLI 帮助 | `bt --help`、各子命令 `--help`、`btrecv --help/--version` | 0 | 命令/参数与 docs/CLI.md 一致；兼容别名不出现在帮助中 |
| CLI 用法错误 | 未知命令、缺地址、`--mode bogus`、缺 `--yes` | 2 | `--json` 时为单个 `type=error` 对象；无 `--json` 时 clap 错误在 stderr |
| devices | list/scan/pair/alias/favorite/detect | 0/0/0/0/0/0；detect 不存在设备为 5 | JSON 字段与 docs/CLI.md 一致；detect 三态 present=0 absent=1 unknown=5 的映射有单测 |
| send | file/files/text/folder/--zip/--mode assistant | 0 | job/summary 字段正确；assistant 模式 channel=assistant；缺失文件 2、目标不存在 5 |
| records/export | list/delete/clear/stats、csv/jsonl | 0；删除不存在 5；缺 --yes 2 | CSV 表头 74 B、数据行 94 B（不含换行）；JSONL 单行 198 B（“你好 hello”文本记录） |
| config | list/get/set、越界收敛、密码 | 0；get auth-password 2；位置密码 2 | `opp_chunk_size 999999` → 65023；mode=opp 新进程可见；`auth.token` 0600，config 不含密码 |
| 续传 | `resume list/run/cancel` | 0 | 真实 BTJ1 文件；过期 paused 条目 run 后清空并退出 1 且给警告，符合文档 |
| btrecv 监听 | 无参 btrecv，2 s 后 SIGINT | 0 | JSON `stopped:true, listening:false`；`bt recv --json` 与 `btrecv recv` 同样 0 |
| `--events` | send/devices/recv | 0 | NDJSON，末行 `type=result`；send 失败时末行 `type=error` |
| 真实后端 | `BT_BACKEND` 未设置，无 BlueZ | selftest 0；scan/send/recv 1 | 明确报 BlueZ ServiceUnknown，selftest 将 transport/engine 标为环境跳过 |
| 权限错误 | 只读 `BT_HOME` 下 `config set`/selftest/list | 1 | `kind:"io"`，不是文档承诺的 4/permission（见 R-03） |
| 迁移夹具 | 合成旧 config.json + data.db + send-journal.json | 0 | 导入 2 条记录、配置、1 条 journal；密码 0600；二次执行幂等；journal 过期条目按规则放弃 |
| 4 GiB zip | `bt send file ... --zip`，4 GiB+1 B 稀疏文件 | 0 | 33 s 后成功，生成 4,171,732 B zip；与文档“4 GiB 及以上会报错”矛盾（见 R-01） |
| NSIS 编译 | 解包 NSIS 3.10 + 未修改 installer.nsi + 占位载荷 | 0 | 生产文件名下无警告；产物 4,824,081 B；dist 无 setup.exe |
| .deb | 现有 dist 包 + 重新构建到 /tmp | 0 | 现有 5,554,490 B；重建 5,554,492 B；dpkg-deb 内容齐全 |
| 托盘 D-Bus | 手动运行 ignored 用例 `--ignored` | 0 | 1 passed，成功持有 `org.kde.StatusNotifierItem` |
| 归档 | `find legacy/dotnet -type f` | 0 | 124 个文件；旧 src/tests/tools 123 个 + sln 1 个，逐路径映射缺失 0 |
| 敏感信息 | `git grep` 绝对路径/密码 | 0 | 无本机绝对路径；无凭据入库；271 个 tracked 文件 |

## 3 需求自述与实测偏差表

| 实现方自述 | 实测 | 结论 |
| --- | --- | --- |
| 246 测试通过、1 ignored | 246 passed，1 ignored，ignored 原因明确 | 相符 |
| GUI selftest 16 项 | 16 项，退出 0；代码确实构造页面状态 | 相符 |
| cli-only bt 3,597,360 B、GUI 11,166,104 B、.deb 5,554,490 B、完整 bt 11,676,568 B | 全部复现；.deb 重建 5,554,492 B（±2 B 打包时间戳差异） | 相符 |
| 跨 crate 漂移已修、CLI 无重复 mock 分支 | workspace 构建/测试通过；mock 选择只在 bt-app 一处 | 相符 |
| `answer_pin` 回填 Linux RequestPinCode / Windows ProvidePin | 代码路径存在；bt-app e2e 断言 PIN 值到达 MockTransport；CLI/GUI 都走 answer_pin | 相符（真机未验证） |
| GUI 与 CLI 统一走 `bt_app::pack_zip_file_in` | 两侧均调用；GUI 单文件 zip、CLI --zip 路径一致 | 相符 |
| resume 切到真实 BTJ1 journal，`journal/send-journal.bin` | 实测 journal 文件、resume list/run/cancel、迁移导入均走 BTJ1 | 相符 |
| 文档与实现一致 | 存在多处不一致，见 R-04、R-11、R-15、R-16、R-19 等 | 不相符 |
| `opp_chunk_size` 全仓 65023 | 用户配置默认/上限/迁移/文档均为 65023；内部 `OppSettings/OppOptions::default` 仍为 32768（生产路径由 config 覆盖） | 基本相符，表述不严格 |
| NSIS 安装器未生成；T3 声称曾用 NSIS 3.10 编译通过 | dist 无 setup.exe，T5“未生成”准确；`packaging/` 内没有 NSIS 工具，但 gitignored `target/t3/nsis/root` 有 NSIS 3.10；用该工具编译未修改脚本成功（Linux 下需为 `${__FILEDIR__}\app.ico` 这个反斜杠文件名提供兼容文件，Windows 无此问题） | 矛盾已查清：T3 的编译声明可信，最终安装器未保留；安装/卸载行为仍未验证 |

## 4 缺陷清单

阻断级：无。

### 4.1 高

**R-01 `--zip` 对 4 GiB 及以上文件不报错，生成大小字段错误的 zip**
- 现象：`bt send file <ADDR> <4GiB+1B> --zip` 返回 0，生成 zip；文档承诺“4 GiB 及以上文件会报错”。
- 复现：`truncate -s 4294967297 big.bin` 后 `BT_BACKEND=mock bt send file AA:BB:CC:00:00:01 big.bin --zip --json`，33 s 后 exit 0，zip 4,171,732 B。
- 期望：在打包前按源文件大小与累计归档大小拒绝 `>=4 GiB`，或实现 ZIP64；至少不得返回成功。
- 证据：`crates/bt-engine/src/package.rs:266` 将 `uncompressed` 直接 `min(u32::MAX)`，偏移/大小全部 `u32`；`write_zip` 无大小检查（`package.rs:217`）；文档 `docs/CLI.md:196`。
- 建议：入口处检查单文件及文件夹总大小，超限返回 `invalid`；文档与测试同步。

**R-02 Ctrl+C 取消发送会删除 journal，与“保留续传信息”文档冲突**
- 现象：用户取消（Ctrl+C/`cancel_all_jobs`）时，助手通道任务走 `JournalFinal::Cancel`，`journal.cancel` 删除条目及自产载荷；随后 `resume list` 为空。
- 复现：代码路径审查；真实长传输本机不可复现（mock 无长任务），但分支确定。`send.rs:792-798`（canceled 分支）、`send.rs:825-837`（Err 分支）选择 `Cancel`；`journal.rs:698-700`。
- 期望：SND-10 与 Skill 承诺“任务 canceled，journal 保留，可 `resume list`/`resume run`”；应保留条目为 paused/pending，或正式修改验收与 Skill 并给出替代恢复流程。
- 证据：`docs/ACCEPTANCE.md:52`、`skills/bluetooth-transfer/SKILL.md:163`。
- 建议：取消与失败区分；用户取消保留 journal（载荷也保留或按 abandon 规则处理），并补一条 e2e 断言。

### 4.2 中

**R-03 权限错误实际退出 1/kind=io，文档承诺 4/permission**
- 现象：只读数据目录下 `bt config set mode opp --json`、`bt selftest --json`、`bt devices list --json` 均 exit 1，`error.kind="io"`。
- 期望：`docs/CLI.md:26` 承诺“配置、数据、接收目录不可写”为 4；应为 `permission` 或修改文档/验收（CLI-03）。
- 证据：`crates/bt-core/src/error.rs:114-118` 把所有 `io::Error` 映射为 `Io`；`bt-cli/src/error.rs:62` 只有 `Permission` 才映射 4。
- 建议：`From<io::Error>` 识别 `PermissionDenied` → `Permission`；接收目录/配置写入路径补测试。

**R-04 声明 MSRV 1.85，实际依赖要求 1.95**
- 现象：README/ARCH 写“系统 Rust 1.85 及以上可直接构建”，`Cargo.toml` `rust-version = "1.85"`；但 `eframe/egui 0.37.0` 的 `rust-version = "1.95"`，`notify-rust 4.19.0` 为 1.89，`image` 为 1.88。1.85–1.94 用户会构建失败。
- 期望：`rust-version` 与文档改为实际最低版本（当前 1.95），或在 CI 加 MSRV 任务。
- 证据：`Cargo.toml:9`、`README.md:53/76`、`docs/ARCHITECTURE.md:205`；依赖元数据在仓库内工具链 registry 中实测。
- 建议：提升 `rust-version` 与 `clippy.toml` 的 `msrv`，并在发布说明标注。

**R-05 `--events` 解析失败与 REPL 内错误不输出 JSON/NDJSON**
- 现象：`bt frobnicate --events` exit 2，stdout 为空（clap 错误只在 stderr）；`bt repl --json` 输入非法命令、`gui`/`recv`/`repl` 等被禁命令时也只写 stderr，不产生“每行一个 JSON 对象”。
- 期望：`--events` 失败时最后一行 `type=error`；REPL `--json` 每行命令输出一个 JSON 对象（包括错误）。
- 证据：`crates/bt-cli/src/lib.rs:25` 只检测 `--json`，`lib.rs:137-166` 仅 JSON 模式输出对象；`crates/bt-cli/src/commands/repl.rs:51-56` 直接 `eprintln`。文档 `docs/CLI.md:20-21` 与 REPL 小节。
- 建议：clap 失败在 Events 模式输出 NDJSON error；REPL 的解析/禁用错误走 `emit_result` 或统一 error 输出。

**R-06 journal 租约续租未实现，文档声称每分钟续租**
- 现象：`journal.rs` 定义 `RENEW=60s` 与 `renew()`，但发送路径从不调用 `renew`/`mark_progress`；条目的 `resume_offset` 也从不更新。
- 影响：Linux/Windows 的进程存活判定可靠，实际影响小；但在存活判定不可靠的平台上会退化为 6 分钟租约，长传输可能被其他进程视为可接管，与文档承诺不符。
- 证据：`crates/bt-engine/src/journal.rs:15/638/683`；`crates/bt-engine/src/send.rs` 无 renew 调用；`docs/PROTOCOL.md:172`。
- 建议：发送线程每分钟调用 `renew` 并周期 `mark_progress`，或删除 `RENEW` 并修订文档为“进程存活判定优先”。

**R-25 只读 CLI 命令会通过后端自动恢复 journal 并触发发送**
- 现象：除 `resume` 外的 CLI 命令都用 `Context::open`（auto_resume=true）；`RealBackend::build` 在有 transport 时会调用 `engine.resume_pending()`，因此 `bt devices list`/`records list`/`export`/`config set` 也可能连接设备并开始续传，命令结束再 `shutdown` 取消。
- 期望：查询类/配置类命令不应触发发送；续传应由 `resume run` 或显式开关触发。
- 复现：在有可用 transport 且 journal 有条目的环境执行 `bt devices list`，观察后端自动入队/连接；本机无 BlueZ，只能以代码路径确认。
- 证据：`crates/bt-cli/src/lib.rs:48-52`；`crates/bt-app/src/backend.rs:183-191`；`crates/bt-engine/src/send.rs:372-408`。
- 建议：只读命令改用 `open_without_resume`，或增加 `--resume` 显式选项；GUI 启动自动恢复可保留。

### 4.3 低

本节各条的复现方式为按证据中的文件行号检查代码，或执行该条描述的命令；均为文档、交互或边缘行为，不另行列出长步骤。

**R-07 通知点击未实现**
- 现象：`Notifier` 只有 `notify`，没有 action/click 回调；ACCEPTANCE SYS-03 写“通知点击”。
- 证据：`crates/bt-platform/src/notify.rs:37-58`；`docs/ACCEPTANCE.md:99`。
- 建议：实现点击后置前窗口/打开记录，或从验收中删去“通知点击”。

**R-08 GUI selftest 的字体检查恒为 ok**
- 现象：`report.push("font", true, font.detail.clone())` 无论是否找到 CJK 字体都通过；无字体环境仍显示整体通过。
- 证据：`crates/bt-gui/src/selftest.rs:99`。
- 建议：使用 `font.found()`；找不到字体时给出失败或显式环境跳过项。

**R-09 记录页缺少文档声称的时间筛选**
- 现象：`docs/ARCHITECTURE.md:175` 写“按方向、通道、状态、设备、时间与关键字筛选”；GUI 筛选结构只有 direction/kind/status/channel/device/search，`record_query` 的 from/to 恒为 None。
- 证据：`crates/bt-gui/src/state.rs:152-162`、`state.rs:756-774`。
- 建议：补时间范围控件，或删除文档中的“时间”。

**R-10 OPP 任务暂停后 `resumable=true`，但 OPP 无 journal 续传**
- 现象：暂停态的终态被硬编码 `resumable=true`，GUI 显示“支持断点续传”；继续时从 0 重发。
- 证据：`crates/bt-engine/src/send.rs:475-481`；OPP 无 journal 写入。
- 建议：按通道/实际 journal 状态设置 resumable。

**R-11 zip 文档写 STORE，实现使用 Deflate**
- 现象：`docs/CLI.md:196` 称“写成 STORE zip”；`package.rs` 使用 `DeflateEncoder`（method 8）。
- 建议：改文档为“Deflate”，或实现 STORE；同时明确目录包也使用同一压缩方式。

**R-12 `copied_files` 永远为 0 但仍出现在 migrate JSON**
- 现象：迁移报告字段保留，但实现从不上调；CLI/GUI 都展示该字段。
- 证据：`crates/bt-api/src/dto.rs:459`；`bt-app/src/backend.rs` 迁移路径无赋值。
- 建议：实现文件复制计数，或移除字段并改文档。

**R-13 数据目录/记录库/日志默认权限为 0755/0644，仅密码 0600**
- 现象：`$BT_HOME/.local/share/BluetoothTransfer/records.db` 0644，目录 0755；同机其他用户可读记录与日志。
- 证据：`crates/bt-app/src/paths.rs:78-92` 用默认 umask；实测 `-rw-r--r-- records.db`；`auth.token` 为 0600。
- 建议：数据目录与 DB/日志设置 0700/0600（Unix），或在文档明确数据可见性。

**R-14 `docs/RELEASE.md` 的测试数量与实际不符**
- 现象：写“bt-gui 10 项”，实际 `cargo test -p bt-gui` 为 8 passed；bt-platform 22 项相符（另有 1 ignored）。
- 证据：`docs/RELEASE.md:107`；实测输出。
- 建议：同步为 8 并注明 ignored 项。

**R-15 发布工作流存在产物与门禁瑕疵**
- `workflow_dispatch` 的 `skip_smoke` 输入声明后从未使用；`release.yml` 不做 fmt/clippy/test；Windows/Linux 的 `dist/gui`、`dist/cli` 暂存目录随 artifact 上传，publish 的 `find artifacts -type f` 会把暂存文件算进 combined 校验和；手动 dispatch 可在非 tag 上创建 `v2.0.0` release。
- 证据：`.github/workflows/release.yml:6-11`、`:100-107`、`:162-182`。
- 建议：删除无用输入；发布前复用质量门禁；上传前清理暂存目录；dispatch 强制校验 tag。

**R-16 文档标题包含括号**
- 现象：`docs/RELEASE.md:137` 标题“Windows 安装器（Windows runner）”使用括号，违反仓库标题规范。
- 建议：改为无括号标题。

**R-17 CLI 没有任务队列查询/取消命令**
- 现象：T4 任务书要求“queue 查询与取消”，最终 CLI 只有 send 等待与 resume，没有 `bt queue`；GUI 有队列页。CLI.md/ACCEPTANCE 未声明该能力。
- 建议：补命令或明确从验收范围移除。

**R-18 bt-core 存在文件系统调用，与架构声明不符**
- 现象：ARCH 称“bt-core 不做文件系统与网络调用”，但 `sha256_file` 直接读文件。
- 证据：`docs/ARCHITECTURE.md:43`；`crates/bt-core/src/hash.rs:1/19`。
- 建议：把文件哈希移到 bt-engine/bt-store，或修订架构说明。

**R-19 detect 的 unknown 结果通过 `type=result` 输出，与“失败为 type=error”表述冲突**
- 现象：`bt devices detect <不存在> --events` 最后一行 `type=result` 且 `ok:false`，不是 `type=error`。
- 证据：`docs/CLI.md:20-21`；实测输出。
- 建议：明确三态结果不算错误并修改文档，或改为 error。

**R-20 GUI 与 CLI 的多文件 `--zip` 语义不同**
- 现象：CLI 对每个文件分别打 zip；GUI 在多项时提示并放弃 zip（按原样发送）。
- 证据：`crates/bt-cli/src/commands/send.rs:34-40`、`crates/bt-gui/src/state/actions.rs:312-315`。
- 建议：统一为同一行为并写入文档。

**R-21 cli-only 构建下 `bt --json` 无子命令输出人类文本**
- 现象：`bt --json` exit 2，stderr 人类提示，stdout 无 JSON；同构建 `bt gui --json` 则输出 JSON error。
- 证据：`crates/bt-cli/src/lib.rs:109-122`。
- 建议：无子命令路径也走 `CliError::report`，保持 JSON 契约。

**R-22 NSIS 脚本版本回退为硬编码**
- 现象：脚本 `!define VERSION "2.0.0"`；CI 虽传 `/DVERSION`，但手工编译或 CI 参数遗漏时不会读 workspace 版本，与“单一版本来源”声明不完全一致。
- 证据：`packaging/windows/installer.nsi:8-10`；`docs/ARCHITECTURE.md:166`。
- 建议：由构建脚本生成 define，或删除硬编码并在缺失时失败。

**R-23 Linux 单实例 socket 读取无上限**
- 现象：接收线程 `read_to_string` 无长度上限，同机同用户进程可发送超大载荷占用内存；socket 名含 uid 但无 SO_PEERCRED 校验。
- 证据：`crates/bt-platform/src/single_instance.rs:176`。
- 建议：限制消息大小并校验 peer uid；与外部收件箱的 1 MB 上限保持一致。

**R-24 其他文档与实现小偏差**
- `docs/RELEASE.md:112` 的 NSIS 说法已可由本次编译证实（不再算缺陷，但“安装/卸载证据”确实缺失）。
- `README.md:146` “配置全默认时文件大小 0 字节”，实际是不创建文件；语义可接受但建议统一为“不落盘”。
- GUI 页面清单（ARCH 173-175/SYS-06）未单列队列页，而实际 7 页含队列。
- `docs/ACCEPTANCE.md:90` 的 DAT-01 在本次合成夹具下已能验证，建议从“未验证”升级为“夹具验证”。

## 5 未验证项清单

| 编号 | 未验证内容 | 原因 | 建议验证环境 |
| --- | --- | --- | --- |
| U-01 | 真实扫描、配对、SDP、RFCOMM、OPP 互操作 | 无蓝牙硬件、无 BlueZ | 有适配器的 Linux 主机 + BlueZ 5.x + Android/Windows 对端，记录型号/系统/步骤 |
| U-02 | 真实助手通道发送、断线重连、接收端 SHA/重名/半成品续传 | 无对端与硬件 | 两台装有本应用的 Linux/Windows 设备 |
| U-03 | Windows WinRT 发现/配对/RFCOMM/服务广播 | 无 Windows 环境；`bt-store/app/gui/cli` 的 Windows check 被 `lib.exe` 阻断 | Windows 10/11 x64 + MSVC Build Tools |
| U-04 | NSIS 安装/升级/静默安装/卸载/右键菜单/SendTo/自启 | 只完成脚本编译（NSIS 3.10，占位载荷），未安装 | Windows 10/11 虚拟机 |
| U-05 | GUI 窗口渲染、真实交互、托盘宿主面板、通知显示/点击、剪贴板、文件对话框 | 无法人眼确认；EGL 报错；无通知守护/托盘宿主 | 有桌面会话的 Linux 与 Windows 机器 |
| U-06 | 真实 Ctrl+C 打断长传输并 `resume` | 无长传输与硬件 | 大文件 + 真实对端；先验证 R-02 修复 |
| U-07 | 接收询问交互、并发 Busy、7 天清理、磁盘满、权限拒绝 | mock 无真实传输；无第二连接 | 真机 + 可制造磁盘满/只读目录的环境 |
| U-08 | 退出码 3/4 的真实触发 | mock 不跨进程 busy；4 在本机被证实走 1（R-03） | 真实接收占用与只读目录 |
| U-09 | AppImage 构建与运行 | 无 `appimagetool` | 安装 FUSE 的桌面 Linux 或 CI |
| U-10 | GitHub Actions 双平台工作流、产物命名/断言 | 未在 GitHub 运行 | 实际 tag 发布会话 |
| U-11 | 与旧 .NET btrecv 跨实现互传 | 无旧版运行环境；只做字节布局比对 | Windows 旧版 btrecv + 本机新发送端 |
| U-12 | 真实旧数据迁移（非合成夹具） | 无旧版用户数据样本 | 从旧 .NET 版本升级的 Windows 用户目录 |
| U-13 | .deb 实际安装/卸载/桌面缓存刷新 | 无 root；未安装 | Debian/Ubuntu 测试机 |
| U-14 | 大文件与 4 GiB 以上 OPP/助手传输 | 蓝牙耗时与空间不允许 | 真实对端与足够空间，先处理 R-01 |
| U-15 | 无 CJK 字体环境的 GUI 显示 | 本机有 msyh.ttc；selftest 字体检查恒真（R-08） | 最小化 Linux 容器/桌面 |

## 6 放行结论

**有条件放行。** 未发现阻断级缺陷：核心协议字节布局与旧 .NET 源码逐字段一致，fmt/clippy/test/构建/CLI 契约/GUI 自检在本机全部通过，体积指标与文档声明一致，journal/迁移/续传/接收路径有实现与测试支撑。

发布前必须处理：

1. R-01：为 `--zip` 增加 4 GiB 上限检查（或 ZIP64），禁止静默生成无效归档。
2. R-02：实现“取消保留 journal”或正式修改 SND-10 与 SKILL；当前文档工作流不可执行。
3. R-03：把权限错误映射到 exit 4/`permission`，或修改退出码文档。
4. R-04：修正 MSRV 元数据与构建说明（实际最低 1.95）。
5. R-05：补齐 `--events` 与 REPL 的机器可读错误输出，或修改 CLI 契约。
6. R-06：实现 journal 租约续租，或把文档改为“进程存活判定优先”。
7. R-25：避免只读 CLI 命令隐式触发 journal 续传，或明确记录该行为。

其余低风险项（R-07 至 R-24）建议在 2.0.0 发布前同步修订文档，不单独阻断发布。真实蓝牙、Windows 安装器、GUI 渲染与 AppImage 仍属未验证边界，不能以本报告替代真机验收。

## 7 T8 复审

复审对象：HEAD `f52ed9c`（修复提交 f52ed9c、ddb244a、4e230ce、c5a4838、e23ab6e、b0adfdd；相对 T6 基线 `2a2da9f` 共 6 个提交，32 个文件、+1126/-138 行）。工作树仅 `?? docs/REVIEW.md`。本轮未修改源码，未执行 git 写命令。

复审方法：重跑门禁；用稀疏文件亲自构造 4 GiB 边界；在 /tmp 用仓库源码另建独立 harness 驱动 `PendingPackageStore`、`RealBackend + MockTransport`、journal seed，避免只运行实现方新增测试；无法用 CLI 进程复现的路径（无慢速传输、无 BlueZ）以等价入口加代码路径确认，并明确标注。

### 7.1 修复项结论

| 编号 | 结论 | 独立证据与备注 |
| --- | --- | --- |
| R-01 | 已修复 | 单文件 4 GiB-1：CLI `--zip` exit 0，33 s，pending 无残留；4 GiB：exit 1、kind=unsupported、提示含 4 GiB、pending 0。文件夹 4 GiB-1：独立 harness 打包成功，Python 解析中央目录 `file_size=4294967295`、`method=8`、CRC=0（已另算 4 GiB-1 零字节 CRC 确为 0），zip 4,171,732 B；文件夹 4 GiB 与 3 GiB+1 GiB 累计均拒绝，pending 0。 |
| R-02 | 部分修复 | 助手通道文本取消：独立 harness 触发 `cancel_all_jobs` 后 job=canceled，journal 1 条 paused，payload 存在；`resume list` 可见；`resume run`（mock 完成）resumed=1、completed=1，journal 与 pending 清空；`resume cancel` 对自产载荷同样清理。**但 CLI/GUI 单文件 zip 路径在取消时仍删除自产 zip**，见 R-26。 |
| R-03 | 已修复 | 只读 `BT_HOME` 下 config set/devices list/selftest exit 4、kind=permission；只读导出目标 exit 4；只读 SQLite 数据目录 exit 4（记录库不可写）；持锁 15 s 的 journal 锁使 `resume list` 等待 5 s 后 exit 4、kind=locked。`exit_code_for` 单测覆盖全部错误分类。 |
| R-04 | 已修复声明与接线，未本地实测 1.95 | `Cargo.toml`/clippy.toml/README/ARCH 均改 1.95；`cargo xtask msrv` exit 0，输出 workspace=1.95、依赖最高要求=1.95；`cargo metadata` 实测依赖 max 1.95（egui/eframe 0.37）；CI 增加 quality 步骤与独立 MSRV 作业。本机只装了 1.96，`rustup run 1.95.0` 报未安装；1.95 下载尝试在本环境停滞并已取消，故 1.95 实际编译未验证。 |
| R-05 | 部分修复 | `bt frobnicate --json/--events`、缺子命令、`btrecv --bogus` 均输出契约内错误对象/NDJSON 行并 exit 2；REPL `--json` 下未知命令、`gui`/`recv`/`repl`、`config set auth-password` 均输出错误对象。**REPL 的 `help`/`?` 仍只写 stderr，不输出 JSON**，见 R-28。 |
| R-06 | 已修复，文档需限定 | `Journal::heartbeat` 一次加锁续租并更新 resume_offset；发送进度回调按 60 s 节流调用；journal 与 LeaseHeartbeat 两个测试均使用可控时钟并覆盖续租与暂停不续租。心跳只在进度回调上触发，见 R-29。 |
| R-25 | 已修复（代码/单测） | `Command::auto_resumes_journal` 只放行 send/recv/gui 与隐藏发送别名，其余走 `open_without_resume`；独立 seed 后在 devices/records/config/export/selftest/migrate dry/resume list 下 journal 字节 sha256 完全不变，`resume run` 作为正对照会改变并清理。e2e `read_only_commands...` 使用 mock，未真正覆盖 CLI auto_resume，见 R-30。 |

### 7.2 全量门禁与体积复测

| 项目 | 命令 | 退出码 | 结果 |
| --- | --- | --- | --- |
| 格式化 | `cargo fmt --all --check` | 0 | 无输出 |
| 静态检查 | `cargo clippy --workspace --all-targets -- -D warnings` | 0 | 无警告 |
| 全量测试 | `cargo test --workspace` | 0 | 273 passed、0 failed、1 ignored；较 T6 的 246/1 增加 27 个测试，ignored 仍为托盘 D-Bus |
| CLI e2e | `cargo test -p bt-cli` | 0 | 单测 12、e2e 38、pair_pin 3；新增 zip 4 GiB、取消续传、权限、events/REPL、只读 journal 用例均通过 |
| 发布构建 | `cargo build --workspace --release` | 0 | 完整 `bt` 11,682,712 B、`bt-gui` 11,171,480 B、完整 `btrecv` 3,352,432 B |
| cli-only 发布 | `cargo build -p bt-cli --no-default-features --release` | 0 | `bt` 3,602,864 B、`btrecv` 3,349,872 B |
| cli-only 调试 | `cargo build -p bt-cli --no-default-features` | 0 | 通过 |
| GUI 自检 | `cargo run -p bt-gui -- --selftest` | 0 | 16 项全 ok |
| MSRV | `cargo xtask msrv` | 0 | workspace=1.95，依赖最高要求=1.95 |
| Windows target | `cargo check -p bt-transport/-platform/-engine --target x86_64-pc-windows-msvc` | 0 | 三者通过；store/app/cli/gui 仍 101（lib.exe 缺失），与 T6 相同 |

体积对照：

| 产物 | T7/文档声明 | 本次实测 | 结论 |
| --- | --- | --- | --- |
| cli-only `bt` | 3,602,864 | 3,602,864 | 相符 |
| cli-only `btrecv` | 3,349,872 | 3,349,872 | 相符 |
| `bt-gui` | 11,171,480 | 11,171,480 | 相符 |
| 完整 `bt` | 11,682,712 | 11,682,712 | 相符 |
| `.deb` | 5,554,490 | 重建 5,576,046；dist 旧包 5,554,490 | **不一致**，见 R-27 |
| NSIS | 未生成 | 未生成；脚本未改，T6 的 3.10 编译证据仍有效 | 未验证安装行为 |

### 7.3 新发现与剩余缺陷

**R-26 中 必修：CLI/GUI 单文件 zip 取消后仍删除自产 zip，R-02 的续传承诺在该路径不成立**
- 现象：`bt send file <ADDR> <PATH> --zip` 在 Ctrl+C/取消后，journal 条目已 paused，但 `OwnedFiles` 的 Drop 无条件删除 pending 下的 zip；GUI 的 `temp_zip_jobs` 在 `JobState::Canceled`（is_terminal）时由 `cleanup_job_zip` 删除。随后 `resume run` 因载荷不存在把条目作为 abandoned 清除（实测缺失载荷时 exit 1、warning“待发送文件已不存在”），文档承诺的“保留自产载荷、resume run 可继续”不成立。
- 复现/证据：`crates/bt-cli/src/commands/send.rs:34-61`（`file()` 在 `send_items` 后 drop `owned`）、`:107-123`（zip 产物进入 owned）；`crates/bt-gui/src/state.rs:507` 与 `:585-589`、`crates/bt-gui/src/state/actions.rs:346`；`crates/bt-api/src/dto.rs:67-69`（Canceled 是终态）。本机无慢速传输，未能做进程级 SIGINT 复现；上述分支为确定性代码路径。对比：文本/文件夹/裸文件由引擎持有载荷，独立 harness 已验。
- 期望：取消（以及 paused→cancel）时保留 zip；只有完成、显式 `resume cancel` 或 abandon 规则才删除。GUI 的 `cleanup_job_zip` 至少应对 Canceled 保留。
- 建议：让 `send_items` 返回终态摘要，`OwnedFiles` 仅在非取消时删除；GUI 按 Canceled/Paused 跳过 zip 清理；补 `send file --zip` 的 gate 取消 e2e（或 bt-app 层同路径）。
- 影响：R-02 的 `ACCEPTANCE.md:52`“契约已验证”和 `CLI.md:201`、`ARCHITECTURE.md:129` 对 zip 路径过度声明；不是用户源文件丢失，但续传工作流不可用。

**R-27 低：`.deb` 体积/产物未随修复更新**
- 现象：README:153、ARCHITECTURE:114、ACCEPTANCE:134、RELEASE:88/113 仍写 5,554,490 B；用当前 release 二进制重建为 5,576,046 B；`dist/` 中现有包仍是修复前二进制。
- 建议：重建 dist 包并同步四处文档；CI 体积断言仍远低于 20 MB，不影响上限。

**R-28 低：REPL `help`/`?` 违反新增的“每行一个对象”契约**
- 现象：`printf 'help\nquit\n' | bt repl --json` 只在 stderr 输出人类帮助，stdout 无对象；`CLI.md:14` 写“REPL 内每条非空命令输出一个对象”。
- 证据：`crates/bt-cli/src/commands/repl.rs:45-47`；实测。
- 建议：`help`/`?` 在 JSON/Events 模式输出对象（或明确排除并改文档）；`quit` 同样按控制命令处理。

**R-29 低：R-06 文档对“无进度”场景过于绝对**
- 现象：`PROTOCOL.md:172`、`ARCHITECTURE.md:128` 称“长传输不会因租约过期被接管”，但 heartbeat 只在进度回调触发；若一次进度更新之间超过 6 分钟（例如把 `opp_send_idle_timeout_s` 调到 >360 s 且链路停顿），存活判定不可靠的平台仍会按租约过期。Linux/Windows 的进程存活判定优先，实际风险低；默认 idle 300 s 通常先超时。
- 建议：文档改为“有进度时每 60 s 续租；无进度超过 6 分钟且无法判定进程存活时依赖租约”，或在心跳线程中加入定时续租。

**R-30 低：R-25 的 CLI e2e 用 mock，未覆盖真实 auto_resume 分支**
- 现象：`crates/bt-cli/tests/e2e.rs:39` 的 `Env` 固定 `BT_BACKEND=mock`；`read_only_commands_do_not_touch_the_resume_journal`（:935）在 mock 下运行，即使 `auto_resumes_journal` 选错也不会触碰 journal。ACCEPTANCE:122 把它列为 CLI-16 证据。
- 补强：该场景的代码/单元矩阵测试通过（`Command::auto_resumes_journal`），本复审也在真实后端（无 transport）下做了 journal 字节比对；但真实 auto_resume 分支仍缺少 CLI 级 e2e，建议在可注入 transport 的 bt-app e2e 中补一条“查询命令不产生 job”的断言，或删掉 e2e 的过度声明。

**R-31 低：T6 低风险项多数仍未处理**
- 仍存在：R-07 通知点击无实现（notify.rs 未变）；R-08 字体检查恒 ok；R-09 记录时间筛选缺失；R-10 OPP 暂停仍标 resumable；R-12 `copied_files` 恒 0；R-13 数据目录/记录默认权限；R-14 RELEASE 仍写 bt-gui 10 项（实测 8）；R-15 发布工作流暂存目录/门禁；R-16 RELEASE 标题仍含括号；R-17 CLI 无 queue 命令；R-18 bt-core 仍做文件系统哈希；R-19 detect unknown 仍 `type=result`；R-20 GUI/CLI 多文件 zip 语义仍不同；R-21 cli-only `bt --json` 无子命令仍人类文本；R-22 NSIS 版本硬编码；R-23 单实例 socket 读取无上限。均为低风险，不阻断本次放行；R-11 的 STORE 文档已改 Deflate，视为解决。

接受理由（T9 追加）：R-08 已在 selftest 中显式区分环境跳过，R-09 已删除文档中的时间筛选声明，R-14、R-16、R-19、R-21、R-22、R-23 的读取上限与 R-24 的文档项已修；剩余项接受现状。R-07 需要桌面通知宿主与 action 回调，当前环境无法验证点击路径；R-10 的 `resumable` 表示任务可继续而非断点续传，改字段会波及队列、记录与 GUI 契约，留待独立变更；R-12 字段为旧迁移报告兼容保留；R-13 的目录权限受 umask 与既有目录影响，需要迁移策略；R-15 发布工作流无法在本机运行验证；R-17 队列命令超出 2.0.0 CLI 契约；R-18 文件哈希迁移会扩大改动面；R-20 已在 ARCHITECTURE 明确 GUI 多文件行为；R-23 的 peer uid 校验受 std 稳定 API 限制，暂只加 1 MiB 读取上限。

**R-32 低：`send folder` 的 zip 超限错误形态与文档不一致**
- 现象：`docs/CLI.md:196-201` 称 zip 超限“退出码 1、error.kind=unsupported、error.hint 给出建议”；但 `send folder` 的打包在引擎 `build_payloads` 中，失败被收成 `jobs[].error` 字符串（`crates/bt-engine/src/send.rs:653-712`、CLI `wait_jobs` 的 SendResult），没有命令级 `type=error`/`error.kind`；只有 CLI 自己预打包的 `send file --zip` 才有该结构。真实 transport 下未复现（本机无 BlueZ），由代码路径确认。
- 建议：让 `send folder` 也在 CLI 预打包以统一错误，或把文档限定为“单文件 --zip”。

### 7.4 未验证项

1. Rust 1.95 实际编译：本机只有 1.96，1.95 下载在本环境停滞；CI `msrv` 作业已接线但未实际运行。需在 CI 或装有 1.95 的机器执行 `cargo check --workspace --all-targets`。
2. 真实蓝牙扫描/配对/SDP/RFCOMM/OPP/助手通道；无硬件、无 BlueZ。
3. 真实长传输的 CLI SIGINT 与 zip 取消（R-26）：无慢速 transport；只能以等价 harness 与代码路径确认。
4. `resume run` 通过真实 engine/transport 完成；本次仅验证 journal 计划 + mock 完成，以及引擎取消侧。
5. Windows WinRT、NSIS 安装/升级/卸载、GUI 渲染/托盘/通知、AppImage、GitHub Actions 双平台发布：环境缺失，未验证。
6. `.deb` 实际安装/卸载；本次只做 dpkg-deb 内容与体积检查。
7. 真实旧 .NET 数据迁移与跨实现互传；未变。
8. 4 GiB 以上实际蓝牙传输、磁盘满；未变。

### 7.5 最终结论

**有条件放行。** T7 对 R-01、R-03、R-04、R-05（核心错误路径）、R-06、R-25 的修复经独立复现成立；R-02 的引擎/journal 语义也已修复。剩余必修项只有 **R-26**：CLI/GUI 单文件 zip 取消后仍删除自产 zip，使“取消后 resume run”在该路径不可用，且与 ACCEPTANCE/CLI/ARCH 的声明冲突。R-27 至 R-32 为低风险文档、测试覆盖与残留项，建议在发布前同步，不单独阻断；MSRV 1.95 与真机边界仍需在对应环境实测，不能以本次本机结果替代。

## 8 T10 终审

终审对象：HEAD f206157（T9 提交 b0adfdd..f206157，共 11 个提交；相对 T8 的 f52ed9c 为 36 个文件、+2027/-176 行）。工作树干净，docs/REVIEW.md 已入库。本轮未改源码、未做 git 写操作；只追加本章。

### 8.1 修复项逐条结论

| 编号 | 结论 | 独立验证要点 |
| --- | --- | --- |
| R-26 | 部分修复：主链路通过，仍有 R-33 源文件边界 | 真实进程 SIGINT（BT_TEST_TRANSPORT=stall）实测 exit 1、summary.canceled=1、job=canceled、journal=paused、自产 zip 138 B 保留、用户源文件 18 B 保留；显式 resume cancel 删除 zip、保留源文件。脚本化 e2e zip_cancel_keeps_payload_and_resume_run_finishes_it 完成“取消→paused→resume run 完成→journal/payload 清理→source 保留→显式 cancel 清理”；GUI canceled_zip_job_keeps_payload_for_resume 通过。但源文件若位于 pending，会在完成后被 journal cleaner 删除，见 R-33；损坏 journal 下 settle 不保守，见 R-34。 |
| R-27 | 部分修复（低） | 文档改为约 5.60 MB 并给出重建区间 5,603,710–5,603,912；本次两次重建 5,603,926 与 5,603,842，dist 旧包 5,603,884。区间未覆盖 5,603,926（+14 B），见 R-36。 |
| R-28 | 已修复 | help、?、:help 在 repl --json 下输出 command=repl.help、ok=true、9 条 commands；--events 下输出 type=result；human 模式仍写 stderr，quit/exit 不输出对象；与 CLI.md 一致。 |
| R-29 | 已修复 | PROTOCOL、ARCHITECTURE、CHANGELOG 明确“仅进度回调续租；Linux/Windows 进程存活兜底；其他平台 6 分钟无进度可能按租约接管；默认 300 s idle 先超时”。代码 SystemLiveness::reliable 仅 Linux/Windows，与实际一致。 |
| R-30 | 已修复 | 调试构建 BT_TEST_TRANSPORT=mock 使 selftest transport=“通道 0”、engine 已装配；同一变量在 release 中 transport=“未启用（环境跳过）”，证明 release 空实现。真实 CLI e2e real_backend_resumes_only_for_transfer_commands 通过；手工以 debug 二进制验证 devices list 不改 journal、send 改变 journal。 |
| R-32 | 已修复 | release 二进制 bt send folder 对 4 GiB 单文件与 3 GiB+1 GiB 累计均 exit 1、type=error、kind=unsupported、hint 含“拆分/4 GiB”，pending 0，源文件完整。 |
| R-31 | 部分达成 | R-14 的 bt-gui 数量已改 9（实测 8 单测 + 1 集成）；R-16 标题已改；R-19/R-20/R-21/R-22/R-23/R-24 已修或已限定；R-22 无 VERSION 时 NSIS exit 1、有 VERSION 编译成功；R-08 仍是 ok=true + “环境跳过”文本，见 R-37；R-07/R-10/R-12/R-13/R-15/R-17/R-18 按 REVIEW 接受理由保留。 |

### 8.2 全量门禁与体积复测

| 项目 | 命令 | 退出码 | 结果 |
| --- | --- | --- | --- |
| 格式化 | cargo fmt --all --check | 0 | 无输出 |
| 静态检查 | cargo clippy --workspace --all-targets -- -D warnings | 0 | 无警告 |
| 全量测试 | cargo test --workspace | 0 | 283 passed、0 failed、1 ignored；较 T8 的 273/1 增加 10 个测试 |
| CLI e2e | cargo test -p bt-cli | 0 | e2e 43、单测 12、pair_pin 3；含真实 SIGINT、zip 取消续传、真实后端 auto_resume、权限用例 |
| 发布构建 | cargo build --workspace --release | 0 | 完整 bt 11,687,448 B、bt-gui 11,173,528 B、完整 btrecv 3,352,304 B |
| cli-only | cargo build -p bt-cli --no-default-features --release | 0 | bt 3,607,088 B、btrecv 3,350,000 B；调试构建亦通过 |
| GUI 自检 | cargo run -p bt-gui -- --selftest | 0 | 16 项全 ok |
| MSRV | cargo xtask msrv | 0 | workspace=1.95，依赖最高要求=1.95 |
| Windows target | cargo check -p bt-transport/-platform/-engine --target x86_64-pc-windows-msvc | 0 | 三者通过；store/app/cli/gui 仍 101（lib.exe 缺失），与 T8 相同 |
| R-01 回归 | 单文件 4 GiB-1 / 4 GiB --zip | 0 / 1 | 4 GiB-1 32 s 成功、无 pending；4 GiB 拒绝、kind unsupported、无 pending |
| R-22 | NSIS 无/有 VERSION | 1 / 0 | 无 VERSION 按设计失败；有 VERSION 编译成功 |

体积对照：

| 产物 | 文档声明 | 本次实测 | 结论 |
| --- | --- | --- | --- |
| cli-only bt | 3,607,088 | 3,607,088 | 相符 |
| cli-only btrecv | 3,350,000 | 3,350,000 | 相符 |
| bt-gui | 11,173,528 | 11,173,528 | 相符 |
| 完整 bt | 11,687,448 | 11,687,448 | 相符 |
| .deb | 5,603,710–5,603,912 | 重建 5,603,926 / 5,603,842；dist 5,603,884 | 区间略窄，见 R-36 |
| NSIS | 未生成 | 脚本以 3.10 占位载荷编译成功（无 VERSION 失败） | 安装行为未验证 |

### 8.3 R-26 载荷生命周期与安全边界

已独立验证的通过项：

- 真实进程 SIGINT：BT_TEST_TRANSPORT=stall 下启动真实 bt send file --zip，SIGINT 后 exit 1、job canceled、journal paused、pending zip 保留、源文件保留；显式 resume cancel 删除 zip、源文件仍在。
- 脚本化 e2e：取消后 zip 146 B 保留；resume run 在带 OFFER/ACK/DONE 的脚本化传输上 completed=1，journal 与 pending 清空，source 保留；再次 resume cancel 清理自产 zip。
- GUI 集成测试：取消后 journal paused、zip 保留、zip_payloads 已收尾、source 保留。
- settle_owned_payload：有效 paused journal 引用时返回 false 并保留；journal 锁等待 5 s 后返回 false 并保留；pending 外用户源文件返回 false；pending/../victim.txt 普通外部文件因 canonicalize 在 pending 外而返回 false；pending 内指向外部文件的 symlink 返回 false。
- 明确记录的边界：pending 内用户源文件会在完成时被删除（R-33）；损坏 journal 会被当成空日志，settle 随后删除 payload（R-34）；pending/../victim-link 形式的悬挂 symlink 会被删除（R-35）。

### 8.4 新发现与剩余缺陷

**R-33 中 必修：用户源文件若位于 pending，会在传输完成时被删除，违反“用户源文件永不被删”**
- 现象：SendSource::File 的 produced 由 self.packages.is_owned(path) 路径判定（crates/bt-engine/src/send.rs:688）；用户在 pending 目录选择或放置的普通文件会被标记为自产，完成后 Journal::complete 的 cleaner 删除它（journal.rs:555/710、package.rs:138）。CLI/GUI 都没有拒绝 pending 内源文件。
- 独立复现：用仓库源码另建 harness，把 pending/user-source.txt（19 B）作为 SendSource::File 交给带脚本化 OFFER/ACK/DONE 的引擎；任务 Completed 后 SOURCE_EXISTS=false JOURNAL=0 PENDING_FILES=0，即源文件被删。
- 期望：用户选择的源文件永不删除，只有明确自产（CLI/GUI 预打包或 journal 恢复载荷）才可清理；至少应拒绝或提示 pending 内源文件。
- 建议：为 produced 引入显式来源标志（命令层/恢复层传入），不要仅按路径判断；或在 file_item/GUI 入队处拒绝 pending 内源文件并提示移动到其他目录；补“源文件在 pending”回归测试。
- 影响：违反 R-26 与 CLI.md“用户源文件永不删除”的绝对声明；源文件在应用内部目录才触发，但用户可通过 CLI/文件对话框选择该路径。

**R-34 低：损坏 journal 不满足“不可读时保守保留”**
- 现象：settle_owned_payload 调用 journal_for(paths).read_all()；journal 损坏时 read_unlocked 改名留证并返回 Ok(Vec::new())（crates/bt-engine/src/journal.rs:470-480），settle 视为“无引用”并删除 pending 载荷。独立测试：损坏 journal + 138 B payload，settle=true、payload 被删、journal 变 .corrupt.<ts>。
- 期望：若 T9 的“journal 不可读时保守保留”包括损坏文件，应保留并在 quarantine 后提示手工恢复；若设计上损坏=空日志，则应把该表述限定为“读取错误（锁/IO）时保留”，不要承诺损坏也保留。
- 建议：settle_owned_payload 在读取前后检查 journal 是否被 quarantine/消失，或提供严格读取 API；补损坏 journal 单测。

**R-35 低：settle_owned_payload 可经 pending/../ 删除 pending 外的悬挂 symlink**
- 现象：PendingPackageStore::is_owned 在 canonicalize 失败时回退为词法路径再 starts_with（crates/bt-engine/src/package.rs:138-148）；对不存在的悬挂 symlink，pending/../victim-link 的 canonicalize 失败，词法上仍在 pending 前缀内，delete 删除该外部 symlink。独立测试：外部 dangling symlink 被删（SETTLE=true），普通外部文件与指向外部文件的 pending symlink 均安全（SETTLE=false）。
- 期望：只允许删除 pending 内真实路径；拒绝含 .. 的路径或做词法归一化后再校验；Windows 的盘符/大小写/分隔符变体需专项测试。
- 建议：is_owned 先拒绝 ParentDir 组件或使用与 ensure_within 一致的归一化；settle_owned_payload 只接受已归一化的绝对路径；补 symlink 与 .. 测试。

**R-36 低：.deb 文档区间略窄**
- 现象：README:153、ARCHITECTURE:114、ACCEPTANCE:134、RELEASE:88/113 写 5,603,710–5,603,912；本次两次重建 5,603,926 与 5,603,842，dist 旧包 5,603,884；上限被突破 14 B。整体“约 5.60 MB”成立。
- 建议：改为“约 5.60 MB，字节数随打包时间戳在数 KB 内波动”并附命令，不承诺窄区间；或扩大区间并注明采样环境。

**R-37 低：R-08 字体检查仍以 ok=true 呈现**
- 现象：缺字体时 selftest 输出 [ok] font: 环境跳过...（crates/bt-gui/src/selftest.rs:104），不是失败项；虽已显式标注环境跳过并给出 BT_FONT_PATH，仍与“字体检查”语义有距离。
- 建议：若接受环境跳过，保留现状并在验收中说明；否则输出独立 skip 状态，避免用户把 ok 理解为字体可用。

**R-38 低：CLI 退出码表仍把单实例锁归到 4**
- 现象：docs/CLI.md:27 写 4 包含“单实例锁等待超时”，但单实例冲突实际走 BtError::busy→3（bt-gui/src/lib.rs 的 acquire/signal 分支）；当前只有 journal 锁返回 Locked→4。建议改表或明确 3/4 的区分。

### 8.5 未验证项

1. Rust 1.95 实际编译：本机只有 1.96；1.95 未安装，下载尝试在本环境停滞并取消；CI 的 MSRV 作业已接线但未实际运行。需在有 1.95 的机器或 CI 实跑 cargo check --workspace --all-targets。
2. 真实蓝牙扫描/配对/SDP/RFCOMM/OPP/助手通道、真实长传输 SIGINT 与断线续传；无硬件/BlueZ。
3. 真实 GUI 窗口/托盘/通知/剪贴板、Windows WinRT、NSIS 安装/升级/卸载、AppImage、GitHub Actions 发布：环境缺失。
4. .deb 实际安装/卸载；只做重建与 dpkg-deb 检查。
5. 真实旧 .NET 数据迁移与旧版接收端跨实现互传。
6. settle_owned_payload 的 Windows 大小写/分隔符/盘符路径变体；本机仅 Linux。
7. 4 GiB 以上真实蓝牙传输、磁盘满；未变。

### 8.6 最终判定

**有条件放行。** T9 对 R-26 主链路、R-28、R-29、R-30、R-32 的修复经独立复现成立；R-27 为低风险测量表述问题，R-31 的大部分项目已处理或已记录接受理由。剩余必修项只有 **R-33**：用户源文件若位于 pending/ 会在完成时被 journal cleaner 删除，直接违反“用户源文件永不被删”；应在命令层拒绝该来源或改为显式来源标志。R-34 至 R-38 为低风险边界/文档问题，建议发布前处理。MSRV 1.95、真实蓝牙、Windows 安装器、GUI 渲染与 AppImage 仍属未验证边界，不能以本机结果替代。
