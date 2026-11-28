#!/usr/bin/env python3
"""解析 PE 头，校验 Windows 可执行文件的子系统与资源段。

bt-gui.exe 必须是 GUI 子系统（2）且带 .rsrc 段（图标与版本资源），
bt.exe 与 btrecv.exe 必须是控制台子系统（3），保证 CLI 输出不被隐藏。
"""

import argparse
import struct
import sys
from pathlib import Path

SUBSYSTEM_NAMES = {2: "GUI", 3: "Console"}


class PeError(Exception):
    pass


def parse_pe(data: bytes) -> dict:
    if len(data) < 0x40 or data[:2] != b"MZ":
        raise PeError("缺少 MZ 头，不是 PE 文件")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if pe_offset + 24 > len(data):
        raise PeError("PE 头偏移越界")
    if data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise PeError("缺少 PE 签名")
    coff = pe_offset + 4
    _machine, section_count, _timestamp, _symbols, _symbol_count, optional_size, _flags = struct.unpack_from(
        "<HHIIIHH", data, coff
    )
    optional = coff + 20
    if optional_size < 70 or optional + optional_size > len(data):
        raise PeError("可选头长度非法")
    magic = struct.unpack_from("<H", data, optional)[0]
    if magic not in (0x10B, 0x20B):
        raise PeError(f"未知可选头类型 0x{magic:x}")
    subsystem = struct.unpack_from("<H", data, optional + 68)[0]
    sections: list[str] = []
    table = optional + optional_size
    for index in range(section_count):
        offset = table + index * 40
        if offset + 40 > len(data):
            raise PeError("节表越界")
        name = data[offset : offset + 8].split(b"\0", 1)[0].decode("ascii", "replace")
        raw_size = struct.unpack_from("<I", data, offset + 16)[0]
        raw_pointer = struct.unpack_from("<I", data, offset + 20)[0]
        if raw_size and raw_pointer + raw_size > len(data):
            raise PeError(f"节 {name} 的原始数据越界")
        sections.append(name)
    return {"subsystem": subsystem, "sections": sections}


def expected_subsystem(name: str, override: int | None) -> int:
    if override is not None:
        return override
    if name.startswith("bt-gui"):
        return 2
    if name in {"bt", "btrecv"}:
        return 3
    raise PeError(f"无法从文件名 {name} 推断预期子系统，请用 --subsystem 指定")


def verify(path: Path, override: int | None, require_rsrc: bool) -> str | None:
    try:
        info = parse_pe(path.read_bytes())
        expected = expected_subsystem(path.stem, override)
    except (OSError, PeError) as error:
        return f"{path}: {error}"
    actual = info["subsystem"]
    if actual != expected:
        return f"{path}: 子系统为 {actual}（{SUBSYSTEM_NAMES.get(actual, '未知')}），预期 {expected}"
    has_rsrc = ".rsrc" in info["sections"]
    if require_rsrc and not has_rsrc:
        return f"{path}: 缺少 .rsrc 段，图标或版本资源未嵌入"
    rsrc = "含 .rsrc" if has_rsrc else "无 .rsrc"
    print(f"通过 {path}: 子系统 {actual}（{SUBSYSTEM_NAMES[actual]}），{rsrc}")
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binaries", nargs="+", type=Path)
    parser.add_argument("--subsystem", choices=["gui", "console"], help="覆盖按文件名推断的预期子系统")
    parser.add_argument("--require-rsrc", action="store_true", help="对所有二进制要求 .rsrc 段")
    args = parser.parse_args()
    override = {"gui": 2, "console": 3}.get(args.subsystem) if args.subsystem else None
    failures: list[str] = []
    for path in args.binaries:
        require_rsrc = args.require_rsrc or path.stem.startswith("bt-gui")
        message = verify(path, override, require_rsrc)
        if message:
            failures.append(message)
    for message in failures:
        print(f"失败 {message}", file=sys.stderr)
    if failures:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
