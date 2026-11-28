# 发布构建与安装器

## 版本

- 工作区单一版本号写在根 `Cargo.toml` 的 `workspace.package.version`，当前为 2.0.1。
- 发布标签必须是 `v<version>`；版本校验作业会在标签与工作区版本不一致时直接失败。
- `main` 上的质量门禁与打包全部通过后自动创建标签并发布，不需要人工打标签或手动触发。
- 发布流程只允许在 CI 中执行；本地脚本用于验证打包逻辑，不代替 Windows 真机安装验证。

## 发布流水线

流水线集中在 `.github/workflows/ci.yml`，门禁、打包与发布共用同一份作业定义，按触发事件决定是否发布。

### 触发矩阵

| 事件 | 门禁与打包 | 发布 |
| --- | --- | --- |
| `pull_request` | 执行 | 不执行 |
| `push` 到 `main` | 执行 | 执行 |
| `workflow_dispatch` 在 `main` | 执行 | 执行 |
| `push` 标签 `v*` | 执行 | 执行，标签必须等于工作区版本 |
| `workflow_dispatch` 在其它分支 | 执行 | 不执行 |

### 作业依赖

```text
version ──┬── quality ──────────┐
          ├── windows-package ──┤
          └── linux-package ────┼── publish
msrv ──────────────────────────┘
```

- `version`：从根 `Cargo.toml` 的 `workspace.package.version` 读取版本，校验 `x.y.z` 形式；标签触发时还要求 `v<version>` 与标签名一致。
- `quality`：双平台执行 MSRV 元数据检查、格式化、Clippy `-D warnings`、测试、发布构建、纯 CLI 构建与 GUI 自检。
- `msrv`：用 1.95.0 执行 `cargo check --workspace --all-targets`。
- `windows-package`：release 构建、NSIS 编码校验、生成安装器、PE 子系统与资源校验、GUI zip 与 cli-only zip、体积断言、校验和，上传 `windows-release`。
- `linux-package`：release 构建、`.deb`、AppImage、体积断言、校验和，上传 `linux-release`。
- `publish`：依赖以上全部作业，下载全部 artifact，生成 `combined-sha256sums.txt`，用 CHANGELOG 对应版本段落生成说明，发布非 draft、非 prerelease 的 Release 并附全部资产。

### 幂等与版本

- 版本唯一来源是 `Cargo.toml` 的 `workspace.package.version`，流水线不写死版本号。
- Release `v<version>` 已存在时记录 notice 并跳过发布，退出码 0，不覆盖已发布资产。
- 标签 `v<version>` 已存在且指向当前提交时继续创建 Release，覆盖标签触发与重跑场景。
- 标签 `v<version>` 已存在但指向其它提交时记录 notice 并跳过，绝不移动标签。
- 创建 Release 时以当前提交为目标，等价于 `gh release create v<version> --target <sha>`，标签指向被门禁验证过的提交。
- 发布作业使用 concurrency 组 `publish-v<version>`，同一版本并发触发时串行化，后到的一次在检查阶段发现 Release 已存在并跳过。
- 发布用 `GITHUB_TOKEN` 创建标签，GitHub 不为该 token 创建的事件再次触发工作流，因此没有标签递归触发。

### 权限与安全

- 工作流默认 `contents: read`，只有 `publish` 作业声明 `contents: write` 与 `actions: read`。
- 仓库需在 Settings → Actions → General → Workflow permissions 允许工作流申请写权限；PR 运行始终只读。
- 凭据只通过 `GITHUB_TOKEN` 注入 `gh`，不打印 token，不输出环境变量全集。

### PR 约束

`pull_request` 运行永远不发布：`publish` 作业在 PR 事件下被条件跳过，工作流只有读取权限。PR 只做门禁、Windows 打包与 Linux 打包校验，其中 Windows 作业断言 NSIS 编码、PE 子系统与 `.rsrc` 资源。

### 失败排查

- 版本校验失败：检查 `Cargo.toml` 的 `workspace.package.version` 与标签名是否一致。
- Windows 打包失败：查看 `Windows 打包` 作业的编码校验、安装器生成与 PE 校验步骤；`winresource` 需要 Windows SDK 的 `rc.exe`，`windows-latest` 自带。
- Linux 打包失败：查看依赖安装、appimagetool 下载与体积断言。
- 发布失败：查看 `发布前检查` 的 notice 与 `创建 Release` 的 `gh` 输出；Release 已存在时按设计跳过，不是失败。

### 安装器编码与 PE 断言

- `packaging/windows/check-nsi-encoding.py` 校验全部 `.nsi`/`.nsh` 为 UTF-8 BOM；`packaging/windows/build-installer.py` 调用 makensis 时显式传 `/INPUTCHARSET UTF8`。
- `packaging/windows/verify-pe.py` 校验 `bt-gui.exe` 为 GUI 子系统且含 `.rsrc`，`bt.exe` 与 `btrecv.exe` 为控制台子系统。

## 构建矩阵

| 平台 | 目标 | 构建方式 | 产物 |
| --- | --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | `windows-latest` 原生构建 | GUI 绿色包 zip、cli-only zip、NSIS 安装器 |
| Linux x64 | `x86_64-unknown-linux-gnu` | `ubuntu-latest` 原生构建 | `.deb`、AppImage、GUI 单文件 |
| Linux 交叉检查 | `x86_64-pc-windows-msvc` | 本地 `cargo check --target` | 只做类型检查，不在 CI 运行 |

构建工具链由根 `rust-toolchain.toml` 固定为 1.100.1，`ci.yml` 的 `dtolnay/rust-toolchain` 与质量门禁使用同一版本；MSRV 仍为 1.95，由独立的 MSRV 作业校验。

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

Windows 与 Linux 构建分别输出 `sha256sums-windows.txt` 与 `sha256sums-linux.txt`；发布作业再生成覆盖全部产物的 `combined-sha256sums.txt`，行内只写文件名，可直接用 `sha256sum -c` 校验。

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
- 源码编码：安装器脚本与全部 `.nsi`/`.nsh` 保存为 UTF-8 BOM；编译入口 `packaging/windows/build-installer.py` 另传 `/INPUTCHARSET UTF8`（Linux 上为 `-INPUTCHARSET UTF8`）。缺少 BOM 时 Windows 的 makensis 会按系统 ANSI 代码页读取，安装界面中文乱码。

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
| AppImage | 25 MB（建议值，未在 CI 断言） | 8,555,000 字节（8.16 MiB） | 用 appimagetool continuous 构建，`--selftest` 退出 0 |

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
- `cargo test -p bt-platform -p bt-gui` 通过：bt-platform 22 项（另有 1 项需要桌面会话 D-Bus 的 ignored）、bt-gui 17 项单元测试（含设置页滚轮与全部页面滚动度量、内嵌图标解码、自检报告落盘）加 2 项载荷生命周期集成测试。
- `cargo run -p bt-gui -- --selftest --report /tmp/bt-gui-report.json` 退出 0，生成含 16 项检查的合法 JSON；Linux 下不传 `--report` 时行为与之前一致，只写标准输出。
- `cargo check -p bt-transport -p bt-platform -p bt-engine --target x86_64-pc-windows-msvc` 通过；`cargo check -p bt-gui --target x86_64-pc-windows-msvc` 仍被依赖 `libsqlite3-sys` 的 C 构建脚本阻断，缺少 MSVC `lib.exe`。
- `python3 packaging/windows/check-nsi-encoding.py` 通过；复制一份无 BOM 的样本到临时目录后脚本按预期退出 1。
- `python3 packaging/windows/verify-pe.py` 对合成的 GUI 与控制台 PE 样本校验通过；对本机 ELF 二进制按预期报「缺少 MZ 头，不是 PE 文件」并以非 0 退出。
- `.github/workflows/ci.yml` 经 PyYAML 解析通过；`packaging/release-notes.py --version 2.0.1` 抽取到 CHANGELOG 对应段落，缺失版本按设计以退出码 3 结束。
- `packaging/windows/build-installer.py` 在 PATH 无 `makensis` 时按预期退出 2；指定 `target/t3/nsis` 副本时可编译安装器并读回中文。
- `cargo run -p bt-gui -- --selftest` 在无显示环境退出 0，16 项检查覆盖 7 个页面、配对对话框、PIN 回填、GUI zip 入队与清理、后端 mock 事件、收件箱往返、zip 往返与日志落盘；并行与重复执行的自检用例连续 5 轮通过。
- `cargo check -p bt-platform --target x86_64-pc-windows-msvc` 通过。
- `cargo build -p bt-gui --release` 与 `cargo build -p bt-cli --no-default-features --release` 通过并记录体积。
- Linux 托盘：在会话 D-Bus 上启动并持有 `org.kde.StatusNotifierItem` 名称；本机没有托盘宿主面板，图标未实际显示。
- NSIS 脚本：PATH 无 `makensis`，使用 `target/t3/nsis` 下的 NSIS 3.10 解包副本复核。`packaging/windows/build-installer.py --version 2.0.1` 带占位载荷编译通过，makensis 以 `-INPUTCHARSET UTF8` 处理脚本并报告 `(UTF8)`；`-PPO` 预处理输出中 `蓝牙传输`、`用蓝牙传输发送` 等中文完整无乱码。同一脚本去掉 BOM 并以 `-INPUTCHARSET ACP` 预处理时按预期报 `Bad text encoding`，说明 BOM 与字符集参数确实生效。占位产物只用于脚本验证，不作为发布产物；Windows 上的真实安装、卸载与升级仍未验证。
- `.deb` 构建脚本：用 release 二进制运行 `VERSION=2.0.0 packaging/linux/build-deb.sh`，产物约 5.67 MB 并通过 `dpkg-deb --info` 检查；精确字节随二进制、打包时间戳与压缩实现变化，不可复现，引用构建脚本输出而非固定值。
- AppImage：用 appimagetool continuous 与 `packaging/linux/build-appimage.sh` 生成 8,555,000 字节产物，`APPIMAGE_EXTRACT_AND_RUN=1` 运行 `--selftest` 退出 0。

未验证（环境缺失，不伪造结论）：

- 自动发布路径：main 上自动创建标签与 Release 需要合并后由 GitHub Actions 运行验证；PR 运行不发布，只覆盖门禁与双平台打包。
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
VERSION=2.0.1 packaging/linux/build-deb.sh
VERSION=2.0.1 APPIMAGETOOL=/path/to/appimagetool packaging/linux/build-appimage.sh

# Windows 安装器：显式 UTF-8 输入字符集，输出 dist/BluetoothTransfer-2.0.1-setup.exe
python3 packaging/windows/build-installer.py --version 2.0.1 --build-dir target/release

# 安装器源码编码与 PE 子系统校验
python3 packaging/windows/check-nsi-encoding.py
python3 packaging/windows/verify-pe.py target/release/bt-gui.exe target/release/bt.exe target/release/btrecv.exe

# 从 CHANGELOG 抽取发布说明
python3 packaging/release-notes.py --version 2.0.1 --output release-notes.md
```

## 图标

`packaging/generate-icons.py` 生成 `packaging/windows/app.ico`、窗口图标源 `packaging/windows/app.png` 与 Linux hicolor 图标。`bt-gui` 在 Windows 构建时通过 `build.rs` 把 ico 编译进 exe 资源，并在启动时把 app.png 解码为窗口图标；托盘图标由代码绘制。修改品牌色或图形后重新执行该脚本。