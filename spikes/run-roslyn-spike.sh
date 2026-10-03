#!/usr/bin/env bash
# Roslyn LS 沙箱化 spike（附录 C Q2）——自动化 harness
# 用法: ./run-roslyn-spike.sh <vsix-path> <workspace-dir>
set -u
VSIX="${1:?用法: $0 <vsix-path> <workspace-dir>}"
WS="${2:-/tmp/spike-workspace}"

result="{\"spike\":\"roslyn\",\"vsix\":\"$VSIX\",\"steps\":{"
fail=0

# 1. 解包
UNPACK_DIR=$(mktemp -d)
if unzip -oq "$VSIX" -d "$UNPACK_DIR" 2>/dev/null; then
  result+="\"unpack\":\"pass\","
else
  result+="\"unpack\":\"fail: unzip 失败（VSIX 损坏？）\"}"; echo "$result"; exit 1
fi

# 2. 定位 LS 入口
LS_FILE=$(find "$UNPACK_DIR" -name "Microsoft.CodeAnalysis.LanguageServer*" | head -1)
if [ -z "$LS_FILE" ]; then
  result+="\"locate\":\"fail: 未找到 Microsoft.CodeAnalysis.LanguageServer*\"}"; echo "$result"; exit 1
fi
LS_DIR=$(dirname "$LS_FILE")
result+="\"locate\":\"pass\",\"ls_dir\":\"$LS_DIR\","

# 3. 握手 + 沙箱化（经 cargo run 例程；见 tenon-lsp examples/roslyn_spike.rs）
mkdir -p "$WS"
cd "$(dirname "$0")/.."
OUT=$(cargo run -q -p tenon-lsp --example roslyn_spike -- "$LS_DIR" "$WS" 2>&1)
if echo "$OUT" | grep -q "SPIKE_PASS"; then
  result+="\"handshake_sandboxed\":\"pass\","
else
  result+="\"handshake_sandboxed\":\"fail: $OUT\","
  fail=1
fi

# 4. 守卫冒烟（常规测试覆盖铁律七）
if cargo test -q -p tenon-lsp --test host_tests 2>/dev/null | grep -q "test result: ok"; then
  result+="\"guard_smoke\":\"pass\""
else
  result+="\"guard_smoke\":\"fail\""; fail=1
fi

result+="},\"verdict\":\"$([ $fail -eq 0 ] && echo PASS || echo FAIL)\"}"
echo "$result"
