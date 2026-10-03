# Tenon Windows 安装脚本（设计方案 §12.3 / M3 交付：Windows 走 WSL2）
# 用法（管理员 PowerShell）：
#   Set-ExecutionPolicy Bypass -Scope Process -Force
#   .\scripts\install-windows.ps1
#
# 流程：
#   1. 检查/启用 WSL2（无则引导 wsl --install，需重启）
#   2. 在 WSL2 Ubuntu 内安装依赖（node/pnpm/rust + git）
#   3. 构建 daemon 与 UI（WSL2 内；文件系统建议置于 WSL FS 以获得性能，§21）
#   4. 输出 Tauri Windows 包构建指引（后续 `pnpm tauri build`）

$ErrorActionPreference = "Stop"

function Info($msg) { Write-Host "[tenon] $msg" -ForegroundColor Cyan }

# 1. WSL2 检查
Info "检查 WSL2..."
$wsl = Get-Command wsl -ErrorAction SilentlyContinue
if (-not $wsl) {
    Info "未检测到 WSL，执行 wsl --install（完成后请重启并重跑本脚本）"
    wsl --install --no-launch
    exit 0
}
$defaultDistro = (wsl --list --quiet | Select-Object -First 1)
if (-not $defaultDistro) {
    Info "WSL 无发行版，安装 Ubuntu..."
    wsl --install -d Ubuntu --no-launch
    exit 0
}

# 2. WSL 内依赖
Info "在 WSL2 内安装构建依赖..."
wsl bash -lc "sudo apt-get update -y && sudo apt-get install -y curl build-essential git libwebkit2gtk-4.1-dev libssl-dev"
wsl bash -lc "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
wsl bash -lc "command -v node >/dev/null || (curl -fsSL https://deb.nodesource.com/setup_22.x | sudo bash - && sudo apt-get install -y nodejs)"
wsl bash -lc "npm install -g pnpm@10"

# 3a. 优先使用预编译 daemon（dist/wsl2-installer/tenon-daemon，musl 静态链接）
$prebuilt = Join-Path $PSScriptRoot "dist\wsl2-installer\tenon-daemon"
if (Test-Path $prebuilt) {
    Info "使用预编译 daemon 二进制（复制进 WSL 主目录）..."
    wsl bash -lc "mkdir -p ~/tenon && cp /mnt/c/$(($prebuilt -replace '^([A-Za-z]):', '$1').Replace('\','/').SubString(3)) ~/tenon/tenon-daemon 2>/dev/null || cp $(($prebuilt -replace '^([A-Za-z]):', '/mnt/$1').Replace('\','/')) ~/tenon/tenon-daemon; chmod +x ~/tenon/tenon-daemon"
    Info "启动：wsl bash -c 'cd ~/tenon && ./tenon-daemon --db ~/tenon.db'"
    exit 0
}

# 3b. 无预编译产物 → 源码构建（假定仓库已克隆至 WSL 内 ~/tenon）
Info "构建 daemon + UI（WSL2）..."
wsl bash -lc "cd ~/tenon && ~/.cargo/bin/cargo build --release -p tenon-daemon --bin tenon-daemon && pnpm install && pnpm --filter tenon-ui build"

Info "构建完成。daemon 二进制：~/tenon/target/release/tenon-daemon"
Info "Windows 桌面包（可选）：wsl 内 pnpm tauri build，或在 Windows 侧使用 pnpm tauri build（需 Rust Windows 工具链）"
Info "注意：仓库建议置于 WSL 文件系统（\\wsl$\Ubuntu\home\<user>\tenon），NTFS 跨界 I/O 性能差（§21）"
