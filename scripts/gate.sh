#!/bin/sh
# Tenon 本地门禁一键跑齐（CI 同款 + 平台盲区增强）：
#   scripts/gate.sh          # fmt + clippy（主机 + Linux 交叉白名单）+ cargo test + vitest + ui:build
#   scripts/gate.sh --full   # 追加 Playwright 真 daemon E2E（CI e2e 同款，需已 playwright install chromium）
#   scripts/gate.sh --head   # 在 HEAD 干净 worktree 上跑 cargo 门禁——验证「提交内容本身」过门禁
#
# 背景（v1.211 三连红 postmortem，2026-10-09）：
#   1) linux.rs 等平台门控代码在 macOS 本地 clippy 永远不编译——错误只在 CI Linux 端爆出，
#      本地推演不出 → 交叉 clippy 白名单补盲（撞 C 依赖的 crate 交叉检查不可行，
#      留给 CI ubuntu native 全图 clippy + fail-fast:false + 推送后 watch 三层兜底）；
#   2) 共享工作树下「测的是 worktree、推的是 HEAD」会脱节（v1.211 误提交陈旧块）
#      → --head 模式对 HEAD 提交树跑门禁，隔离 index 提交后、推送前跑一次；
#   3) 本地门禁过 ≠ 远端 CI 绿——本脚本不豁免 AGENTS.md「推送后监测 CI 到绿」纪律。
#
# 交叉 clippy 白名单（x86_64-unknown-linux-gnu，纯 Rust 依赖链）：
#   tenon-sandbox（linux.rs 整文件 cfg 门控，上一次连红事故主角）、tenon-mcp。
#   其余 crate（agent / daemon / fs / models）依赖链含 cc-rs C 编译（需交叉 gcc），
#   macOS 本地交叉不可行——新 crate 想入列先跑：
#     cargo clippy -p <crate> --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
set -eu
cd "$(dirname "$0")/.."

MODE="default"
for arg in "$@"; do
    case "$arg" in
        --full) MODE="full" ;;
        --head) MODE="head" ;;
        *) echo "未知参数: $arg（支持 --full / --head）" >&2; exit 2 ;;
    esac
done

LINUX_TARGET="x86_64-unknown-linux-gnu"
CROSS_CLIPPY_CRATES="tenon-sandbox tenon-mcp"

# cargo 门禁三件套 + Linux 交叉盲区补查。$1 = 工作目录（主工作树或 --head 临时 worktree）。
# CARGO_TARGET_DIR 由调用方决定（--head 共享主 target 缓存，避免全量重编）。
run_cargo_gates() {
    (
        cd "$1"
        # CI 同款：tauri-build 校验 externalBin / resources 路径存在，双双占位
        #（已存在不覆盖——本地可能有真实构建产物）
        mkdir -p app/src-tauri/binaries ui/dist
        [ -f "app/src-tauri/binaries/tenon-daemon-$(rustc -vV | sed -n 's/host: //p')" ] || \
            echo placeholder > "app/src-tauri/binaries/tenon-daemon-$(rustc -vV | sed -n 's/host: //p')"
        [ -f ui/dist/index.html ] || echo placeholder > ui/dist/index.html

        echo "== fmt =="
        cargo fmt --all -- --check
        echo "== clippy（主机目标）=="
        cargo clippy --workspace --all-targets -- -D warnings
        echo "== clippy（Linux 交叉白名单：${CROSS_CLIPPY_CRATES}）=="
        rustup target list --installed | grep -q "$LINUX_TARGET" || rustup target add "$LINUX_TARGET"
        for crate in $CROSS_CLIPPY_CRATES; do
            cargo clippy -p "$crate" --all-targets --target "$LINUX_TARGET" -- -D warnings
        done
        echo "== cargo test（TENON_SKIP_LIVE=1，GLM 联调走 config.local.toml 专项）=="
        TENON_SKIP_LIVE=1 cargo test --workspace --no-fail-fast
    )
}

if [ "$MODE" = "head" ]; then
    # UI 门禁（vitest / build / e2e）不在此模式：node_modules 不随 worktree 复制，
    # 先在主工作树跑完 UI 门禁后，再用本模式验证 HEAD 的 Rust 内容。
    HEAD_SHA="$(git rev-parse HEAD)"
    WT="$(mktemp -d "${TMPDIR:-/tmp}/tenon-gate-head.XXXXXX")"
    cleanup() {
        git worktree remove --force "$WT" >/dev/null 2>&1 || rm -rf "$WT"
        git worktree prune >/dev/null 2>&1 || true
    }
    # EXIT trap 必须透传真实退出码（trap 内末命令成功会把失败洗成 0 假绿）
    trap 'rc=$?; cleanup; exit $rc' EXIT
    trap 'cleanup; exit 130' INT
    trap 'cleanup; exit 143' TERM
    git worktree add --detach "$WT" "$HEAD_SHA" >/dev/null
    echo "== HEAD 干净 worktree 门禁 @ ${HEAD_SHA}（提交内容本身，而非共享工作树现状）=="
    CARGO_TARGET_DIR="$(pwd)/target"
    export CARGO_TARGET_DIR
    run_cargo_gates "$WT"
    echo "HEAD @ ${HEAD_SHA} 门禁全过——提交内容与验证内容一致"
    exit 0
fi

run_cargo_gates "$(pwd)"

echo "== vitest =="
pnpm --filter tenon-ui test
echo "== ui:build =="
pnpm --filter tenon-ui build

if [ "$MODE" = "full" ]; then
    echo "== Playwright E2E（真 daemon）=="
    pnpm ui:test:e2e
fi

echo "本地门禁全过（mode=${MODE}）——推送后记得 gh run watch 盯 CI 到绿"
