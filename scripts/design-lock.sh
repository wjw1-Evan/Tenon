#!/usr/bin/env bash
# 共享设计文档并发编辑锁（v1.191）。
#
# 背景：docs/design.md / docs/design-changelog.md / AGENTS.md 是并行 agent 会话的
# 共同必改文件——Edit 锚点漂移、design.md 头部版本行互相覆写、changelog 表尾追加
# 竞态频发。协议：改动前取锁（标签取本次版本号，如 v1.191），锁内完成
# 「读 changelog 表尾定版本 → 追加演进行 → 同步 design.md 头部元数据」，
# 改毕立即释放；锁仅覆盖文档编辑窗口，不跨构建 / 测试。
#
# 用法：
#   design-lock.sh acquire <标签> [--timeout 秒]  取锁；被持则等待至超时（默认 120s）；陈旧锁自动接管
#   design-lock.sh release <标签> [--force]       释放；标签须与 acquire 一致
#   design-lock.sh refresh <标签>                 续期（长编辑窗口防陈旧接管）
#   design-lock.sh status                         查看持锁者
#   design-lock.sh with-lock <标签> -- <命令...>  取锁执行命令，退出（含失败）自动释放
#   design-lock.sh selftest                       并发自测（隔离临时锁目录，不触真实锁）
#
# 环境变量：
#   TENON_DOC_LOCK_DIR    锁目录（默认 <仓库根>/.git/tenon-doc-lock，位于 .git 内不入库）
#   TENON_DOC_LOCK_STALE  陈旧阈值秒数（默认 900；超时未续期可被接管）
#   TENON_DOC_LOCK_TIMEOUT acquire 等待秒数（默认 120）
#   TENON_DOC_LOCK_POLL   轮询间隔秒数（默认 2）
#   TENON_DOC_LOCK_SETUP_GRACE  建锁宽限秒数（默认 10）
set -euo pipefail

LOCK_DIR="${TENON_DOC_LOCK_DIR:-}"
if [[ -z "$LOCK_DIR" ]]; then
  repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
  LOCK_DIR="$repo_root/.git/tenon-doc-lock"
fi
STALE="${TENON_DOC_LOCK_STALE:-900}"
TIMEOUT="${TENON_DOC_LOCK_TIMEOUT:-120}"
POLL="${TENON_DOC_LOCK_POLL:-2}"
# 建锁宽限秒数：mkdir 成功到 holder 落盘之间存在毫秒级窗口，期间 holder 缺失属正常；
# 超过宽限期仍无 holder 才判定为崩溃残留并接管
LOCK_SETUP_GRACE="${TENON_DOC_LOCK_SETUP_GRACE:-10}"
HOLDER="$LOCK_DIR/holder"

usage() {
  cat >&2 <<'EOF'
用法: design-lock.sh <命令> [参数]
  acquire <标签> [--timeout 秒]  取锁；被持则等待至超时；陈旧锁自动接管
  release <标签> [--force]       释放；标签须与 acquire 一致
  refresh <标签>                 续期（长编辑窗口防陈旧接管）
  status                         查看持锁者
  with-lock <标签> -- <命令...>  取锁执行命令，退出（含失败）自动释放
  selftest                       并发自测（隔离临时锁目录）
EOF
  exit 2
}

now() { date +%s; }

# 读持锁者记录，格式「标签|获取纪元秒」；无记录输出空
read_holder() { cat "$HOLDER" 2>/dev/null || true; }

holder_label() {
  local h; h="$(read_holder)"
  printf '%s' "${h%%|*}"
}

is_epoch() { [[ "${1:-}" =~ ^[0-9]+$ ]]; }

# 锁目录创建时间（纪元秒，可能带小数由调用方截断）——BSD/GNU stat 兼容
dir_mtime() {
  stat -f %m "$LOCK_DIR" 2>/dev/null || stat -c %Y "$LOCK_DIR" 2>/dev/null || true
}

cmd_acquire() {
  local label="" timeout="$TIMEOUT"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --timeout) timeout="$2"; shift 2 ;;
      *) label="$1"; shift ;;
    esac
  done
  if [[ -z "$label" ]]; then echo "用法：acquire <标签> [--timeout 秒]" >&2; exit 2; fi

  local t deadline held_label held_epoch age h dmt
  t="$(now)"
  deadline=$(( t + timeout ))
  while :; do
    t="$(now)"
    if mkdir "$LOCK_DIR" 2>/dev/null; then
      # 抢到目录只算一半：holder 落盘成功才算持锁（目录被并发接管者删除则重来）
      local tmp="$LOCK_DIR/.tmp.$$" epoch
      epoch="$(now)"
      if printf '%s|%s\n' "$label" "$epoch" > "$tmp" 2>/dev/null \
         && mv "$tmp" "$HOLDER" 2>/dev/null \
         && [[ "$(read_holder)" == "$label|$epoch" ]]; then
        echo "已取得文档锁：标签 ${label}（$(basename "$LOCK_DIR")）"
        return 0
      fi
      continue
    fi
    # 目录已存在：同标签重入幂等 / 建锁进行中宽限 / 崩溃残留与陈旧锁接管 / 等待
    h="$(read_holder)"
    held_label="${h%%|*}"
    held_epoch="${h##*|}"
    if [[ "$held_label" == "$label" ]]; then
      echo "已持有文档锁（重入）：标签 $label"
      return 0
    fi
    if ! is_epoch "$held_epoch"; then
      # holder 缺失：建锁进行中（毫秒级窗口）或崩溃残留——目录年龄在宽限期内按忙等待
      dmt="$(dir_mtime)"; dmt="${dmt%%.*}"
      if is_epoch "$dmt" && (( t - dmt <= LOCK_SETUP_GRACE )); then
        if ! busy_wait; then return 1; fi
        continue
      fi
      echo "接管残缺文档锁（holder 缺失或损坏，原持锁者：${held_label:-<未知>}）" >&2
      rm -rf "$LOCK_DIR"
      continue
    fi
    age=$(( t - held_epoch ))
    if [[ "$age" -gt "$STALE" ]]; then
      echo "接管陈旧文档锁（原持锁者：${held_label:-<未知>}，${age}s 未续期 > ${STALE}s）" >&2
      rm -rf "$LOCK_DIR"
      continue
    fi
    if ! busy_wait; then return 1; fi
  done
}

# 忙等待一拍；到死线则报忙并返回 1（由调用方退出）
busy_wait() {
  if [[ "$t" -ge "$deadline" ]]; then
    echo "文档锁被持有：${held_label:-<未知>}（已等 ${timeout}s）。稍后重试，或 status 查看" >&2
    return 1
  fi
  sleep "$POLL"
}

cmd_release() {
  local label="" force=0
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --force) force=1; shift ;;
      *) label="$1"; shift ;;
    esac
  done
  if [[ -z "$label" && "$force" -eq 0 ]]; then echo "用法：release <标签> [--force]" >&2; exit 2; fi
  if [[ ! -d "$LOCK_DIR" ]]; then echo "无文档锁可释放"; return 0; fi
  local held_label; held_label="$(holder_label)"
  if [[ "$force" -eq 0 && "$held_label" != "$label" ]]; then
    echo "拒绝释放：持锁者是「${held_label:-<未知>}」而非「${label}」（确需强制加 --force）" >&2
    return 1
  fi
  rm -rf "$LOCK_DIR"
  echo "已释放文档锁（标签 ${held_label:-<未知>}）"
}

cmd_refresh() {
  local label="${1:-}"
  if [[ -z "$label" ]]; then echo "用法：refresh <标签>" >&2; exit 2; fi
  if [[ ! -d "$LOCK_DIR" ]]; then echo "无文档锁可续期" >&2; return 1; fi
  local held_label; held_label="$(holder_label)"
  if [[ "$held_label" != "$label" ]]; then
    echo "拒绝续期：持锁者是「${held_label:-<未知>}」而非「${label}」" >&2
    return 1
  fi
  local tmp="$LOCK_DIR/.tmp.$$"
  printf '%s|%s\n' "$label" "$(now)" > "$tmp"
  mv "$tmp" "$HOLDER"
  echo "已续期文档锁：标签 $label"
}

cmd_status() {
  if [[ ! -d "$LOCK_DIR" ]]; then echo "文档锁空闲"; return 0; fi
  local h t
  h="$(read_holder)"
  held_label="${h%%|*}"
  held_epoch="${h##*|}"
  if ! is_epoch "$held_epoch"; then
    echo "文档锁被占（holder 残缺，下次 acquire 将接管）：${held_label:-<未知>}"
    return 0
  fi
  t="$(now)"
  age=$(( t - held_epoch ))
  if [[ "$age" -gt "$STALE" ]]; then
    echo "文档锁被持有：${held_label:-<未知>}，已 ${age}s（已陈旧，可被接管）"
  else
    echo "文档锁被持有：${held_label:-<未知>}，已 ${age}s"
  fi
}

cmd_with_lock() {
  local label="${1:-}"
  if [[ -z "$label" ]]; then echo "用法：with-lock <标签> -- <命令...>" >&2; exit 2; fi
  shift
  if [[ "${1:-}" == "--" ]]; then shift; fi
  if [[ $# -eq 0 ]]; then echo "用法：with-lock <标签> -- <命令...>" >&2; exit 2; fi
  cmd_acquire "$label"
  trap 'rm -rf "$LOCK_DIR" 2>/dev/null || true' EXIT INT TERM
  "$@"
}

cmd_selftest() {
  local self="$1"
  local td; td="$(mktemp -d "${TMPDIR:-/tmp}/tenon-doc-lock-selftest.XXXXXX")"
  # STALE 须大于 TIMEOUT：等待者不得把他人刚取得的锁误判陈旧；陈旧接管单独用回拨纪元测
  export TENON_DOC_LOCK_DIR="$td/lock"
  export TENON_DOC_LOCK_STALE=60 TENON_DOC_LOCK_POLL=1 TENON_DOC_LOCK_TIMEOUT=2
  local pass=0 fail=0
  expect_ok() {
    if "$self" "$@" >/dev/null 2>&1; then pass=$((pass + 1)); else
      fail=$((fail + 1)); echo "FAIL(应成功): $*"; fi
  }
  expect_fail() {
    if "$self" "$@" >/dev/null 2>&1; then
      fail=$((fail + 1)); echo "FAIL(应失败): $*"; else pass=$((pass + 1)); fi
  }

  expect_ok acquire A                 # 取锁
  expect_fail acquire B               # 他人取锁等满超时被拒（STALE 未到不接管）
  expect_ok acquire A                 # 同标签重入幂等
  expect_fail release B               # 错标签拒释放
  expect_ok status                    # status 可查看
  expect_ok release A                 # 正确释放
  expect_ok acquire A                 # 释放后可复得
  expect_ok refresh A                 # 续期
  expect_ok release A
  expect_ok acquire A
  local now_s backdated
  now_s="$(date +%s)"
  backdated=$(( now_s - 100 ))
  printf 'A|%s\n' "$backdated" > "$TENON_DOC_LOCK_DIR/holder"  # 回拨纪元伪造陈旧
  expect_ok acquire B                 # 陈旧锁自动接管
  expect_ok release B
  expect_ok with-lock C -- true       # with-lock 正常命令
  expect_fail with-lock C -- false    # 失败命令传播非零退出码
  expect_ok status                    # with-lock 退出后锁已释放
  # 并发争抢：8 路同时取锁，恰 1 路成功（子壳内关 set -e 以记录退出码）
  local i winners=0
  for i in 1 2 3 4 5 6 7 8; do
    ( set +e; "$self" acquire "R$i" >/dev/null 2>&1; echo $? > "$td/rc$i" ) &
  done
  wait
  for i in 1 2 3 4 5 6 7 8; do
    if [[ "$(cat "$td/rc$i")" == "0" ]]; then winners=$((winners + 1)); fi
  done
  if [[ "$winners" -eq 1 ]]; then pass=$((pass + 1)); else
    fail=$((fail + 1)); echo "FAIL(并发争抢应恰 1 胜): 实际 $winners 胜"; fi
  expect_ok release cleanup --force   # 强制释放兜底

  rm -rf "$td"
  echo "selftest：$pass 过 / $fail 败"
  if [[ "$fail" -eq 0 ]]; then exit 0; fi
  exit 1
}

main() {
  local cmd="${1:-}"
  if [[ -z "$cmd" ]]; then usage; fi
  case "$cmd" in
    acquire)  shift; cmd_acquire "$@" ;;
    release)  shift; cmd_release "$@" ;;
    refresh)  shift; cmd_refresh "$@" ;;
    status)   cmd_status ;;
    with-lock) shift; cmd_with_lock "$@" ;;
    selftest) cmd_selftest "$(cd "$(dirname "$0")" && pwd)/$(basename "$0")" ;;
    help|-h|--help) usage ;;
    *) echo "未知命令：$cmd" >&2; usage ;;
  esac
}

main "$@"
