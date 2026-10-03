# Tenon

> 开源的 Agent 优先桌面开发环境：左边是完整编辑器（智能提示 / 诊断 / 重构），右边是自主干活的代理；人改与 AI 改实时互见，全程沙箱隔离、证据可查、随时回滚。

## 核心特性

- **共生而非外挂**：代理与编辑器共享同一套 LSP 活实例 / 诊断 / 语言包——一次索引、行为一致，编辑器即审查界面；
- **本地优先隐私**：会话 / Trace / 索引全本地，遥测默认零上报，云模型调用按用户显式配置并明示；
- **多模型、成本可控**：BYOK + 显式路由 + OpenAI 兼容端点 + 任务级成本归因；
- **审批负担最低且安全不缩水**：动作分级 + 沙箱三态 + 熔断器 + 随时回滚——安全动作自动执行，危险动作才打扰。

## 文档

- **[完整设计方案 v1.13](docs/design.md)** —— 产品定位、需求规格、架构、安全模型、编辑器与语言包、Agent 内核、数据与 API、路线图、测试策略

> v0.1 / v0.3 两轮评审（共 41 项）与 v1.0 复审（21 项）的结论已全部并入设计方案；Roslyn LS spike 受阻（无 VSIX），按附录 C Q2 回退规则 C# 包后移（v1.13 记录）。**平台优先级：macOS 为主**（2026-10-04 产品决策）——Windows 走 WSL2 路径以脚本 + 预编译 musl 二进制交付，端到端验证待 Windows 环境。
>
> 项目原名 OpenCodex，2026-10 定名 **Tenon**（沿革 OpenCodex → Weft → Tenon，官网 tenonide.dev）。

## 开发指南

**[docs/design.md](docs/design.md) 是程序开发的唯一依据**：功能范围、架构、接口、安全模型、数据结构与测试策略均以设计方案为准（设计方案前言亦声明其为 M0-M3 开发的唯一依据）。

开发过程中**允许并鼓励修改完善设计文件**，但须遵守：

- 实现中发现设计缺陷、遗漏或与实现冲突时，**先更新 design.md、再写代码**，不允许代码与设计静默偏离；
- 设计变更须在文档头部「版本演进」表中记录（递增版本号 + 一行要点）；涉及方向性取舍的，同步补充对应章节与 ADR；
- 设计与实现不一致视为缺陷：要么改代码对齐设计，要么改设计并说明理由，二者取一后必须收敛。

### 开发流程

1. 阅读设计文件中与本次任务相关的章节（实施：第 6-18 章；安全：第 12 章全篇；阅读路径见前言）；
2. 若设计有待完善之处，先修改 design.md 并记录版本演进；
3. 按设计实现，满足第 18 章分层测试与安全测试要求；
4. 交付时保持代码与设计文件同步变更。

## 技术栈

Rust 内核 / daemon · Tauri 2 桌面壳 · React + TS + Monaco · tree-sitter + LSP · SQLite + sqlite-vec · Seatbelt / seccomp / WSL2 沙箱（详见设计方案 §16）

## 状态

设计方案 v1.13（决策闭环；产品名 **Tenon**，官网 tenonide.dev）。**M0 已验收**：附录 D 基准 10 任务接 GLM 真实模型跑出基线 **8/10 通过（80%）≥ 50% 验收线，安全违规 0**（五指标基线见 `evals/baseline-glm.json`）。**M1 已实现**：语言包（tsserver / Pyright 共享 LSP 宿主 + 语义端点 + 铁律七守卫）；Laya 本地决策模型（三原语 + 五集成点 + registry 分发 + Evals 门「通过率不降、token 下降」）；沙箱三态（macOS Seatbelt + Linux Landlock/seccomp + §18.2 逃逸套件）；shadow git 快照（revert/restore/unrevert）+ 人机共编三方合并 + 崩溃恢复；模型路由（会话级热切换）+ 语言包向导。**M2 已实现**：官方静态 registry（§13.1 manifest + ed25519 签名 + 权限 diff + 保留字拦截）；MCP 外部进程插件（默认 D 级恒审批 / net:* → C）；并行子代理（worktree 隔离 + 不相交调度 + 复合 D 卡）；Open VSX 语言子集实验兼容 + `/lsp` codeaction；AgentTrace UI + 浏览器访问（CORS 白名单 + 配对入口）。**M3 已实现**：AI Evals 可视化 + 定时触发（`[evals].interval_hours`）；团队策略（`~/.tenon/policy.toml` 只收窄）；局域网配对（`--lan` + PairingStore 一次性码 + 可吊销令牌）；Windows WSL2 安装脚本（`scripts/install-windows.ps1`）。

测试：**Rust 278 + vitest 24 全绿**；clippy 0 警告；Linux x86_64 交叉检查通过。签名密钥 `--generate-keys` 生成 ed25519 对并接入 laya/registry 公钥解析链。

## 代码结构（monorepo，ADR-13）

```text
crates/
├── tenon-config     # config.toml schema（附录 E）
├── tenon-store      # SQLite：事件溯源 / 审批审计 / 成本归因 / 冷归档（§14.2）
├── tenon-core       # A/B/C/D 分级 / 密钥脱敏 / 熔断器 / 状态机 / 三方合并 / 上下文 / 提示（§9/§10/§12）
├── tenon-snapshot   # shadow git 快照库：snapshot / revert / restore / unrevert（§10.3）
├── tenon-sandbox    # 写守卫 / 网络三态 / Seatbelt+Landlock+seccomp / 超时执行器（§12.3）
├── tenon-models     # provider trait：OpenAI 兼容 / Anthropic / mock；路由与成本（§11）
├── tenon-lsp        # LSP 宿主：多路复用 + 铁律七守卫 + Open VSX 子集转换（§8.5/§13.3）
├── tenon-fs         # 文件树 / 读写 / ops / rg 搜索 / fuzzy / watcher / 脏缓冲（§8.1/§8.6）
├── tenon-laya       # 本地决策模型：choice/score/bool 三原语 + 五集成点（§9.8）
├── tenon-agent      # Agent 循环：感知→判断→执行→验证 + 审批 + Evals 运行器（§9.1/§18.3）
├── tenon-registry   # 官方静态 registry 客户端：manifest/签名/权限 diff（§13）
├── tenon-mcp        # MCP 客户端：initialize/tools 握手 + 默认 D 级映射（§13.3）
└── tenon-daemon     # 本地 HTTP+WS API（§15）：token / WS 票据 / 配对 / 团队策略
ui/                 # React + TS + Monaco 四区工作区 + i18n（中英）+ vitest
scripts/            # install-windows.ps1（WSL2 安装脚本，M3）
app/src-tauri/      # Tauri 2 桌面壳：daemon sidecar + 握手注入（§6.2）
```

## 快速开始

```bash
# 1. daemon（随机端口 + 握手 JSON 行）
cargo run -p tenon-daemon --bin tenon-daemon -- --db ~/.tenon/db.sqlite

# 2. UI（浏览器开发模式；?port=&token= 传入握手，或由 Tauri 壳自动注入）
pnpm install && pnpm ui

# 3. 测试
cargo test --workspace     # Rust 全量（含 GLM 真实端点联调：无 config.local.toml 自动跳过）
pnpm ui:test               # 前端 vitest

# 4. Evals 基准（附录 D 10 任务）
cargo run -p tenon-evals -- --provider glm
```

模型接入：默认读取 `~/.tenon/config.toml`（schema 见设计方案附录 E）。开发期联调可建仓库根 `config.local.toml`（已 git-ignore），`[models.providers.*]` 支持 `kind = "openai" | "anthropic" | "openai_responses"`。

## License

Apache-2.0
