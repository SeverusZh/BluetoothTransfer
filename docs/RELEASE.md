# 发布构建与安装器

## 版本

- 工作区单一版本号写在根 `Cargo.toml` 的 `workspace.package.version`，当前为 2.0.0。
- 发布标签必须是 `v<version>`；`release.yml` 的版本校验作业会在标签与工作区版本不一致时直接失败。
- 发布流程只允许在 CI 中执行；本地脚本用于验证打包逻辑，不代替 Windows 真机安装验证。

## 构建矩阵

| 平台 | 目标 | 构建方式 | 产物 |
| --- | --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | `windows-latest` 原生构建 | GUI 绿色包 zip、cli-only zip、NSIS 安装器 |
| Linux x64 | `x86_64-unknown-linux-gnu` | `ubuntu-latest` 原生构建 | `.deb`、AppImage、GUI 单文件 |
| Linux 交叉检查 | `x86_64-pc-windows-msvc` | `cargo check --target` | 只做类型检查，无链接器 |

发布构建使用根 `Cargo.toml` 的 release 配置：`opt-level = "z"`、`lto = true`、`codegen-units = 1`、`panic = "abort"`、`strip = true`。

## 产物结构

Windows GUI 绿色包：

```text
BluetoothTransfer-<version>-windows-x64.zip
  bt-gui.exe
  LICENSE
  README.md（仓库存在时）
```

Windows cli-only 包：

```text
BluetoothTransfer-<version>-cli-windows-x64.zip
  bt.exe
  btrecv.exe
  LICENSE
```

Windows 安装器：`BluetoothTransfer-<version>-setup.exe`，per-user 安装到 `%LOCALAPPDATA%\Programs\BluetoothTransfer`，安装器内包含 GUI 与两个 CLI 可执行文件。

Linux 包：

```text
bluetooth-transfer_<version>_amd64.deb
  /usr/bin/bt-gui
  /usr/bin/bt
  /usr/bin/btrecv
  /usr/share/applications/bluetooth-transfer.desktop
  /usr/share/icons/hicolor/{16x16,32x32,48x48,256x256}/apps/bluetooth-transfer.png
  /usr/share/doc/bluetooth-transfer/copyright
BluetoothTransfer-<version>-x86_64.AppImage
  AppRun、usr/bin/{bt-gui,bt,btrecv}、桌面文件与 256px 图标
```

校验和统一输出为 `sha256sums.txt`；发布作业会再生成一份覆盖全部产物的 `combined-sha256sums.txt`。

## 安装器行为

NSIS 安装器：

- `RequestExecutionLevel user`，不申请管理员权限；所有注册表写入都在 `HKCU`。
- 安装前用 `taskkill` 结束正在运行的 `bt-gui.exe`、`bt.exe`、`btrecv.exe`，避免覆盖占用中的文件；只按本应用可执行文件名匹配。
- 默认安装主程序、开始菜单快捷方式与卸载项；升级安装只覆盖程序文件，不触碰 `%LOCALAPPDATA%\BlueToothTransfer` 与 `%APPDATA%\BlueToothTransfer` 中的配置、记录和日志。
- 可选组件：
  - 资源管理器右键菜单：写 `HKCU\Software\Classes` 下本应用的 5 个静态 Verb（文件、文件夹、文件夹背景、驱动器、`.txt` 文本），并创建 `%APPDATA%\Microsoft\Windows\SendTo\蓝牙传输.lnk`。Windows 11 需在「显示更多选项」中查看。
  - 开机自启：写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 的 `BluetoothTransfer` 值，带 `--minimized`。
- 卸载：删除开始菜单快捷方式、SendTo 快捷方式、本应用的 Verb 键、卸载项与 Run 值；询问是否同时删除用户数据，选择否时保留数据。
- 重复执行：安装器使用覆盖写入并先删除旧 `uninstall.exe`；重复安装不会累积菜单项或注册表项，卸载后再次安装可恢复。
- 静默安装：`BluetoothTransfer-<version>-setup.exe /S`；静默卸载：`uninstall.exe /S`。

Linux：

- `.deb` 可通过 `sudo apt install ./bluetooth-transfer_<version>_amd64.deb` 安装；`postinst`/`postrm` 只尝试刷新桌面数据库与图标缓存，失败不阻断安装。
- AppImage 需要 `chmod +x` 后直接运行；`APPIMAGE_EXTRACT_AND_RUN=1` 可在无 FUSE 环境解包运行。
- 用户级右键菜单与开机自启由应用设置页安装到 `~/.local/share` 与 `~/.config/autostart`，卸载应用不会自动删除这些用户级文件，可在设置页移除。

## 体积实测

本机为 Linux x86_64，无 MSVC 链接器、无 Windows 运行环境、无 `makensis` 预装。实测与上限如下，安装器体积一栏标注了验证边界。

| 产物 | 上限 | 本机实测 | 说明 |
| --- | --- | --- | --- |
| cli-only 二进制 `bt` | 8 MB | 3,609,904 字节（3.44 MiB） | `cargo build -p bt-cli --no-default-features --release` |
| cli-only 二进制 `btrecv` | 8 MB | 3,352,048 字节（3.20 MiB） | 同上，两个二进制合计 6.96 MB |
| 完整 `bt` | 无独立上限 | 11,690,904 字节（11.15 MiB） | 默认特性含 GUI，用作 GUI 绿色包内的命令行入口 |
| GUI 单文件 `bt-gui` | 20 MB | 11,176,216 字节（10.66 MiB） | 已开启 strip/LTO/opt-level z |
| NSIS 安装器 | 15 MB | 未生成 | 本机无 makensis、无 MSVC 链接器，只能在 CI 的 Windows runner 生成 |
| `.deb` | 20 MB（CI 断言） | 约 5.67 MB，字节数不可复现 | `VERSION=2.0.0 packaging/linux/build-deb.sh` 用 release 二进制构建；二进制变化、dpkg-deb 时间戳与压缩实现可致数十 KB 差异，文档只记录方法与量级，不记录精确字节 |
| AppImage | 25 MB（建议值，未在 CI 断言） | 未生成 | 无 appimagetool，下载超时 |

体积上限的依据：cli-only 目标来自架构文档；GUI 20 MB 与安装器 15 MB 来自任务验收目标，release 配置已开启 strip、LTO 与 `opt-level z`。本机没有 makensis 与 MSVC 链接器，NSIS 安装器无法生成，不用占位二进制伪造体积；真实安装器由 CI 的 Windows runner 构建并断言。

## 签名与安全

- 本轮没有 Windows 代码签名证书，安装器未签名；Windows SmartScreen 会提示未知发布者。需要正式分发时在 CI 中加入证书签名步骤。
- 发布产物提供 SHA256 校验和；下载后应核对校验和再运行。
- 安装器只写 `HKCU`、开始菜单、SendTo 快捷方式与自身安装目录；卸载只删除本应用创建的键和文件。

## 本地与 CI 验证记录

已验证：

- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 通过。
- `cargo build --workspace --release` 通过，记录 cli-only、GUI 与完整 bt 的体积。
- `cargo fmt -p bt-platform -p bt-gui --check` 通过。
- `cargo clippy -p bt-platform -p bt-gui --all-targets -- -D warnings` 通过。
- `cargo test -p bt-platform -p bt-gui` 通过：bt-platform 22 项（另有 1 项需要桌面会话 D-Bus 的 ignored）、bt-gui 9 项（8 项单元测试加 1 项 zip 载荷生命周期集成测试）。
- `cargo run -p bt-gui -- --selftest` 在无显示环境退出 0，16 项检查覆盖 7 个页面、配对对话框、PIN 回填、GUI zip 入队与清理、后端 mock 事件、收件箱往返、zip 往返与日志落盘。
- `cargo check -p bt-platform --target x86_64-pc-windows-msvc` 通过。
- `cargo build -p bt-gui --release` 与 `cargo build -p bt-cli --no-default-features --release` 通过并记录体积。
- Linux 托盘：在会话 D-Bus 上启动并持有 `org.kde.StatusNotifierItem` 名称；本机没有托盘宿主面板，图标未实际显示。
- NSIS 脚本：PATH 无 `makensis`，使用 gitignored 的 NSIS 3.10 解包副本复核：未传 `VERSION` 时脚本按设计失败，传 `-DVERSION=2.0.0` 加占位载荷可编译；产物只用于脚本验证，不作为发布产物，Windows 安装/卸载/升级仍未验证。
- `.deb` 构建脚本：用 release 二进制运行 `VERSION=2.0.0 packaging/linux/build-deb.sh`，产物约 5.67 MB 并通过 `dpkg-deb --info` 检查；精确字节随二进制、打包时间戳与压缩实现变化，不可复现，引用构建脚本输出而非固定值。

未验证（环境缺失，不伪造结论）：

- Windows 真机上的安装、升级、静默安装、卸载、右键菜单、SendTo、开机自启与托盘显示。
- Windows GUI 运行与 Windows 通知；本机 `cargo check -p bt-gui --target x86_64-pc-windows-msvc` 被依赖 `libsqlite3-sys` 的 C 构建脚本阻断：缺少 MSVC `lib.exe`，不是 GUI 代码类型错误。bt-platform 的 Windows 目标检查已通过。
- 真实蓝牙设备发现、配对、OPP/助手通道传输与接收；本机无蓝牙硬件与 BlueZ 服务。
- 真实窗口渲染：本机只验证无窗口 `--selftest`；尝试启动窗口进程时事件循环持续到超时，但 EGL 驱动告警且无人眼观察，不作为窗口可用结论。
- 桌面通知：本机会话没有通知守护进程，只验证了通知调用路径的编译与错误返回。
- 剪贴板、文件对话框、托盘宿主面板的真实交互。

## 常用命令

```bash
# 发布二进制
cargo build --release -p bt-gui -p bt-cli

# 纯 CLI
cargo build --release -p bt-cli --no-default-features

# Linux 包
VERSION=2.0.0 packaging/linux/build-deb.sh
VERSION=2.0.0 APPIMAGETOOL=/path/to/appimagetool packaging/linux/build-appimage.sh

# Windows 安装器与 runner 命令
makensis /DVERSION=2.0.0 /DBUILD_DIR=target/release /DOUTPUT_FILE=dist/setup.exe packaging/windows/installer.nsi
```

## 图标

`packaging/generate-icons.py` 生成 `packaging/windows/app.ico` 与 Linux hicolor 图标；运行时不内嵌字体或图片资源，托盘图标由代码绘制。修改品牌色或图形后重新执行该脚本。