# Roslyn LS 沙箱化 spike 运行手册（附录 C Q2 / 设计方案 v1.13）

## 状态

**受阻**：开发环境（macOS）无 Roslyn VSIX 二进制。共享 LSP 宿主（tenon-lsp）、
沙箱（tenon-sandbox）与铁律七守卫基础设施已就绪——VSIX 可获取后执行本
harness 即可完成 spike（预计 <1 天）。

## 前置条件

1. 获取 Roslyn LS 官方 VSIX（Visual Studio Marketplace
   「Roslyn LanguageServer」或 Visual Studio 安装目录提取，锁定版本记录于此文件）；
2. Linux 环境（可选）：确认 dotnet runtime ≥ 8（`dotnet --info`）。

## 执行步骤

```bash
# 1. 解包 VSIX（zip 格式）
unzip -o Microsoft.VisualStudio.LanguageServer-<ver>.vsix -d /tmp/roslyn-ls

# 2. 定位 LSP 启动入口（VSIX 内 content/LanguageServer/ 下）
LS_DIR=$(dirname "$(find /tmp/roslyn-ls -name 'Microsoft.CodeAnalysis.LanguageServer.*' | head -1)")
echo "LS_DIR=$LS_DIR"

# 3. 沙箱化启动验证（tenon-lsp 宿主 + Linux 沙箱三件套 / macOS Seatbelt）
cargo run -p tenon-lsp --example roslyn_spike -- "$LS_DIR" /tmp/spike-workspace
#   预期：initialize 响应 capabilities；无沙箱违规输出

# 4. 铁律七守卫冒烟：对宿主下发 executeCommand（应 403/拒绝并记录）
#    —— 已由 crates/tenon-lsp/tests/host_tests.rs 常规覆盖

# 5. 结论记录
#    - 启动方式（dll vs 自包含可执行）
#    - 需要的 dotnet 版本
#    - 沙箱内行为差异（文件访问 / 网络尝试）
#    → 回填本文件与设计方案附录 C Q2「实施记录」
```

## 回退决定（已生效，v1.13）

按附录 C Q2 既定规则：spike 未能在 M1 完成 → **C# 语言包整体后移**，
不降级换 csharp-ls。C# 项目在本环境的语义支持恢复时间 = 本 spike 完成 +
语言包打包时间（预计 1-2 天）。

## spike harness

`spikes/run-roslyn-spike.sh <vsix-path> <workspace-dir>`：
自动化步骤 1-4，输出结构化结论（JSON）供回填。
