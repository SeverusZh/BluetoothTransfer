#!/usr/bin/env python3
"""调用 makensis 生成 Windows 安装器，显式声明源码输入字符集为 UTF-8。

BOM 与 /INPUTCHARSET 双保险：即使某个 include 文件漏了 BOM，
makensis 也不会按系统 ANSI 代码页读取中文。
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+$")


def find_makensis(explicit: str | None) -> Path | None:
    if explicit:
        return Path(explicit)
    found = shutil.which("makensis")
    if found:
        return Path(found)
    if os.name == "nt":
        for variable in ("ProgramFiles(x86)", "ProgramFiles"):
            base = os.environ.get(variable)
            if not base:
                continue
            candidate = Path(base) / "NSIS" / "makensis.exe"
            if candidate.is_file():
                return candidate
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="安装器版本，格式 x.y.z")
    parser.add_argument("--build-dir", default=str(ROOT / "target" / "release"), help="包含 exe 的构建目录")
    parser.add_argument("--output", help="输出安装器路径，默认写入 dist")
    parser.add_argument("--makensis", help="makensis 可执行文件路径")
    args = parser.parse_args()

    if not VERSION_PATTERN.match(args.version):
        print(f"版本号 {args.version} 不是 x.y.z 形式", file=sys.stderr)
        return 2
    makensis = find_makensis(args.makensis)
    if makensis is None or not makensis.is_file():
        print("未找到 makensis；Windows 用 choco install nsis，Linux 用 apt install nsis", file=sys.stderr)
        return 2
    build_dir = Path(args.build_dir).resolve()
    if not (build_dir / "bt-gui.exe").is_file():
        print(f"构建目录缺少 bt-gui.exe：{build_dir}", file=sys.stderr)
        return 2
    output = Path(args.output).resolve() if args.output else ROOT / "dist" / f"BluetoothTransfer-{args.version}-setup.exe"
    output.parent.mkdir(parents=True, exist_ok=True)
    script = ROOT / "packaging" / "windows" / "installer.nsi"

    option = "/" if os.name == "nt" else "-"
    command = [
        str(makensis),
        f"{option}INPUTCHARSET",
        "UTF8",
        f"{option}DVERSION={args.version}",
        f"{option}DBUILD_DIR={build_dir}",
        f"{option}DOUTPUT_FILE={output}",
        str(script),
    ]
    print(" ".join(command))
    result = subprocess.run(command, cwd=ROOT)
    if result.returncode != 0:
        print(f"makensis 失败，退出码 {result.returncode}", file=sys.stderr)
        return result.returncode
    if not output.is_file():
        print(f"安装器未生成：{output}", file=sys.stderr)
        return 1
    print(f"安装器已生成：{output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
