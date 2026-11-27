#!/bin/sh
set -eu

dir=${1:-"$HOME/Downloads/BluetoothReceive"}

# 无人值守接收必须显式 --no-ask；Ctrl+C 干净退出并输出退出摘要
exec bt recv --dir "$dir" --no-ask --events
