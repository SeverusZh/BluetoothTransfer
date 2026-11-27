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

### 破坏性变更

- 程序与数据目录改为 Rust 版本的布局：配置在 `<配置根>/BluetoothTransfer/config.toml`，数据在 `<数据根>/BluetoothTransfer/`，journal 位于 `journal/send-journal.bin`。
- CLI 改为 `bt` 与 `btrecv` 两个可执行文件，退出码统一为 0 成功、1 操作失败、2 用法错误、3 占用、4 权限、5 未找到；旧命令名保留隐藏别名。
- GUI 无子命令入口、托盘、单实例与外部收件箱行为由新实现接管。
- OPP 分片默认值与上限统一为 65023 字节；旧配置中的越界值由归一化规则收敛。

### 迁移

- 用 `bt migrate --from <旧配置目录>` 导入旧版 config.json、data.db 与 send-journal.json；重复执行幂等，`--dry-run` 先探测。
- 旧版 .NET 源码归档在 `legacy/dotnet/`，只读参考，不参与构建。
- 真机验收边界见 `docs/ACCEPTANCE.md` 与 `docs/RELEASE.md`。
