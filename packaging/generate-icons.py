#!/usr/bin/env python3
"""生成托盘/安装器共用的品牌图标，不依赖第三方图形库。"""

import os
import struct
import sys
import zlib

BRAND = (15, 108, 189, 255)
WHITE = (255, 255, 255, 255)


def inside_rounded(x, y, size, radius):
    min_edge = 1.0
    max_edge = size - 1.0
    if x < min_edge or y < min_edge or x > max_edge or y > max_edge:
        return False
    cx = min(max(x, radius), size - radius)
    cy = min(max(y, radius), size - radius)
    dx = x - cx
    dy = y - cy
    return dx * dx + dy * dy <= radius * radius


def draw_line(pixels, size, start, end, thickness, color):
    x0, y0 = start
    x1, y1 = end
    dx = x1 - x0
    dy = y1 - y0
    length_squared = dx * dx + dy * dy
    for y in range(size):
        for x in range(size):
            px = x + 0.5
            py = y + 0.5
            if length_squared == 0:
                t = 0.0
            else:
                t = max(0.0, min(1.0, ((px - x0) * dx + (py - y0) * dy) / length_squared))
            closest_x = x0 + t * dx
            closest_y = y0 + t * dy
            distance = ((px - closest_x) ** 2 + (py - closest_y) ** 2) ** 0.5
            if distance <= thickness / 2.0:
                index = (y * size + x) * 4
                pixels[index:index + 4] = bytes(color)


def render(size):
    pixels = bytearray(size * size * 4)
    radius = size * 0.22
    for y in range(size):
        for x in range(size):
            if inside_rounded(x + 0.5, y + 0.5, size, radius):
                index = (y * size + x) * 4
                pixels[index:index + 4] = bytes(BRAND)
    scale = size / 32.0
    points = [
        (16.0, 6.0),
        (24.0, 12.5),
        (16.0, 18.0),
        (24.0, 23.5),
        (16.0, 28.0),
        (16.0, 6.0),
        (16.0, 28.0),
    ]
    for start, end in zip(points, points[1:]):
        draw_line(
            pixels,
            size,
            (start[0] * scale, start[1] * scale),
            (end[0] * scale, end[1] * scale),
            max(1.0, 2.1 * scale),
            WHITE,
        )
    return pixels


def png_bytes(size, pixels):
    raw = bytearray()
    for y in range(size):
        raw.append(0)
        offset = y * size * 4
        raw.extend(pixels[offset:offset + size * 4])

    def chunk(kind, data):
        return (
            struct.pack('>I', len(data))
            + kind
            + data
            + struct.pack('>I', zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    header = struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0)
    return (
        b'\x89PNG\r\n\x1a\n'
        + chunk(b'IHDR', header)
        + chunk(b'IDAT', zlib.compress(bytes(raw), 9))
        + chunk(b'IEND', b'')
    )


def ico_bytes(images):
    count = len(images)
    header = struct.pack('<HHH', 0, 1, count)
    offset = 6 + 16 * count
    entries = bytearray()
    payload = bytearray()
    for size, data in images:
        entries.extend(struct.pack('<BBBBHHII', size % 256, size % 256, 0, 0, 1, 32, len(data), offset))
        payload.extend(data)
        offset += len(data)
    return header + bytes(entries) + bytes(payload)


def dib_entry(size, rgba):
    header = struct.pack('<IiiHHIIiiII', 40, size, size * 2, 1, 32, 0, size * size * 4, 0, 0, 0, 0)
    body = bytearray()
    for y in range(size - 1, -1, -1):
        row = rgba[y * size * 4:(y + 1) * size * 4]
        for x in range(size):
            index = x * 4
            body.extend((row[index + 2], row[index + 1], row[index], row[index + 3]))
    mask_row = ((size + 31) // 32) * 4
    mask = bytes(mask_row * size)
    return header + bytes(body) + mask


def main():
    root = os.path.dirname(os.path.abspath(__file__))
    windows = os.path.join(root, 'windows')
    linux = os.path.join(root, 'linux')
    os.makedirs(windows, exist_ok=True)
    os.makedirs(os.path.join(linux, 'icons'), exist_ok=True)

    master = 256
    large = render(master)
    with open(os.path.join(linux, 'icons', 'bluetooth-transfer-256.png'), 'wb') as handle:
        handle.write(png_bytes(master, large))

    sizes = [16, 32, 48, 64, 128, 256]
    images = []
    for size in sizes:
        pixels = render(size)
        data = png_bytes(size, pixels)
        images.append((size, dib_entry(size, pixels)))
        if size in (16, 32, 48, 256):
            with open(os.path.join(linux, 'icons', f'bluetooth-transfer-{size}.png'), 'wb') as handle:
                handle.write(data)

    with open(os.path.join(windows, 'app.ico'), 'wb') as handle:
        handle.write(ico_bytes(images))
    print('icons generated')


if __name__ == '__main__':
    sys.exit(main())