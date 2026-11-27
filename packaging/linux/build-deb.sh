#!/bin/sh
# 构建 Debian 包：只读取 target/release 产物，不修改源码树。
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
VERSION=${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)}
ARCH=${ARCH:-$(dpkg --print-architecture)}
BUILD_DIR=${BUILD_DIR:-$ROOT_DIR/target/release}
OUT_DIR=${OUT_DIR:-$ROOT_DIR/dist}
STAGE="$OUT_DIR/deb/bluetooth-transfer_${VERSION}_${ARCH}"

if [ -z "$VERSION" ]; then
  echo "无法确定版本号：请设置 VERSION" >&2
  exit 1
fi

for binary in bt-gui bt btrecv; do
  if [ ! -x "$BUILD_DIR/$binary" ]; then
    echo "缺少 $BUILD_DIR/$binary，请先运行 cargo build --release -p bt-gui -p bt-cli" >&2
    exit 1
  fi
done

rm -rf "$STAGE"
mkdir -p "$STAGE/DEBIAN" \
  "$STAGE/usr/bin" \
  "$STAGE/usr/share/applications" \
  "$STAGE/usr/share/doc/bluetooth-transfer"

install -m 0755 "$BUILD_DIR/bt-gui" "$STAGE/usr/bin/bt-gui"
install -m 0755 "$BUILD_DIR/bt" "$STAGE/usr/bin/bt"
install -m 0755 "$BUILD_DIR/btrecv" "$STAGE/usr/bin/btrecv"
install -m 0644 "$SCRIPT_DIR/bluetooth-transfer.desktop" "$STAGE/usr/share/applications/bluetooth-transfer.desktop"
install -m 0644 "$ROOT_DIR/LICENSE" "$STAGE/usr/share/doc/bluetooth-transfer/copyright"

for size in 16 32 48 256; do
  icon_dir="$STAGE/usr/share/icons/hicolor/${size}x${size}/apps"
  mkdir -p "$icon_dir"
  install -m 0644 "$SCRIPT_DIR/icons/bluetooth-transfer-${size}.png" "$icon_dir/bluetooth-transfer.png"
done

cat > "$STAGE/DEBIAN/control" <<EOF
Package: bluetooth-transfer
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Maintainer: BlueToothTransfer <noreply@example.invalid>
Depends: libc6, libxkbcommon0, libx11-6, libxi6, libxcursor1, libxrandr2, libxinerama1, libgl1, libegl1, libwayland-client0, libdbus-1-3
Description: 通过蓝牙发送与接收文件
 提供 OPP 与私有助手通道发送、接收服务、传输记录与图形界面。
EOF

cat > "$STAGE/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
exit 0
EOF

cat > "$STAGE/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
exit 0
EOF

chmod 0755 "$STAGE/DEBIAN/postinst" "$STAGE/DEBIAN/postrm"
mkdir -p "$OUT_DIR"
dpkg-deb --build --root-owner-group "$STAGE" "$OUT_DIR/bluetooth-transfer_${VERSION}_${ARCH}.deb"
echo "已生成 $OUT_DIR/bluetooth-transfer_${VERSION}_${ARCH}.deb"