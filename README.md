# Tenon

> 开源的 Agent 优先桌面开发环境：左边是完整编辑器（智能提示 / 诊断 / 重构），右边是自主干活的代理；人改与 AI 改实时互见，全程沙箱隔离、证据可查、随时回滚。

官网 [tenonide.dev](https://tenonide.dev) · macOS 为主平台（Windows 走 WSL2 路径） · Apache-2.0

## 核心特性

- **共生而非外挂**：代理与编辑器共享同一套 LSP 活实例 / 诊断 / 语言包——一次索引、行为一致，编辑器即审查界面；
- **本地优先隐私**：会话 / Trace / 索引全本地，遥测默认零上报，云模型调用按用户显式配置并明示；
- **多模型、成本可控**：BYOK + 显式路由 + OpenAI 兼容端点 + 任务级成本归因；
- **多项目运行模型**：同一 daemon 管理多个已登记项目，会话 / 文件 / LSP / 事件按 `project_id` 隔离；
- **零审批且安全不缩水**：动作分级 + 沙箱三态 + 熔断器 + 对话节点随时回滚。

## 文档

**[docs/design.md](docs/design.md) 是唯一开发依据**：产品定位、架构、安全模型、数据与 API、路线图与测试策略均以设计方案为准；**[版本演进表](docs/design-changelog.md)**记录全部设计变更（递增版本号 + 一行要点）。

开发约定：实现与设计冲突时**先更新 design.md、再写代码**，不允许静默偏离；代码与设计不一致视为缺陷，改代码或改设计后必须收敛。

## 快速开始

```bash
pnpm install

# Web 一键启动（v1.76，参考 opencode web）：构建 UI → daemon 同源托管 → 自动开浏览器
pnpm web

# 日常开发（v1.68）：dev daemon 固定 127.0.0.1:9876（Rust 改动自动重建重启）+ Vite HMR
pnpm dev             # 浏览器裸开 http://localhost:5173，免参直连
pnpm ui              # 仅 UI 侧迭代（配合 pnpm dev:daemon）

# 桌面版：debug 壳启动时探测 5173 dev server——在线则 WebView 导航 Vite（HMR 直达桌面窗口），
# 离线回落 daemon 同源托管 ui/dist（生产同款链路）。tauri-build 的 externalBin 要求 sidecar
# 先就位（binaries/ 已 gitignore，复制方法与 release 流水线一致；daemon 重建后无需重拷——
# 运行时优先取 target/debug 同级新产物）
pnpm ui:build        # 首次运行需先产出 ui/dist（5173 dev server 在线时不需要）
cargo build -p tenon-daemon
mkdir -p app/src-tauri/binaries
cp target/debug/tenon-daemon "app/src-tauri/binaries/tenon-daemon-$(rustc -vV | sed -n 's/^host: //p')"
cargo build -p tenon-app && ./target/debug/tenon-app
# 可选：TENON_PROJECT=<路径> 指定首启项目（缺省当前目录）；桌面启动即开局域网访问（v1.157），
# macOS 首次会弹「接受传入网络连接」防火墙提示，属预期

# 测试与基准
cargo test --workspace                 # Rust 全量（含 GLM 真实端点联调：无 config.local.toml 自动跳过）
pnpm ui:test                           # 前端 vitest
pnpm ui:test:e2e                       # Playwright 真 daemon E2E
cargo run -p tenon-evals -- --provider glm   # Evals 基准（附录 D 10 任务）
```

模型接入：读 `~/.tenon/config.toml`（schema 见设计方案附录 E）；开发期联调可建仓库根 `config.local.toml`（已 git-ignore），`[models.providers.*]` 支持 `kind = "openai" | "anthropic" | "openai_responses"`。

## 技术栈

Rust 内核 / daemon · Tauri 2 桌面壳 · React + TS + Monaco · tree-sitter + LSP · SQLite + sqlite-vec · Seatbelt / Landlock / seccomp / WSL2 沙箱（详见设计方案 §16）

## 状态

**M0 已验收**：附录 D 基准 10 任务接 GLM 真实模型 **8/10 通过、安全违规 0**（基线见 `evals/baseline-glm.json`）；**M1 / M2 / M3 全部实现**（多项目、语言包、沙箱三态、快照回滚、模型路由、Laya 决策模型、registry / 并行子代理、Evals、团队策略、局域网配对、自动更新执行器、Windows WSL2 安装脚本）。

测试：**Rust 367 + performance gates 3 + Vitest 144 全绿**；clippy 0 警告；Playwright 真 daemon E2E 覆盖多项目隔离与冷启动预算。

## License

Apache-2.0
