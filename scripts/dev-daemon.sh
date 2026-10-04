#!/bin/sh
# Tenon 开发 daemon（v1.65 热重载链路）：
# - 固定 127.0.0.1:9876 + token "dev"（ui/src/main.tsx 的 DEV 回落约定，
#   页面经 CORS 白名单跨源调用，Vite 代理不参与）
# - HOME 隔离到 .tenon-dev/：不污染真实 daemon 数据 / endpoint 文件 / 单实例锁
# - 监听 crates/**.rs：自动 cargo build + 重启；端口与 token 固定，
#   UI 免刷新自动重连（会话与项目状态全在 daemon + 磁盘，§6.2）
#
# 独立使用：pnpm dev:daemon；一键（daemon + Vite HMR）：pnpm dev
set -u
cd "$(dirname "$0")/.."
ROOT="$(pwd)"

PORT="${TENON_DEV_PORT:-9876}"
TOKEN="${TENON_DEV_TOKEN:-dev}"
DEV_HOME="$ROOT/.tenon-dev/home"
STAMP="$ROOT/.tenon-dev/build-stamp"
mkdir -p "$DEV_HOME"

child=""
cleanup() {
    [ -n "$child" ] && kill "$child" 2>/dev/null
    exit 0
}
trap cleanup INT TERM

build_and_start() {
    if [ -n "$child" ]; then
        kill "$child" 2>/dev/null
        wait "$child" 2>/dev/null
        child=""
    fi
    echo "[dev-daemon] cargo build -p tenon-daemon ..."
    if cargo build -p tenon-daemon --bin tenon-daemon; then
        HOME="$DEV_HOME" "$ROOT/target/debug/tenon-daemon" \
            --project . --no-lock --port "$PORT" --token "$TOKEN" "$@" &
        child=$!
        echo "[dev-daemon] 运行中 http://127.0.0.1:${PORT}（token=${TOKEN}，pid ${child}）"
    else
        echo "[dev-daemon] 编译失败；修正后保存文件自动重试" >&2
    fi
}

# 仓库根 config.local.toml（本地模型接入，不入库）存在则显式传入——
# HOME 已隔离，daemon 默认发现链找不到它
if [ -f "$ROOT/config.local.toml" ]; then
    build_and_start --config "$ROOT/config.local.toml"
else
    build_and_start
fi

touch "$STAMP"
while :; do
    changed=$(find crates -name '*.rs' -newer "$STAMP" -print -quit 2>/dev/null)
    if [ -n "$changed" ]; then
        # 先挪标记再构建：构建期间的新改动留给下一轮，不丢
        touch "$STAMP"
        if [ -f "$ROOT/config.local.toml" ]; then
            build_and_start --config "$ROOT/config.local.toml"
        else
            build_and_start
        fi
    fi
    sleep 1
done
