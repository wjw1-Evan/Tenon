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

**M0 已验收**：附录 D 基准 10 任务接 GLM 真实模型 **8/10 通过、安全违规 0**（基线见 `evals/baseline-glm.json`）；**M1 / M2 / M3 全部实现**（多项目、语言包、沙箱三态、快照回滚、模型路由、Laya 决策模型、registry / 并行子代理、Evals、团队策略、局域网配对、自动更新执行器、Windows WSL2 安装脚本）。各版本变更明细见设计方案「版本演进」表（当前 v1.96：移除项目视图顶部搜索——撤销 v1.88「搜索 + 添加入口」中的搜索半边（登记项目数量级小、即时过滤无实用价值），工具栏仅存添加入口，过滤链路 / 空态 / 双语文案 / 样式整链删除；v1.95：术语统一——§4.2「国际化」更名「i18n」；v1.94：i18n 合规——Evals / AgentTrace / 语言包 / 冲突合并四面板接入双语文案（此前整组件绕过 t()），snapshot / store 死函数收尾；v1.93：断链接线实装——暂停改真挂起等待恢复（此前 resume 为空操作）并堵任务重入竞态、`set_readonly` 实装 + 命令面板只读开关、provider 可配单价接入成本归因与 token / 预算熔断、shadow 库按 `keep_days` 周期 gc、超期会话每日归档、审批事件 / 月日聚合 / Mode 等死符号与死配置字段清扫，已落地；v1.92：功能面收敛——移除 legacy 隐式项目端点 / 组合任务自循环 / Open VSX 实验 / 会话档位 / 遥测开关等无用功能，修复 `/project/:id/lsp` 假作用域，接线 Laya 会话注入与 AGENTS.md 项目规则，§9.8 收敛为三集成点）。

测试：**Rust 367 + performance gates 3 + Vitest 144 全绿**；clippy 0 警告；Playwright 真 daemon E2E 覆盖多项目隔离、全局活动条与冷启动预算。

## License

Apache-2.0
