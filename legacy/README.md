# 旧版 .NET 实现归档

本目录保存重构前的 Windows .NET 8 WPF 实现，包含 GUI、CLI、接收端、应用服务、测试与发布脚本。

- 只读参考：不参与 Rust workspace 构建、测试与发布，根 `Cargo.toml` 的 members 不包含本目录。
- 协议与行为以本目录为实现基线，最终以 Rust 实现及其测试为准；两者冲突时先核对新旧代码与 `docs/PROTOCOL.md`。
- 历史源码完整保留，便于追溯旧版数据格式、OBEX 交互与界面行为；不接受新功能修改。
- 旧版解决方案 `BluetoothTransfer.sln` 与 `src/`、`tests/`、`tools/` 保持原有内部结构。

迁移旧版用户数据使用 `bt migrate --from <旧配置目录>`，无需构建本目录。
