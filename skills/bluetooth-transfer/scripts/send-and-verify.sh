#!/bin/sh
set -eu

if [ "$#" -lt 2 ]; then
    echo "用法：send-and-verify.sh <ADDR> <PATH> [send 额外参数...]" >&2
    exit 2
fi

device=$1
path=$2
shift 2

# 助手通道不可用不阻断 OPP 发送
bt devices detect "$device" --json >/dev/null 2>&1 || true

result=$(bt send file "$device" "$path" "$@" --json)
printf '%s\n' "$result"

case "$result" in
    *'"ok":true'*) ;;
    *)
        echo "发送失败：检查 error.kind 与 error.hint" >&2
        exit 1
        ;;
esac

bt records list --direction send --limit 1 --json
