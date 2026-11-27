# 命令行文档

本文档描述 `bt` 与 `btrecv` 的稳定契约：命令、参数、JSON 字段、事件流与退出码。脚本与 Agent 应依赖本文档中的字段名与退出码，不要解析人类可读表格。

## 二进制与输出约定

- `bt`：完整命令集。
- `btrecv`：接收薄壳，无参数即监听，参数与 `bt recv` 一致。
- 数据写 stdout，诊断写 stderr。
- `--json`：stdout 恰好一个 JSON 对象，不混入日志与进度。
- `--events`：stdout 为 NDJSON，每行一个事件；最后一行固定为 `type=result`，失败时为 `type=error`。
- 错误对象字段：`ok=false`、`command`、`type=error`、`error.kind`、`error.message`、`error.hint`。
- 参数解析失败时若带 `--json` 或 `--events`，同样按各自模式输出错误对象或错误行，`command` 为 `bt` 或 `btrecv`，退出码 2。
- REPL 内每条非空命令输出一个对象；用法错误、未知命令与被禁命令同样输出错误对象，不静默写 stderr。
- 事件对象字段见「NDJSON 事件」一节。

字段命名一律 snake_case。时间一律为 UTC RFC 3339，形如 `2026-01-01T00:00:00Z`。

## 退出码

| 码 | 含义 | 典型场景 |
| --- | --- | --- |
| 0 | 成功 | 命令完成、`selftest` 全部通过、接收被 Ctrl+C 停止 |
| 1 | 操作失败 | 传输失败、自检未通过、`gui` 创建窗口失败、续传条目全部跳过 |
| 2 | 用法错误 | 未知命令、缺参数、地址非法、`--mode` 非法、清空记录缺少 `--yes` |
| 3 | 已有实例在运行 | 接收服务或任务队列被占用，后端返回 busy |
| 4 | 权限或锁失败 | 配置、数据、接收目录、导出目标不可写；配置、journal 或单实例锁等待超时 |
| 5 | 目标未找到 | 设备地址不在缓存、记录编号不存在、导出目标不可达 |

`devices detect` 使用独立三态映射：`present=0`、`absent=1`、`unknown=5`。

## 环境变量

| 变量 | 作用 |
| --- | --- |
| `BT_HOME` | 便携根覆盖。设置后配置在 `$BT_HOME/.config/BluetoothTransfer/`，数据在 `$BT_HOME/.local/share/BluetoothTransfer/`，默认接收目录为 `$BT_HOME/Downloads/BluetoothReceive` |
| `BT_BACKEND` | 值为 `mock` 时使用内存模拟后端；其它值走正式装配 |
| `XDG_CONFIG_HOME` / `XDG_DATA_HOME` | Linux 配置根与数据根，未设置时用 `$HOME/.config` 与 `$HOME/.local/share` |
| `APPDATA` / `LOCALAPPDATA` | Windows 配置根与数据根 |

数据布局：

```text
$BT_HOME/.config/BluetoothTransfer/
  config.toml
$BT_HOME/.local/share/BluetoothTransfer/
  records.db
  logs/
  journal/send-journal.bin   BTJ1 续传索引
  pending/
  auth.token    OPP 认证密码，权限 0600
$BT_HOME/Downloads/BluetoothReceive/
```

未设置 `BT_HOME` 时，配置根与数据根按平台拆分；接收目录默认是用户下载目录下的 `BluetoothReceive`。

## 密码处理

OPP 认证密码是第 13 项设置，不属于 12 项普通配置：

- 只能通过 `bt config set auth-password` 设置，值从 stdin 或 TTY 读取，TTY 下不回显。
- 传入位置参数值会被拒绝并返回 2；密码不出现在 argv、日志、错误信息与 JSON 输出中。
- `bt config get auth-password` 永远返回用法错误，密码不可读取。
- 空输入表示清除密码。
- 无人值守脚本使用管道：`printf '%s' "$PASSWORD" | bt config set auth-password`。

## 模拟后端

`BT_BACKEND=mock` 使用内存后端，不写磁盘、不访问蓝牙，用于无硬件环境验证 CLI 契约。固定设备：

| 地址 | 名称 | 配对 | RSSI | 助手通道 | 收藏 |
| --- | --- | --- | --- | --- | --- |
| `AA:BB:CC:00:00:01` | Mock Phone | 是 | -48 | available | 否 |
| `AA:BB:CC:00:00:02` | Mock Tablet | 否 | -60 | unknown | 否 |

模拟后端没有跨进程持久化：`bt send` 写入的记录在下一次独立进程调用中不可见。需要同一会话串联命令时使用 `bt repl --json`。`--zip` 的打包产物仍写入数据目录的 `pending/`，命令结束或取消后删除。

## 兼容别名

以下旧命令名保留且不出现在帮助里：

| 旧名 | 等价命令 |
| --- | --- |
| `opp-scan` | `devices scan` |
| `opp-pair` | `devices pair` |
| `opp-send-file` | `send file` |
| `opp-send-files` | `send files` |
| `opp-send-text` | `send text` |
| `opp-send-folder` | `send folder` |
| `detect` | `devices detect` |
| `stats` | `records stats` |

`devices`、`records`、`config` 不带子命令时分别等价于 `devices list`、`records list`、`config list`。

## 设备命令

### bt devices list

```text
bt devices list [--sort last|name|favorite] [--json]
```

读取设备缓存。`last` 按最近发现倒序，`name` 按展示名升序，`favorite` 收藏优先。

JSON：

```json
{"ok":true,"devices":[],"count":0}
```

### bt devices scan

```text
bt devices scan [--seconds N] [--paired-only] [--json] [--events]
```

开始扫描并等待结束。`--seconds` 为扫描时长上限，省略由后端决定。`--paired-only` 在扫描结束后只保留已配对设备。

JSON：

```json
{"ok":true,"devices":[],"count":0,"paired_only":false,"duration_s":5}
```

事件：`scan_started`、`device_list`、`scan_finished`、`result`。

### bt devices pair

```text
bt devices pair <ADDR> [--pin <PIN>] [--json]
```

发起配对并等待结果。确认类请求在非交互环境自动接受；PIN 类请求用 `--pin` 回填，未提供时在交互终端提示输入，非交互环境返回用法错误。PIN 经 `answer_pin` 传到传输层，不写入日志、JSON 与记录；值仍会出现在进程参数中，敏感场合避免在共享主机上使用。

JSON：

```json
{"ok":true,"address":"AA:BB:CC:00:00:02","paired":true,"device":{}}
```

`paired` 表示本次配对操作结果；`device` 是操作后的缓存快照，部分后端不会即时回填 `paired` 字段。

### bt devices alias

```text
bt devices alias <ADDR> [ALIAS...] [--json]
```

设置别名；省略别名或传空串清除。多个词以空格拼接。

JSON：

```json
{"ok":true,"address":"AA:BB:CC:00:00:01","alias":"工作机","device":{}}
```

### bt devices favorite

```text
bt devices favorite <ADDR> [--unset] [--json]
```

收藏或取消收藏。

JSON：

```json
{"ok":true,"address":"AA:BB:CC:00:00:01","favorite":true,"device":{}}
```

### bt devices detect

```text
bt devices detect <ADDR> [--json]
```

探测对端助手通道。三态与退出码：`present=0`、`absent=1`、`unknown=5`。探测超时或设备不在缓存归入 `unknown`。

JSON：

```json
{"ok":true,"address":"AA:BB:CC:00:00:01","state":"present","device":{}}
{"ok":false,"address":"AA:BB:CC:00:99:99","state":"unknown","device":null,"error":"设备不存在 AA:BB:CC:00:99:99"}
```

`error` 只在探测失败或未知时出现。

## 发送命令

### bt send file / bt send files

```text
bt send file <ADDR> <PATH>... [--zip] [--mode auto|assistant|opp] [--json] [--events]
bt send files <ADDR> <PATH>... [--zip] [--mode auto|assistant|opp] [--json] [--events]
```

顺序发送一个或多个文件并汇总。`--zip` 由 bt-app 的打包器把每个输入文件写成 Deflate zip 再发送，CLI 与 GUI 共用同一实现；产物位于数据目录 `pending/`，传输结束或失败后清理。

zip 走 ZIP32 字段，不做 ZIP64：单个文件或文件夹累计未压缩大小达到 4 GiB 时拒绝打包，退出码 1，`error.kind=unsupported`，`error.hint` 给出拆分或不打包发送的建议；打包失败不产出 zip、不残留 `pending/` 半成品。压缩后大小或条目偏移超出 ZIP32 上限时同样失败并清理产物。`--mode` 省略时使用配置的 `mode`；显式 `auto` 表示交给后端按配置与探测结果选择。

发送被 Ctrl+C 取消时，命令以 1 退出，任务状态为 canceled；journal 条目转为 paused 并保留自产载荷，可用 `bt resume list` 查看、`bt resume run` 继续。只有 `bt resume cancel`、传输完成或 7 天 TTL 清理会移除条目。

JSON：

```json
{"ok":true,"jobs":[],"summary":{"total":1,"completed":1,"failed":0,"canceled":0,"bytes":5}}
```

事件：`job_added`、`job_progress`、`job_state_changed`、`queue_idle`、`result`。

### bt send text

```text
bt send text <ADDR> <TEXT>... [--name NAME] [--mode ...] [--json] [--events]
```

文本以空格拼接后发送，对端落盘为文本文件。`--name` 只影响记录展示名；远端文件名由配置 `push_text_file_name` 决定。

### bt send folder

```text
bt send folder <ADDR> <DIR> [--mode ...] [--json] [--events]
```

目录由引擎打包为 zip 后发送，记录类型为 `folder`。

## 接收命令

### bt recv

```text
bt recv [--dir DIR] [--ask|--no-ask] [--once] [--json] [--events]
```

阻塞监听，Ctrl+C 停止并以 0 退出。`--once` 完成一次接收后退出。`--ask` 与 `--no-ask` 覆盖配置的 `receive_ask`。

非交互环境且开启询问时，未确认的接收请求会被拒绝并提示使用 `--no-ask`；无人值守接收必须显式 `--no-ask`。

`--json` 在退出时输出单个对象，阻塞期间不输出中间结果；需要实时观测请用 `--events`。

```json
{"ok":true,"stopped":true,"listening":false,"dir":"/path/BluetoothReceive","ask":true,"received":[],"received_count":0}
```

事件：`receive_listening`、`receive_progress`、`receive_ask`、`receive_done`、`result`。

### btrecv

```text
btrecv [--dir DIR] [--ask|--no-ask] [--once] [--json] [--events]
btrecv recv [同上参数]
btrecv --help
btrecv --version
```

无参数直接监听，不再进入一次性命令分支。退出码与 `bt recv` 一致。

## 记录命令

### bt records list

```text
bt records list [--direction send|recv] [--type file|multi-file|folder|text|other]
                [--status ok|failed|canceled|interrupted|running] [--peer ADDR]
                [--search TEXT] [--limit N] [--offset N] [--json]
```

默认 `--limit 100`、`--offset 0`。`--peer` 必须是蓝牙地址。

JSON：

```json
{"ok":true,"records":[],"count":0,"limit":100,"offset":0}
```

### bt records delete

```text
bt records delete <ID> [--json]
```

删除单条记录；编号不存在时退出 5。

JSON：`{"ok":true,"id":1,"deleted":true}`。

### bt records clear

```text
bt records clear --yes [--json]
```

清空全部记录，必须显式 `--yes`；缺少时退出 2。

JSON：`{"ok":true,"deleted":3}`。

### bt records stats

```text
bt records stats [--json]
```

JSON：`{"ok":true,"stats":{...}}`，`Stats` 字段见「数据对象」。

### bt export

```text
bt export <csv|jsonl> <OUT> [--json]
```

导出全部记录；`json` 是 `jsonl` 的兼容别名。

JSON：`{"ok":true,"path":"/tmp/records.csv","rows":1,"bytes":120,"format":"csv"}`。

## 配置命令

```text
bt config list [--json]
bt config get <KEY> [--json]
bt config set <KEY> [VALUE] [--json]
```

普通配置键：`opp_chunk_size`、`opp_connect_timeout_s`、`opp_send_idle_timeout_s`、`opp_retry_count`、`opp_retry_delay_s`、`opp_protection`、`opp_name_bom`、`assistant_chunk_size`、`push_text_file_name`、`receive_dir`、`receive_ask`、`mode`。旧式键名如 `OppChunkSize`、`TransferMode` 仍可识别。

密码键 `auth-password` 规则见「密码处理」。密码以外的键缺少值时报用法错误。

JSON：

```json
{"ok":true,"config":{"opp_chunk_size":65023,"mode":"auto"}}
{"ok":true,"key":"mode","value":"opp"}
{"ok":true,"key":"auth-password","set":true}
```

越界数值按配置归一化规则收敛，`set` 输出的是实际生效值。

## 续传命令

```text
bt resume list [--json]
bt resume run [--json] [--events]
bt resume cancel [--json]
```

`list` 通过 bt-app 读取 BTJ1 journal 索引；`run` 按 journal 恢复计划重新入队：pending 与 paused 条目强制走助手通道，缺失载荷、超过 7 天或超过重试上限的条目按 abandon 规则清除并写入 `warnings`，正在其他进程发送的条目标记为跳过。成功完成的条目从 journal 删除。`cancel` 删除全部待续传条目及自产载荷。取消的发送会以 paused 条目保留，`run` 会继续它。这三个命令打开后端时不自动恢复 journal，避免查询或取消前触发发送；只有 `send`、`recv` 与 `gui` 在打开后端时才自动恢复 journal，`devices`、`records`、`stats`、`export`、`config`、`resume list` 等查询命令不会触发发送。journal 为空时三个命令都以 0 退出，`run` 有条目但全部跳过时退出 1。

JSON：

```json
{"ok":true,"journal_dir":"/data/BluetoothTransfer/journal","entries":[{"id":"0123456789abcdef","peer":"AA:BB:CC:00:00:01","display_name":"photo.jpg","payload":"/data/BluetoothTransfer/pending/....bin","size":1024,"state":"paused","attempts":1,"updated_at":"2026-01-01T00:00:00Z"}],"count":1}
{"ok":true,"resumed":1,"jobs":[],"summary":{"total":1,"completed":1,"failed":0,"canceled":0,"bytes":1024},"warnings":[]}
{"ok":true,"canceled":1}
```

## 迁移命令

```text
bt migrate [--from DIR] [--dry-run] [--json]
```

迁移旧版 .NET 数据。省略 `--from` 时按平台配置目录推断为 `<配置根>/BluetoothTransfer`。`--dry-run` 只探测不写入。

JSON：

```json
{"ok":true,"from":"/home/user/.config/BluetoothTransfer","dry_run":false,"imported_records":0,"imported_config":false,"copied_files":0,"skipped":0,"warnings":[]}
```

## 自检与其它命令

```text
bt selftest [--json]
bt gui
bt repl [--json] [--events]
bt --version
bt --help
```

`selftest` 全部通过退出 0，否则 1。JSON：`{"ok":true,"checks":[{"name":"config","ok":true,"detail":"..."}],"failed":[]}`。

`gui` 启动图形界面；cli-only 构建返回明确错误并以 1 退出。无子命令的 `bt` 在带 GUI 的构建中打开窗口，在 cli-only 构建中打印用法并以 2 退出。

`repl` 从 stdin 逐行读取命令，双引号或单引号保留空格；支持 `help`、`?`、`quit`、`exit`。同一会话复用后端，`--json` 时每行命令输出一个 JSON 对象，`--events` 时每行输出一个 NDJSON 对象；参数解析失败、未知命令与被禁命令也输出错误对象。repl 打开后端时不自动恢复 journal，避免仅查询时触发发送。`gui`、`recv`、`repl` 不能在 repl 内运行；`config set auth-password` 也会被拒绝。

## NDJSON 事件

| type | 字段 |
| --- | --- |
| `log` | `level`、`message`、`at` |
| `scan_started` | 无 |
| `scan_finished` | `count` |
| `device_list` | `devices` |
| `device_updated` | `device` |
| `pair_prompt` | `request`、`device`、`kind` |
| `pair_result` | `device_id`、`ok`、`error` |
| `assistant_state_changed` | `device_id`、`state` |
| `job_added` | `job` |
| `job_progress` | `job_id`、`bytes`、`total`、`bps`、`eta_seconds` |
| `job_state_changed` | `job` |
| `queue_idle` | 无 |
| `receive_listening` | `listening` |
| `receive_progress` | `bytes`、`total`、`name` |
| `receive_ask` | `id`、`name`、`size` |
| `receive_done` | `ok`、`path`、`error` |
| `config_changed` | `config` |
| `records_changed` | 无 |
| `result` | 命令结果对象，字段与 `--json` 输出相同 |
| `error` | `command`、`error.kind`、`error.message`、`error.hint` |

## 数据对象

### Device

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| id | string | 蓝牙地址，大写冒号分隔 |
| name | string | 广播名 |
| alias | string|null | 别名 |
| paired | bool | 是否已配对 |
| connected | bool | 是否已连接 |
| favorite | bool | 是否收藏 |
| rssi | int|null | 信号强度 |
| last_seen | string|null | 最近发现时间 |
| assistant_state | string | `unknown`、`probing`、`available`、`unavailable` |
| channels | string[] | `opp`、`assistant` |

### JobSnapshot

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| id | int | 任务编号 |
| device_id | string | 目标地址 |
| device_name | string | 目标展示名 |
| item | object | `source`、`display_name`、`total_bytes`、`kind` |
| channel | string | `opp` 或 `assistant` |
| state | string | `queued`、`connecting`、`sending`、`retrying`、`paused`、`completed`、`failed`、`canceled` |
| bytes_sent | int | 已发送字节 |
| total_bytes | int | 总字节 |
| bps | int | 速率 |
| eta_seconds | int|null | 预计剩余秒数 |
| attempts | int | 尝试次数 |
| error | string|null | 失败原因 |
| resumable | bool | 是否可续传 |
| created_at | string | 入队时间 |
| updated_at | string | 最近状态时间 |

`item.source` 形如 `{"kind":"file","value":"/tmp/a.txt"}`；`kind` 为 `file`、`files`、`folder`、`text`。

### Record

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| id | int | 记录编号 |
| direction | string | `send` 或 `receive` |
| channel | string | `opp` 或 `assistant` |
| kind | string | `file`、`multi_file`、`folder`、`text`、`other` |
| status | string | `running`、`completed`、`failed`、`canceled`、`interrupted` |
| device_id | string|null | 对端地址 |
| device_name | string | 对端展示名 |
| display_name | string | 名称 |
| path | string|null | 本地路径 |
| checksum | string|null | 整文件 SHA-256，小写十六进制 |
| byte_size | int | 总字节 |
| transferred | int | 已传输字节 |
| error | string|null | 失败原因 |
| started_at | string | 开始时间 |
| finished_at | string|null | 结束时间 |
| resumable | bool | 是否可续传 |

CLI 输出不包含文本正文 `content`，避免大字段与敏感内容进入 stdout；需要核对文本时查看接收落盘文件。导出格式同样不含正文。

### Stats

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| total | int | 记录总数 |
| sent | int | 发送条数 |
| received | int | 接收条数 |
| failed | int | 失败条数 |
| bytes_sent | int | 发送字节 |
| bytes_received | int | 接收字节 |
| last_activity | string|null | 最近活动时间 |
| days | array | 每日 `date`、`sent`、`received`、`bytes` |

### Config

| 字段 | 默认 | 约束 |
| --- | --- | --- |
| opp_chunk_size | 65023 | 4096 至 65023 |
| opp_connect_timeout_s | 30 | 1 至 3600 |
| opp_send_idle_timeout_s | 300 | 1 至 86400 |
| opp_retry_count | 3 | 0 至 10 |
| opp_retry_delay_s | 3 | 0 至 300 |
| opp_protection | auto | `auto`、`plain`、`encrypt` |
| opp_name_bom | false | 布尔 |
| assistant_chunk_size | 262144 | 4096 至 1048557 |
| push_text_file_name | bt-note.txt | 清洗后的单段文件名 |
| receive_dir | 下载目录下的 BluetoothReceive | 非空 |
| receive_ask | true | 布尔 |
| mode | auto | `auto`、`assistant`、`opp` |

## 示例

以下示例在 `BT_BACKEND=mock` 下实际执行过，输出字段与真实命令一致：

```bash
export BT_HOME=$(mktemp -d)
export BT_BACKEND=mock

bt devices list --json
bt devices scan --seconds 1 --paired-only --json
bt devices pair AA:BB:CC:00:00:02 --json
bt send file AA:BB:CC:00:00:01 /tmp/photo.jpg --json
bt send text AA:BB:CC:00:00:01 "hello" --events
bt records list --direction send --limit 10 --json
bt export jsonl /tmp/records.jsonl --json
bt selftest --json
bt migrate --json
```

串联同一会话的状态流：

```bash
printf 'send text AA:BB:CC:00:00:01 hello --json\nrecords list --json\nquit\n' | bt repl --json
```