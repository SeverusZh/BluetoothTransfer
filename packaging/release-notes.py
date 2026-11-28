#!/usr/bin/env python3
"""从 CHANGELOG.md 抽取指定版本的发布说明。

退出码 0 表示已找到该版本段落，3 表示 CHANGELOG 中没有该版本。
"""

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HEADING = re.compile(r"^##\s+(?P<version>\d+\.\d+\.\d+)(?:\s|$)")
VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+$")


def extract_version_section(text: str, version: str) -> str | None:
    lines = text.splitlines()
    start = None
    for index, line in enumerate(lines):
        match = HEADING.match(line)
        if match and match.group("version") == version:
            start = index
            break
    if start is None:
        return None
    end = len(lines)
    for index in range(start + 1, len(lines)):
        if lines[index].startswith("## "):
            end = index
            break
    body = lines[start + 1 : end]
    while body and not body[0].strip():
        body.pop(0)
    while body and not body[-1].strip():
        body.pop()
    return "\n".join(body)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True, help="版本号，格式 x.y.z")
    parser.add_argument("--changelog", type=Path, default=ROOT / "CHANGELOG.md", help="CHANGELOG 路径")
    parser.add_argument("--output", type=Path, help="说明输出文件，默认写标准输出")
    args = parser.parse_args()

    if not VERSION_PATTERN.match(args.version):
        print(f"版本号 {args.version} 不是 x.y.z 形式", file=sys.stderr)
        return 2
    try:
        text = args.changelog.read_text(encoding="utf-8")
    except OSError as error:
        print(f"无法读取 {args.changelog}：{error}", file=sys.stderr)
        return 2
    body = extract_version_section(text, args.version)
    if body is None or not body.strip():
        print(f"CHANGELOG 缺少 {args.version} 段落", file=sys.stderr)
        return 3
    notes = f"{body}\n"
    if args.output:
        args.output.write_text(notes, encoding="utf-8")
        print(f"发布说明已写入 {args.output}")
    else:
        sys.stdout.write(notes)
    return 0


if __name__ == "__main__":
    sys.exit(main())
