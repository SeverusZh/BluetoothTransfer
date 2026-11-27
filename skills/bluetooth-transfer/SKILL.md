---
name: bluetooth-transfer
description: 在无图形界面、远程 shell、CI 或需要脚本化时，用 bt CLI 完成蓝牙设备发现、配对、OPP 与助手通道发送、接收、记录、导出与旧数据迁移。包含 JSON 契约、退出码、错误恢复与安全边界。
---

# 蓝牙传输 CLI

## 何时使用

优先用 CLI：

- 无显示环境、SSH、容器、CI，或需要被脚本/Agent 调用。
- 需要机器可读结果：`--json`、`--events`、稳定退出码。
- 需要常驻接收端：`bt recv` 或 `btrecv`。
- 需要批量发送、导出记录、迁移旧数据。

改用 GUI：

- 需要拖放、剪贴板、托盘、系统通知、资源管理器右键菜单。
- 需要浏览队列并逐任务暂停、继续、重试。
- 需要在配对弹窗中查看系统口令。

GUI 与 CLI 共用同一后端与数据目录，记录互通。

## 前置检查

```bash
bt --version
bt selftest --json
bt devices list --json
```

`selftest` 退出 0 表示配置、存储、路径与装配可用。无蓝牙硬件时用模拟后端验证契约：

```bash
export BT_BACKEND=mock
export BT_HOME=$(mktemp -d)
bt devices list --json
```

模拟后端设备固定为 `AA:BB:CC:00:00:01` Mock Phone（已配对、助手通道可用）与 `AA:BB:CC:00:00:02` Mock Tablet。模拟后端不写配置与记录，跨进程不保留状态；`--zip` 的打包产物由 bt-app 写入数据目录 `pending/` 并在命令结束后删除。需要串联状态时用 `bt repl --json`。

## 命令矩阵

| 目标 | 命令 |
| --- | --- |
| 列出缓存设备 | `bt devices list [--sort last\\|name\\|favorite] --json` |
| 扫描 | `bt devices scan [--seconds N] [--paired-only] --json` |
| 配对 | `bt devices pair <ADDR> [--pin PIN] --json` |
| 别名 | `bt devices alias <ADDR> [ALIAS...] --json` |
| 收藏 | `bt devices favorite <ADDR> [--unset] --json` |
| 助手通道三态 | `bt devices detect <ADDR> --json` |
| 发送文件 | `bt send file <ADDR> <PATH>... [--zip] [--mode auto\\|assistant\\|opp] --json` |
| 发送多文件 | `bt send files <ADDR> <PATH>... --json` |
| 发送文本 | `bt send text <ADDR> <TEXT>... [--name NAME] --json` |
| 发送文件夹 | `bt send folder <ADDR> <DIR> --json` |
| 接收 | `bt recv [--dir DIR] [--ask\\|--no-ask] [--once] --json` |
| 接收薄壳 | `btrecv [同上参数]`，无参数即监听 |
| 记录 | `bt records list [筛选] --json` |
| 删除/清空/统计 | `bt records delete <ID>`、`bt records clear --yes`、`bt records stats --json` |
| 导出 | `bt export <csv\\|jsonl> <OUT> --json` |
| 配置 | `bt config list\\|get <KEY>\\|set <KEY> [VALUE]` |
| 断点续传 | `bt resume list\\|run\\|cancel --json` |
| 迁移旧数据 | `bt migrate [--from DIR] --json` |
| 长任务事件 | 追加 `--events`，按行读取 NDJSON，最后一行 `type=result` |

## 退出码

| 码 | 含义 | 处理 |
| --- | --- | --- |
| 0 | 成功 | 继续 |
| 1 | 操作失败 | 读取 `error.kind` 与 `error.hint` |
| 2 | 用法错误 | 修参数，不要重试 |
| 3 | 已有实例在运行 | 停止旧接收端或等待任务结束 |
| 4 | 权限或锁失败 | 修目录权限或等待锁释放，不要用 sudo 跑 GUI |
| 5 | 目标未找到 | 先 `devices scan` 或 `records list` 获取有效目标 |

`devices detect` 例外：`present=0`、`absent=1`、`unknown=5`。`absent` 只表示私有助手通道不可用，OPP 仍可能可用。

## JSON 读取规则

- `--json`：stdout 恰好一个对象；不要从 stderr 读数据。
- `--events`：每行一个对象；最后一行 `type=result` 或 `type=error`；参数解析失败也会输出 `type=error` 行。
- 失败对象：`{"ok":false,"command":"send.file","type":"error","error":{"kind":"not_found","message":"...","hint":"..."}}`。
- 成功对象一律含 `ok: true`；发送结果含 `jobs` 与 `summary`；记录结果含 `records` 与 `count`。
- 字段为 snake_case，时间 UTC RFC 3339。完整 schema 见 `docs/CLI.md`。

## 典型工作流

### 发现到核对

```bash
ADDR=$(bt devices scan --seconds 8 --json | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
bt devices pair "$ADDR" --json
bt devices detect "$ADDR" --json || true
bt send file "$ADDR" /tmp/report.pdf --json
bt records list --direction send --limit 5 --json
```

`sed` 提取只是最小示例；复杂解析用 `jq` 读取 `devices[0].id`。

### 发送并核对记录

先发送，再用记录编号核对，不要用文件名猜：

```bash
OUT=$(bt send folder "$ADDR" /tmp/dataset --json)
echo "$OUT"
bt records list --direction send --limit 1 --json
```

文件夹自动打包为 zip，记录类型为 `folder`。

### 接收端常驻

```bash
bt recv --dir /srv/inbox --no-ask --events
```

- 无人值守必须 `--no-ask`；否则非交互环境会拒绝未确认请求。
- 事件流用 `--events`；`--json` 只在退出时输出汇总。
- Ctrl+C 停止，退出码 0。
- 需要单次接收用 `--once`。
- `btrecv` 无参数即监听，等价于 `bt recv`。

### 断点续传

```bash
bt resume list --json
bt resume run --json
```

`resume run` 按 BTJ1 journal 恢复计划重新入队：pending 与 paused 条目强制走助手通道；缺失载荷、超过 7 天或超过重试上限的条目会被清除并写入 `warnings`。成功完成的条目从 journal 删除。`resume cancel` 取消全部待续传条目，空 journal 返回 `canceled=0`。发送被 Ctrl+C 取消后条目以 paused 保留，不会被隐式删除；只有显式 `resume cancel`、完成或 7 天 TTL 清理会移除。

查询、配置、导出与 `resume list` 打开后端时不会自动恢复 journal；续传只由 `resume run` 或 `send`/`recv`/`gui` 流程触发。

### 导出与统计

```bash
bt records stats --json
bt export jsonl /tmp/records.jsonl --json
bt records clear --yes --json
```

### 串联状态

```bash
printf 'send text AA:BB:CC:00:00:01 hello --json\nrecords list --json\nquit\n' | bt repl --json
```

REPL 内每行一个 JSON；stdin 非 TTY 时不打印提示符。

## 错误恢复决策

| 现象 | 判断 | 动作 |
| --- | --- | --- |
| 退出 2 | 参数/地址/模式非法 | 读 `error.hint`，修参数后重跑 |
| 退出 3 | 接收端或队列被占用 | 找到并停止旧 `bt recv`/`btrecv`，或等待任务结束 |
| 退出 4 | 目录不可写、锁冲突 | 换有写权限的 `--dir`，修权限后重试；不要 sudo |
| 退出 5 发送 | 设备不在缓存 | `devices scan`，确认地址，再 `devices list` |
| 退出 5 记录 | 编号不存在 | `records list` 取有效编号 |
| detect 返回 1 | 对端助手不可用 | 改用 `--mode opp`，或让对方启动接收端 |
| detect 返回 5 | 未知/超时 | 确认设备在线、已配对；必要时 `devices pair` |
| 传输 failed | 对端断开、超时、协议拒绝 | 看 `jobs[].error`；重跑同一命令触发续传；必要时调大 `opp_send_idle_timeout_s` |
| 发送被 Ctrl+C | 任务 canceled，journal 条目转为 paused 并保留载荷 | `resume list` 后 `resume run` |
| 接收被拒绝 | 非交互 + ask | 用 `--no-ask` 或先在终端确认 |
| 配置越界 | 值被收敛 | 读 `config get` 的实际值 |

## 安全边界

- 认证密码只走 stdin 或 TTY：`printf '%s' "$PASSWORD" | bt config set auth-password`。不要放进 argv、环境变量快照或日志。
- 密码不可读取，`config get auth-password` 永远报错。
- 接收目录是唯一落盘根；对端文件名会被清洗，不能用 `..` 穿越。
- 不要用 root 运行 GUI；CLI 接收目录需可写。
- 传输本身是明文蓝牙协议；不要在不可信环境发送敏感数据。
- 密码、PIN、路径都可能出现在 shell 历史中；密码用 `read -rsp`，PIN 用 `--pin` 或交互输入，不写入日志、记录与 JSON。

## 常见陷阱

- iOS 不支持 OPP 接收；对 iPhone 发送文件需要对方运行接收端或改用其它通道。
- 助手通道要求对端运行本应用；普通 OPP 只需对方支持蓝牙文件接收。
- `send files` 是顺序单文件任务，不是一个多文件任务；`summary.total` 等于文件数。
- `--zip` 对 CLI 的每个输入文件分别打包；GUI 的 zip 选项只对单个文件生效，多文件时提示后按原样发送。文件夹由引擎自动打包。
- `--name` 只改记录展示名，不改远端文件名；远端名由 `push_text_file_name` 决定。
- `--mode auto` 等价于交给配置的 `mode`；强制 `assistant` 失败不会回退 OPP。
- 非交互环境配对确认会自动接受；需要人工确认时在 TTY 下运行。
- 模拟后端跨进程不持久化；状态断言放进 `bt repl --json` 或单进程脚本。
- `--json` 时错误也写 stdout；不要只看 stderr 判断成败。
- 设备地址大小写与分隔符不敏感，记录编号不接受地址。

## 示例脚本

- `scripts/send-and-verify.sh <ADDR> <PATH> [--zip]`：发送并打印记录。
- `scripts/receive-loop.sh [DIR]`：常驻接收，Ctrl+C 退出。
