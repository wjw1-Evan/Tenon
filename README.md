# Tenon

> 开源的 Agent 优先桌面开发环境：左边是完整编辑器（智能提示 / 诊断 / 重构），右边是自主干活的代理；人改与 AI 改实时互见，全程沙箱隔离、证据可查、随时回滚。

## 核心特性

- **共生而非外挂**：代理与编辑器共享同一套 LSP 活实例 / 诊断 / 语言包——一次索引、行为一致，编辑器即审查界面；
- **本地优先隐私**：会话 / Trace / 索引全本地，遥测默认零上报，云模型调用按用户显式配置并明示；
- **多模型、成本可控**：BYOK + 显式路由 + OpenAI 兼容端点 + 任务级成本归因；
- **审批负担最低且安全不缩水**：动作分级 + 沙箱三态 + 熔断器 + 随时回滚——安全动作自动执行，危险动作才打扰。

## 文档

- **[完整设计方案 v1.10](docs/design.md)** —— 产品定位、需求规格、架构、安全模型、编辑器与语言包、Agent 内核、数据与 API、路线图、测试策略

> v0.1 / v0.3 两轮评审（共 41 项）与 v1.0 复审（21 项）的结论已全部并入设计方案，历史过程文档已清理。
>
> 项目原名 OpenCodex，2026-10 定名 **Tenon**（沿革 OpenCodex → Weft → Tenon，设计方案附录 C · Q6；官网 tenonide.dev）。

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

设计定稿（v1.11，全部决策已闭环；产品名 **Tenon**，官网域名 tenonide.dev）。**M0 已实现并通过验收**：附录 D 基准 10 任务接 GLM 真实模型跑出基线 **8/10 通过（80%）≥ 50% 验收线，安全违规 0**（五指标基线见 `evals/baseline-glm.json`）。cargo + vitest 共 218 个测试通过（含 GLM 真实端点联调、sqlite-vec 冒烟 spike、真实 tsserver/Pyright 语义端点）。

**M1 后段进行中**：**Laya 本地决策模型（§9.8）已集成**——`tenon-laya` crate 提供分类/打分/布尔三原语与 starter 模型（词法线性分类器，纯 Rust CPU 推理）；静态 registry 分发（ed25519 签名 + SHA-256 + 一次 D 级审批卡，`POST /models/laya/download`）；五个集成点（意图预判/命令风险辅助/上下文预筛/路由启发/批量 triage）逐项开关、200ms 超时整体回退；`decider_call` 事件入 Trace（不含输入原文）；Evals 门验证「通过率不降、token 下降」（只读先验收窄首轮工具目录，离线可复现）。

**沙箱三态与共编（§12.3 / §8.6）已落地**：命令执行统一走 `SandboxSpec` 三态——macOS Seatbelt（断网 deny network*、写限项目内）、Linux namespace+Landlock+seccomp（unshare 断网 + 全盘读/项目写 + 拦 inet socket，x86_64 交叉检查通过）；**§18.2 沙箱逃逸套件**（断网网络调用、项目外写、hooks 子进程逃逸、镜像态不误伤）全部真实沙箱验证。人机共编：UI 未保存缓冲推送 → 代理写盘前 diff3 三方合并（不相交自动合并/同区冲突阻断写入并出三栏预览事件），UI 三栏合并对话框 + 行级「AI」角标（用户编辑即解除）。

**M1 收尾完成**：**崩溃恢复**（daemon 重启扫描非终态会话 → 回滚最近快照 → ROLLED_BACK 入 Trace；「先快照后写入」checkpoint 行写盘前落库，演练测试验证半成品不保留）；**模型路由**（会话级 provider 热切换、上下文随迁、model_fallback 事件；`/model-suggest` 路由建议 = Laya 集成点 #4 优先 + 规则引擎回退）；**语言包安装向导**（项目感知探测 + 官方指引自装 + 一键安装两阶段 D 级审批）。

**M2 进行中**：**官方静态 registry**（`tenon-registry` crate：§13.1 YAML manifest 规范、ed25519 签名清单、检索/安装/**权限 diff**、`official.*` 保留字防 typosquatting）；**MCP 外部进程插件**（`tenon-mcp` crate：stdio JSON-RPC initialize/tools 握手、tools/call 执行、**默认 D 级恒审批 / `net:*` 映射 C**，经 `mcp:` 工具前缀进执行器）；**并行子代理**（worktree 隔离 + 文件集不相交调度〔相交拒绝〕+ 并发 ≤3 + **复合 D 卡**〔逐条列明一次批准〕+ 隔离执行运行时，worktree 改动不落主工作区）。

**M2 完成**：**AgentTrace UI**（tool_calls/token/成本/审批记录实时可视化，底栏「AgentTrace」页签）；**CORS 白名单细化 + 本机配对入口**（放行源附加 CORS 响应头、`/pairing` 免 token 本机入口、局域网源默认拒绝）；**AI Evals 可视化**（`GET /evals` + 底栏「AI Evals」页签：五指标对比表）；**团队策略**（`~/.tenon/policy.toml` 只收窄：强制交互档/工具黑名单/成本上限取严）；**局域网配对存储**（`PairingStore`：显式开启 + 一次性 6 位码 + 可吊销令牌，默认关闭，4 测试）；**Windows WSL2 安装脚本**（`scripts/install-windows.ps1`：WSL2 检查/启用 + 依赖 + 构建 + 性能提示）。

后续：M3 剩余打磨（Evals 定时触发、策略下发通道、正式签名密钥）按第 17 章路线图推进；Roslyn LS spike 待 VSIX 二进制可用。

## 代码结构（monorepo，ADR-13）

```text
crates/
├── tenon-config     # config.toml schema（附录 E）
├── tenon-store      # SQLite：事件溯源 / 审批审计 / 成本归因 / 冷归档（§14.2）
├── tenon-core       # A/B/C/D 分级 / 密钥脱敏 / 熔断器 / 状态机 / 上下文 / 提示（§9/§10/§12）
├── tenon-snapshot   # shadow git 快照库：snapshot / revert / restore / unrevert（§10.3）
├── tenon-sandbox    # 写守卫 / 网络三态 / Seatbelt profile / 超时执行器（§12.3）
├── tenon-models     # provider trait：OpenAI 兼容 / Anthropic / mock；路由与成本（§11）
├── tenon-lsp        # LSP 宿主：多路复用 + 铁律七命令白名单（ADR-9）
├── tenon-fs         # 文件树 / 读写 / ops / rg 搜索 / fuzzy / watcher（§8.1）
├── tenon-agent      # Agent 循环：感知→判断→执行→验证 + 审批 + 快照联动（§9.1）
└── tenon-daemon     # 本地 HTTP+WS API（§15）：token / WS 一次性票据 / Origin 校验
ui/                 # React + TS + Monaco 四区工作区 + i18n（英文源/中文，Q5）+ vitest
app/src-tauri/      # Tauri 2 桌面壳：daemon sidecar + 握手注入（§6.2）
```

## 快速开始

```bash
# 1. daemon（随机端口 + 握手 JSON 行）
cargo run -p tenon-daemon --bin tenon-daemon -- --db ~/.tenon/db.sqlite

# 2. UI（浏览器开发模式；?port=&token= 传入握手，或由 Tauri 壳自动注入）
pnpm install && pnpm ui

# 3. 测试
cargo test --workspace     # Rust 全量
pnpm ui:test               # 前端 vitest
```

模型接入：默认读取 `~/.tenon/config.toml`（schema 见设计方案附录 E）。开发期联调可建仓库根 `config.local.toml`（已 git-ignore），`[models.providers.*]` 支持 `kind = "openai" | "anthropic" | "openai_responses"`。

## License

Apache-2.0
