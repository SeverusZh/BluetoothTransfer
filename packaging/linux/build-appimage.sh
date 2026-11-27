#!/bin/sh
# 构建 AppImage：需要 appimagetool；缺失时明确失败，不静默跳过。
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
VERSION=${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)}
ARCH=${ARCH:-x86_64}
BUILD_DIR=${BUILD_DIR:-$ROOT_DIR/target/release}
OUT_DIR=${OUT_DIR:-$ROOT_DIR/dist}
APPDIR="$OUT_DIR/appimage/BluetoothTransfer.AppDir"
TOOL=${APPIMAGETOOL:-}

if [ -z "$VERSION" ]; then
  echo "无法确定版本号：请设置 VERSION" >&2
  exit 1
fi
for binary in bt-gui bt btrecv; do
  if [ ! -x "$BUILD_DIR/$binary" ]; then
    echo "缺少 $BUILD_DIR/$binary，请先 cargo build --release" >&2
    exit 1
  fi
done

if [ -z "$TOOL" ]; then
  TOOL=$(command -v appimagetool || true)
fi
if [ -z "$TOOL" ] || [ ! -x "$TOOL" ]; then
  echo "未找到 appimagetool：请设置 APPIMAGETOOL，或从 https://github.com/AppImage/appimagetool/releases 下载" >&2
  exit 1
fi

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications"
install -m 0755 "$BUILD_DIR/bt-gui" "$APPDIR/usr/bin/bt-gui"
install -m 0755 "$BUILD_DIR/bt" "$APPDIR/usr/bin/bt"
install -m 0755 "$BUILD_DIR/btrecv" "$APPDIR/usr/bin/btrecv"
install -m 0755 "$SCRIPT_DIR/AppRun" "$APPDIR/AppRun"
install -m 0644 "$SCRIPT_DIR/bluetooth-transfer.desktop" "$APPDIR/usr/share/applications/bluetooth-transfer.desktop"
install -m 0644 "$SCRIPT_DIR/bluetooth-transfer.desktop" "$APPDIR/bluetooth-transfer.desktop"
install -m 0644 "$SCRIPT_DIR/icons/bluetooth-transfer-256.png" "$APPDIR/bluetooth-transfer.png"
install -m 0644 "$SCRIPT_DIR/icons/bluetooth-transfer-256.png" "$APPDIR/.DirIcon"

mkdir -p "$OUT_DIR"
ARCH="$ARCH" "$TOOL" --no-appstream "$APPDIR" "$OUT_DIR/BluetoothTransfer-${VERSION}-${ARCH}.AppImage"
echo "已生成 $OUT_DIR/BluetoothTransfer-${VERSION}-${ARCH}.AppImage"