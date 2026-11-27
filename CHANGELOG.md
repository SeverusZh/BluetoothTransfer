# 变更记录

## 2.0.0

Rust 重写版本，取代原 Windows .NET 8 WPF 实现。

### 重构要点

- 单一 Rust workspace：bt-core、bt-api、bt-transport、bt-store、bt-engine、bt-app、bt-platform、bt-gui 与 bt-cli。
- GUI 与 CLI 共用 bt-api 契约和 bt-app 装配根，后端可替换为无硬件 MockBackend。
- 助手私有协议 v1、OBEX 对象推送、断点续传 journal、接收服务与旧版数据迁移全部重写。
- 新增跨平台 GUI、Linux BlueZ / Windows WinRT 传输后端、NSIS 安装器与 Linux 打包。
- 单文件 zip 打包统一由 bt-app 提供，CLI 与 GUI 行为一致，产物在结束后清理。
- 配对 PIN 通过 answer_pin 从 CLI 或 GUI 回填到传输层，不再固定为 0000。

### 审查修复

- 单文件与文件夹 zip 按 ZIP32 上限校验：单文件或累计未压缩大小达到 4 GiB 时以退出码 1 拒绝，不产出归档、不残留 `pending/` 半成品。
- 发送取消保留可续传条目：Ctrl+C 后 journal 条目转为 paused 并保留载荷，`resume list` 可见、`resume run` 可继续；只有 `resume cancel`、完成或 TTL 清理会移除。
- 权限错误映射为 `error.kind=permission` 与退出码 4；新增 `locked` 分类对应锁等待失败，退出码同样为 4。
- `--json` 与 `--events` 在参数解析失败、btrecv 解析失败与 REPL 内错误时输出契约内的错误对象。
- 发送进度每 60 秒续租 journal 并更新已确认偏移，长传输不再因租约过期被视为可接管。
- 打开后端时只有 `send`、`recv` 与 `gui` 自动恢复 journal；查询、配置、导出与 `resume list` 不再触发发送。
- MSRV 由 1.85 修正为 1.95，依据 eframe/egui 0.37 的 `rust-version`；`cargo xtask msrv` 与 CI 校验声明一致性。

### 破坏性变更

- 程序与数据目录改为 Rust 版本的布局：配置在 `<配置根>/BluetoothTransfer/config.toml`，数据在 `<数据根>/BluetoothTransfer/`，journal 位于 `journal/send-journal.bin`。
- CLI 改为 `bt` 与 `btrecv` 两个可执行文件，退出码统一为 0 成功、1 操作失败、2 用法错误、3 占用、4 权限、5 未找到；旧命令名保留隐藏别名。
- GUI 无子命令入口、托盘、单实例与外部收件箱行为由新实现接管。
- OPP 分片默认值与上限统一为 65023 字节；旧配置中的越界值由归一化规则收敛。

### 迁移

- 用 `bt migrate --from <旧配置目录>` 导入旧版 config.json、data.db 与 send-journal.json；重复执行幂等，`--dry-run` 先探测。
- 旧版 .NET 源码归档在 `legacy/dotnet/`，只读参考，不参与构建。
- 真机验收边界见 `docs/ACCEPTANCE.md` 与 `docs/RELEASE.md`。
