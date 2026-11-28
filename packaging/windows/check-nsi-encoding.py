#!/usr/bin/env python3
"""校验 NSIS 脚本以 UTF-8 BOM 开头且正文可按 UTF-8 解码。

Windows 上的 makensis 在源码没有 BOM 时按系统 ANSI 代码页读取，
中文会变成乱码；BOM 是脚本自身可移植的编码声明。
"""

import argparse
import sys
from pathlib import Path

BOM = b"\xef\xbb\xbf"
SKIP_DIRS = {".git", ".toolchain", "target", "dist", "node_modules"}


def collect(root: Path) -> list[Path]:
    files: list[Path] = []
    for path in sorted(root.rglob("*")):
        if any(part in SKIP_DIRS for part in path.parts):
            continue
        if path.suffix.lower() in {".nsi", ".nsh"} and path.is_file():
            files.append(path)
    return files


def check(path: Path, root: Path) -> str | None:
    data = path.read_bytes()
    label = path.relative_to(root) if path.is_relative_to(root) else path
    if data.startswith((b"\xff\xfe", b"\xfe\xff")):
        return f"{label}: 使用 UTF-16 BOM，改为 UTF-8 BOM"
    if not data.startswith(BOM):
        return f"{label}: 缺少 UTF-8 BOM"
    try:
        data[len(BOM):].decode("utf-8")
    except UnicodeDecodeError as error:
        return f"{label}: 不是合法 UTF-8（{error}）"
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", nargs="?", default=Path(__file__).resolve().parents[2], type=Path)
    args = parser.parse_args()
    root: Path = args.root.resolve()
    if not root.is_dir():
        print(f"目录不存在：{root}", file=sys.stderr)
        return 2
    files = collect(root)
    if not files:
        print(f"未发现 .nsi/.nsh：{root}")
        return 0
    failures = [message for message in (check(path, root) for path in files) if message]
    for message in failures:
        print(f"失败 {message}", file=sys.stderr)
    if failures:
        print(f"NSIS 编码校验失败：{len(failures)}/{len(files)} 个文件", file=sys.stderr)
        return 1
    print(f"NSIS 编码校验通过：{len(files)} 个文件均为 UTF-8 BOM")
    return 0


if __name__ == "__main__":
    sys.exit(main())
