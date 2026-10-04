#!/bin/sh
# Tenon 开发热重载一键启动（v1.65）：
#   后台：dev daemon（固定 127.0.0.1:9876 + token dev，Rust 改动自动重建重启，
#         见 dev-daemon.sh）
#   前台：Vite dev server（UI 保存即 HMR）
# 打开 http://localhost:5173 —— UI 的 DEV 回落握手自动连上 daemon。
set -u
cd "$(dirname "$0")/.."

sh scripts/dev-daemon.sh &
watcher=$!
cleanup() {
    kill "$watcher" 2>/dev/null
    exit 0
}
trap cleanup INT TERM

pnpm --filter tenon-ui dev
cleanup
