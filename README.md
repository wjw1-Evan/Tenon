# Tenon

> 开源的 Agent 优先桌面开发环境：左边是完整编辑器（智能提示 / 诊断 / 重构），右边是自主干活的代理；人改与 AI 改实时互见，全程沙箱隔离、证据可查、随时回滚。

官网 [tenonide.dev](https://tenonide.dev) · macOS 为主平台（Windows 走 WSL2 路径） · Apache-2.0

## 核心特性

- **共生而非外挂**：代理与编辑器共享同一套 LSP 活实例 / 诊断 / 语言包——一次索引、行为一致，编辑器即审查界面；
- **本地优先隐私**：会话 / Trace / 索引全本地，遥测默认零上报，云模型调用按用户显式配置并明示；
- **多模型、成本可控**：BYOK + 显式路由 + OpenAI 兼容端点 + 任务级成本归因；
- **多项目运行模型**：同一 daemon 管理多个已登记项目，会话 / 文件 / LSP / 事件按 `project_id` 隔离；
- **审批负担最低且安全不缩水**：动作分级 + 沙箱三态 + 熔断器 + 随时回滚——安全动作自动执行，危险动作才打扰。

## 文档

**[docs/design.md](docs/design.md) 是唯一开发依据**：产品定位、架构、安全模型、数据与 API、路线图与测试策略均以设计方案为准；**版本演进表**记录全部设计变更（递增版本号 + 一行要点）。

开发约定：实现与设计冲突时**先更新 design.md、再写代码**，不允许静默偏离；代码与设计不一致视为缺陷，改代码或改设计后必须收敛。

## 快速开始

```bash
pnpm install

# Web 一键启动（v1.76，参考 opencode web）：构建 UI → daemon 同源托管 → 自动开浏览器
pnpm web

# 日常开发（v1.68）：dev daemon 固定 127.0.0.1:9876（Rust 改动自动重建重启）+ Vite HMR
pnpm dev             # 浏览器裸开 http://localhost:5173，免参直连
pnpm ui              # 仅 UI 侧迭代（配合 pnpm dev:daemon）

# 桌面壳（仅壳链路验证：sidecar 握手 / 原生目录对话框 / WebView 导航）
cargo build -p tenon-app && TENON_PROJECT=<项目路径> ./target/debug/tenon-app

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

**M0 已验收**：附录 D 基准 10 任务接 GLM 真实模型 **8/10 通过、安全违规 0**（基线见 `evals/baseline-glm.json`）；**M1 / M2 / M3 全部实现**（多项目、语言包、沙箱三态、快照回滚、模型路由、Laya 决策模型、registry / MCP / 并行子代理、Evals、团队策略、局域网配对、Windows WSL2 安装脚本）。各版本变更明细见设计方案「版本演进」表（当前 v1.85）。

测试：**Rust 358 + performance gates 3 + Vitest 134 全绿**；clippy 0 警告；Playwright 真 daemon E2E 覆盖多项目隔离与冷启动预算。

## License

Apache-2.0
