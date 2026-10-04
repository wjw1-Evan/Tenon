# Tenon 完整设计方案

| | |
|---|---|
| 版本 | **v1.18** |
| 日期 | 2026-10-03（v1.11/v1.12）· 2026-10-04（v1.13-v1.18） |
| 状态 | 定稿（v1.10 决策闭环），M0 已验收（附录 D 基线 8/10=80%），M1-M3 主体已实现（见 README 状态节） |
| 许可 | Apache-2.0 |
| 历史评审 | v0.1 / v0.3 两轮共 41 项、v1.0 复审 21 项问题的结论已全部并入本方案（过程文档已清理） |

---

## 前言

**文档目的**：给出 Tenon 从产品定位、需求、架构到实施的完整设计方案，作为团队对齐与 M0-M3 开发的唯一依据。

**读者**：工程（Rust / 前端 / 工具链）、产品、设计、潜在贡献者。

**阅读路径**：

- 只想了解产品 → 第 1-3 章 + 第 19-20 章；
- 负责实施 → 第 6-18 章；
- 安全评审 → 第 12 章全篇。

**版本演进**：

| 版本 | 要点 |
|---|---|
| v0.1 | 初稿：融合五款工具优点的代理设计 |
| v0.2 | 修复 22 项评审问题：动作分级权限、威胁模型、上下文工程、现实路线图 |
| v0.3 | 产品形态决策：桌面端唯一入口、移除 Plan 模式、模型自主判断 |
| v0.4 | 修复 19 项复审问题：沙箱网络三态、本地服务防护、专用 checkpoint、熔断器、TOFU |
| v0.5 | 升级为 Agent-first IDE：文件管理、Monaco 编辑器、语言包与智能提示、Agent↔编辑器共生 |
| **v1.0** | **整合为完整方案：补需求规格、UI 设计、Agent 状态机、数据模型、本地 API、测试策略、ADR** |
| **v1.1** | **修复复审 21 项问题：隐私口径、WS 鉴权与 CORS、本地 API 补全、LSP 宿主命令白名单、L4 改自建索引、Windows 降级档、状态机补全（拒绝 / 超时 / ERROR 路径）、checkpoint 时机与回滚粒度、多会话写锁、语言包运行时分发、无测试验证通道、路线图重排** |
| **v1.2** | **附录 C 六项开放问题全部定稿并前置本期：Roslyn LS、sqlite-vec、中英双语首发、局域网访问后移 M3、静态 registry、更名决策（Weft 候选）** |
| **v1.3** | **开发就绪复核：修复 cache/index 残留矛盾；补 ADR-9~12、新术语、events 枚举（approval_timeout / model_fallback）、l4_chunks 表、熔断预算默认值、子代理 worktree 分级、写锁与用户编辑关系、快照清理与按事件撤销关系、基准任务集定稿时点、M0 降载顺序** |
| **v1.4** | **竞品事实修正：Codex 已开源（Apache-2.0）并支持兼容端点与 MCP；新增 §3.1 开源对标（codex-rs 沙箱 / 审批 / execpolicy / 配置 / 会话 / app-server / Windows 沙箱）** |
| **v1.5** | **全部待决事项闭环：定名 Weft 并全局替换（域名 RDAP 核验：官网 weftide.dev）；附录 D 基准任务集（10 任务）与附录 E config.toml schema 定稿；ADR-13 monorepo / CI；状态改「定稿」** |
| **v1.6** | **终检打磨：快捷键冲突修正（时间轴改 Cmd+Alt+Z，避开 Redo 惯例）、目录树补 worktrees/、M0 交付补脚手架与 CI、命令超时默认 120s（附录 E）、附录指针互通（§9.3 / §14.1 ↔ 附录 E）、术语表补 Weft、T5 断言措辞修正、README 原名注记** |
| **v1.7** | **新增 §3.2 成熟产品参考地图（15 项：Zed/ACP、IntelliJ PSI、OpenHands、Aider、Goose、Gemini CLI、Cline、Zoekt、LiteLLM、Langfuse、Jan 等）+ 闭源 UX 观察清单；后续「IDE 开放协议」定为对齐 ACP；补录 OpenCodex 命名风险实证（151 个同名仓库 / 榜首 16.8k star / .dev 与 .com 域名被占，ADR-12）** |
| **v1.8** | **新增 Laya 本地决策模型集成：§9.8 加速层（意图预判 / 命令风险辅助 / 上下文预筛 / 路由启发 / 批量 triage 五集成点，全程本地 CPU、逐项可关、整体可回退）；自动下载走静态 registry + 签名 + 一次 D 级卡；§11 / §14.1 / §14.2（events 补 `decider_call`）/ §15 / §16 / 附录 E `[models.laya]` / 术语表 / ADR-14 / 路线图（M1 后段核心、M2 深化）同步** |
| **v1.9** | **回滚机制重构为 opencode 式独立 shadow git 快照库（§10.3，源码级参考 sst/opencode `src/snapshot/` + `src/session/revert.ts`）：零写用户仓库（`.git` 内嵌快照状态已有用户反弹实证，sst/opencode#10861）；git / 非 git 项目统一单路径（弃 shadow ref + CoW 双轨，ADR-7 修订）；alternates 种子 + index 复用加速大仓库；快照点 = tree oid；按事件撤销 / 整体恢复映射为 revert / restore 双原语；**新增撤销回滚（unrevert）**；§3.2 / §4.1 / §7.1 / §7.3 / §9.7 / §14 / §15 / §17 / §21 / 术语表 / 附录 E 同步** |
| **v1.10** | **产品定名 Tenon（榫；沿革 OpenCodex → Weft → Tenon；官网 tenonide.dev，备选 usetenon.dev，均经 RDAP 核验未注册；GitHub 命名空间干净，同名仓库榜首仅 ⭐194）；全局替换含路径 `~/.tenon/`、`X-Tenon-Token`；本地目录同步更名 Tenon** |
| **v1.11** | **实施期增补：附录 E `[models.providers.*]` 增加可选 `kind`（协议族：openai / anthropic / openai_responses）与 `model`（该 provider 默认模型）——支持同一 provider 配置接入选定协议的第三方端点（如 GLM 的 Anthropic / OpenAI 双协议端点）；约定开发期本地密钥文件 `config.local.toml`（git-ignore，不入 `~/.tenon/config.toml`、不提交源码），仅供联调测试读取** |
| **v1.12** | **实施期增补（M2/M3 落地同步）：附录 E 增加 `[evals] interval_hours / provider`（M3 流水线定时触发，0=关）；团队策略文件 `~/.tenon/policy.toml`（只收窄：force_interactive / denied_tools / max_cost_usd）；局域网访问落地为 `--lan` 显式绑定 + PairingStore（一次性 6 位码 + 可吊销设备令牌，默认关闭）；签名密钥 `--generate-keys` 生成于 `~/.tenon/keys/signing.*` 并接入 laya/registry 公钥解析链（env → 文件 → 开发模式）；M0 验收基线 8/10=80% 固化于 evals/baseline-glm.json** |
| **v1.13** | **实施期增补（M2 落地同步）：Open VSX 语言子集实验兼容落地（`contributes.languages` 转内部语言包，服务器命令取实验扩展点 `contributes.tenonLsp`——VS Code API 未标准化清单式 LSP 声明，子集显式要求该字段）；`/lsp` 语义操作增加 codeaction；附录 C Q2 Roslyn LS spike 记录为**受阻**（开发环境无 VSIX 二进制；共享 LSP 宿主/沙箱/守卫基础设施已就绪），**按既定回退规则将 C# 语言包后移**，不降级换 csharp-ls** |
| **v1.14** | **实施期澄清（桌面端 E2E 驱动）：§7.3「本会话记住」语义定稿——按动作类别记忆，适用于 B / C 级（B 级有沙箱 + 快照可回滚兜底），D 级永不记忆逐次审批；`/pairing` 自发现响应补 `project` 字段（daemon 启动注册的项目根），浏览器访问 UI 据此打开同一项目而非 daemon cwd** |
| **v1.15** | **多项目运行模型定稿（参考 codex app-server 的 thread/cwd/projectId/runtimeWorkspaceRoots 持久化语义）：项目是 daemon 内一等运行对象；会话强绑定 `project_id` 与规范化项目根；文件 / 搜索 / LSP API 全部按项目作用域；同一 daemon 可并发打开多个项目与项目内多会话，跨项目只做全局任务中心聚合、不做隐式上下文共享；§3.1 / §4.1 / §6-7 / §9-10 / §12 / §14-15 / §17-18 / 附录 A-B/E 同步** |
| **v1.16** | **§7.5 新增外观档：深色（默认）/ 浅色 / 跟随系统三档；`data-theme` 驱动 CSS 变量整套换色；偏好双写——localStorage 快路径 + daemon `ui_prefs` 权威存储（跨启动 / 跨端；daemon 端口动态，localStorage 按 origin 隔离不可跨启动）** |
| **v1.17** | **实施期同步（§6.4 / §8.1 / §15）：ProjectRuntime 落地 watcher 懒启动 / 活跃度追踪 / 60s 回收扫描；项目关闭与空闲回收会停止 watcher 并关闭该项目 LSP hosts；WS 支持 `?project_id=` 过滤并推送 project-scoped `created / modified / removed` 文件事件；Agent lsp_query 与验证诊断共享同一 LSP 宿主；项目任务中心补充待审批 / 脏缓冲 / 项目成本摘要** |
| **v1.18** | **§8.2 新增自动保存：编辑停顿 1s 去抖写盘，Cmd/Ctrl+S 立即保存；保存成功解除脏缓冲（§8.6 语义不变），tab 未保存圆点指示，失败保留待重试** |

---

# 第一部分 · 产品设计

## 1. 背景与问题

### 1.1 行业现状

AI 编程工具呈四极分化，各有明显短板：

| 阵营 | 代表 | 强项 | 短板 |
|---|---|---|---|
| 终端代理 | Claude Code、Codex、OpenCode | 自主任务执行、闭环改码 | 界面门槛高、审查体验弱（Claude Code 闭源绑定；Codex 已开源但仍以终端 / 云为中心） |
| AI IDE | Cursor | 编辑体验、行内补全 | 闭源、模型绑定、代理自主性与治理弱 |
| Agent 运行时 | DSH（DeepSeek Harness） | 一切皆插件、扩展性 | 开发者预览、产品化与安全默认弱 |
| 企业平台 | Harness | 治理、评测、审计 | 重、贵、个人开发者不可用 |

### 1.2 核心痛点

1. **安全与自动的矛盾**：要么处处审批打断心流，要么放开权限裸奔；
2. **代理与 IDE 割裂**：代理自建一套代码理解，编辑器另一套 LSP，重复索引、行为不一致；
3. **成本不透明**：模型调用像黑盒，任务级花费无法归因；
4. **信任缺失**：改了什么、为什么改、能否回滚，证据链不完整；
5. **形态两难**：终端工具上手难，AI IDE 闭源且代理弱。

### 1.3 机会

一个**桌面原生、Agent 优先、开源本地、模型自由**的开发环境，让代理与编辑器**共享同一套语言智能**，用动作分级 + 沙箱 + 熔断器同时拿到「自动」与「安全」。

---

## 2. 产品定位

### 2.1 一句话定位

> **Tenon：开源的 Agent 优先桌面开发环境——左边是完整编辑器（智能提示 / 诊断 / 重构），右边是自主干活的代理；人改与 AI 改实时互见，全程沙箱隔离、证据可查、随时回滚。**

### 2.2 目标用户

| 画像 | 描述 | 核心诉求 | 对应能力 |
|---|---|---|---|
| **小林 · 独立开发者** | 不熟悉终端，习惯 GUI | 说出问题就修好，别让我配环境 | 30 秒上手、桌面 App、诊断上一键 AI 修复 |
| **阿哲 · 资深工程师** | VS Code 重度用户，管大型仓库 | 我要控制权与安全，AI 别乱动 | 动作分级、熔断器、checkpoint、本地模型、Monaco 体验 |
| **Maya · 团队负责人** | 带 5-15 人团队 | 可审计、可管控、成本可见 | AgentTrace、团队策略、AI Evals、成本核算 |

### 2.3 核心使用场景

| # | 场景 | 入口 | 涉及能力 |
|---|---|---|---|
| S1 | 修复 bug（含从诊断发起） | 诊断「AI 修复」/ 会话 | 自主循环、测试+诊断双验证、回滚 |
| S2 | 行内重构指令 | 选中代码 → 输入指令 | 行内指令、B 级沙箱、就地 diff |
| S3 | 批量 issue 并行处理 | 会话下达批量任务 | 子代理、worktree、不相交调度 |
| S4 | 只读代码理解 / 审查 | 会话 / 右键菜单 | 只读开关、共享 LSP、解释生成 |
| S5 | 依赖升级 / API 迁移 | 会话 | C 级网络审批、镜像代理、验证闭环 |
| S6 | 新项目脚手架 | 会话 / 启动页 | 沙箱内创建、语言包推荐 |
| S7 | 团队治理与升级决策 | 设置 / Evals 报告 | 策略文件、Trace、Evals 报告 |

### 2.4 设计原则

1. **30 秒上手**：安装 → 贴 Key（或本地模型零 Key）→ 开聊；编辑器零配置可用，语言包按项目自动建议。
2. **编辑器可用是代理可信的前提**：用户必须能在 App 里看懂、审查、修正每一步改动——编辑器是审查界面本身。
3. **先自主、后审查 + 事中熔断**：无 Plan 前置审批；安全靠动作分级 + 沙箱 + 熔断 + 回滚。
4. **核心稳定、外围皆插件**：内核循环、权限模型、LSP 宿主编译进内核；模型、语言包、工具是插件。
5. **每个自动化承诺必须带收敛条件**：自动修改、修复、并行子代理都有停止条件与预算上限。

---

## 3. 竞品分析与取舍

| 维度 | Claude Code | OpenCode | DSH | Harness | Codex | Cursor | **Tenon 取舍** |
|---|---|---|---|---|---|---|---|
| 入口 | CLI | CLI/TUI | 本地 Web | 平台 | CLI/App | VS Code fork | **桌面 Agent-first IDE + 浏览器兼容** |
| 编辑器 / 语言智能 | 无 | LSP 辅助 | 无 | 无 | 无 | 完整生态 | **Monaco + LSP 语言包**，不自建编辑器也不 fork |
| 闭环改码 | 强 | 依赖模型 | 预览期 | 平台级 | 强 | 中 | 借鉴 + 收敛条件 + 证据链 |
| 模型自由 | 默认绑定，可接兼容端点 | 75+ | 可换 | 托管 | 绑定 | 多模型 | 显式路由 + OpenAI 兼容端点 |
| 插件化 | MCP | 插件+MCP | 一切皆插件 | 平台 | MCP | VS Code 扩展 | 核心稳定、外围插件；Open VSX 语言子集实验 |
| 治理 / 审计 | 弱 | 弱 | 弱 | 强 | 中 | 弱 | 借鉴 Harness，默认全本地 |
| 开源 | 否 | MIT | MIT | 否 | **是（Apache-2.0）** | 否 | Apache-2.0 |

**结论**：不宣称全面超越；以「共生而非外挂」的代理-编辑器关系为差异化核心（详见第 20 章）。Codex 已开源（Apache-2.0），「开源」不再是我方独占优势——差异化重心进一步落在 Agent-first IDE 与编辑器共生。

### 3.1 开源对标：codex（openai/codex，Apache-2.0）

Codex CLI 已开源且核心为 Rust 实现（codex-rs 工作区，另有遗留 TS 版与 SDK），与本项目同语言、同问题域，**实现层可直接研读借鉴**：

| 对标点 | codex 实现（crate 为证） | 对应本方案 | 动作 |
|---|---|---|---|
| 沙箱 | `sandboxing` / `bwrap` / `mxc-sandbox` / `network-proxy` / `process-hardening`：macOS Seatbelt、Linux Landlock + seccomp；网络默认断开 + 代理白名单 | §12.3 沙箱三态 | M0 写守卫 / M1 完整沙箱实现前研读，Seatbelt / Landlock profile 直接参考 |
| 审批策略 | approval_policy（untrusted / on-failure / on-request / never）与沙箱模式（read-only / workspace-write / danger-full-access）正交组合 | §12.2 档位只影响 B 级 | 印证「档位 × 分级正交」设计；审批词汇表可对齐降低用户迁移成本 |
| 命令风险判定 | `execpolicy`：规则引擎评估 shell 命令风险 | §9.2 工具分级 | M1 后评估作为命令级分类补充（不替代 C/D 审批） |
| 模型接入 | `model-provider` / `model-provider-info` / `ollama` / `config-schema`：config.toml `model_providers`（base_url + wire_api）接任意 OpenAI 兼容端点 | §11 通用 provider | config schema 直接参考 |
| 会话持久化 | `rollout` / `rollout-trace` / `thread-store`：JSONL 会话回放 + resume | §10.3 / §14.2 事件溯源 | 回放与 resume 语义参考 |
| 多项目 / 多线程 | app-server 的 thread 可携带 `cwd`、`projectId` 与 runtime workspace roots，thread 摘要持久化 cwd / git 信息；同一服务端可管理多个活跃 thread | §6.4 项目运行时 + §9.7 并发 | 会话强绑定项目根与项目 ID；沙箱权限、事件与成本按项目归因 |
| IDE / 桌面集成 | `app-server` / `app-server-protocol` / `app-server-transport` / `app-server-daemon`：JSON-RPC 进程协议服务 IDE 与桌面壳 | §6.2 daemon + §15 本地 API | 同类问题域，协议形态与演进方式参考 |
| worktree | `worktree` crate | §9.5 子代理独立 worktree | 实现参考 |
| Windows 沙箱 | `windows-sandbox-rs` / `windows-sandbox-service`（原生方案，非 WSL2） | §5 非目标 7、§17 后续 | 后续评估「Windows 原生沙箱」时优先研读 |
| 生态 | `rmcp-client`、`skills`、`plugin`、AGENTS.md（其发起的事实标准） | §13.3 兼容策略 | 验证方向正确 |

**研读纪律**：借鉴架构与机制，不拷贝代码进内核（保持本项目独立演进）；任何引用遵守 Apache-2.0 许可与 NOTICE。**GPL 系（Zed）仅研读架构，代码零拷贝。**

### 3.2 成熟产品参考地图（v1.7）

> 按「哪个子系统向谁学」组织，与 §3.1 codex 对标互补；研读纪律同上。

| # | 产品 | 许可 / 形态 | 重点研读 | 对应本方案 |
|---|---|---|---|---|
| 1 | Zed | GPL-3.0 / Rust 编辑器 | LSP 客户端多 crate 架构、tree-sitter 深度集成、**ACP（Agent Client Protocol）**——JSON-RPC 开放协议，已成编辑器 ↔ 代理互操作的事实标准（Gemini CLI 首个原生接入，JetBrains / Eclipse 跟进） | §8.5 LSP 宿主；§15；§17 后续「IDE 开放协议」**对齐 ACP，不自造协议** |
| 2 | IntelliJ Platform（Community） | Apache-2.0 / JVM | PSI 单一代码模型同时驱动检查、重构与 Junie 代理——「共享语言智能」的成熟先例 | §8.5 / §20 差异化可行性验证 |
| 3 | OpenHands（原 OpenDevin） | MIT / Python | 事件流架构（代理—事件—运行时解耦）、容器沙箱、评测 harness | §9.1、§14.2、§18.3 |
| 4 | Aider | Apache-2.0 / Python | repo map（tree-sitter + 图排序选符号）、编辑格式族、每步 lint/test 闭环、polyglot 基准 | §10.1 L1/L4、§9.4、附录 D |
| 5 | Goose（Block） | Apache-2.0 / Rust | Rust 代理运行时、MCP-first 扩展体系 | §6.3、§13 |
| 6 | Gemini CLI | Apache-2.0 / TS | 审批 × 沙箱策略组合、遥测 opt-in 实践、ACP 接入实现 | §12.2 / §12.3、§4.2、§17 后续 |
| 7 | OpenCode | MIT | 75+ 供应商抽象层、server 与多前端（TUI / Web）拆分；**rewind：独立 shadow git 快照库 + 消息级回滚 / 撤销回滚**（源码 `src/snapshot/index.ts`、`src/session/revert.ts`，§10.3 的直接参考） | §6.2、§11、**§10.3（主要参考）** |
| 8 | Cline / Roo Code | Apache-2.0 / VS Code 扩展 | shadow git checkpoint（同思路先例；主要参考取 OpenCode，§3.2 #7）、逐 diff 人审、细粒度 auto-approve | §10.3、§7.3、§12.2 |
| 9 | Continue | Apache-2.0 | 行内补全架构、多模型角色配置 | §4.1 P2 |
| 10 | Zoekt | Apache-2.0 / Rust | trigram 代码检索与索引分片（Sourcegraph 同源） | §10.1 L4 倒排层 |
| 11 | e2b | Apache-2.0 | 沙箱执行 SDK（microVM）、会话式沙箱生命周期 | §12.3（云端形态参考；v1 坚持本地） |
| 12 | LiteLLM | MIT | 按键预算 / 限流 / 成本归因的成熟能力 | §11；§9.3 任务预算的落地面 |
| 13 | Langfuse | MIT / 可自托管 | LLM trace 数据模型（span / generation / cost）与看板 | §14.2 AgentTrace、§18.3 报告 |
| 14 | Jan | Apache-2.0 / **Tauri** | Tauri 桌面壳生产实践 + 本地模型管理 UX（发现 / 下载 / 切换） | §6.2、§11 |
| 15 | Neovim | Apache-2.0 | LSP 客户端与 tree-sitter 的极简参考实现 | §8.2 |

**闭源观察清单**（只看 UX，不作代码参考）：Claude Code（hooks / 子代理 / rewind）、Cursor（Composer / 后台代理）、GitHub Copilot Agent Mode、Windsurf、Warp——每个里程碑末体验一轮，差异点记入 Evals 观察笔记。

---

## 4. 需求规格

### 4.1 功能需求（按模块）

> P0 = M0/M1 必须；P1 = M2；P2 = M3 或后续。完整清单在此收敛为核心集。

| 模块 | 需求 | 级别 |
|---|---|---|
| 项目与文件 | 多项目登记/打开/关闭、项目状态与任务汇总、拖拽仓库、文件树 CRUD、git 状态装饰 | P0 |
| 项目与文件 | 会话与所有文件 API 强绑定 `project_id`；同 daemon 并发打开多项目；项目运行时引用计数与空闲回收 | P0 |
| 项目与文件 | fuzzy 查找、ripgrep 全局搜索替换（预览 diff）、多标签分栏 | P0 |
| 项目与文件 | LSP 感知重命名/移动；大文件只读分块 | P1 |
| 编辑器 | Monaco 内核、tree-sitter 高亮、主题、虚拟化 | P0 |
| 编辑器 | LSP 补全/hover/定义/引用/重命名/诊断/code action/格式化/语义高亮 | P1 |
| 编辑器 | AI 行内补全（ghost text，默认关） | P2 |
| 语言包 | TS/JS、Python 内置；C#、Rust、Go 一键安装；运行时检测引导 | P1 |
| 语言包 | Open VSX 语言类扩展子集实验兼容 | P1 |
| 代理 | 自主决策循环（感知→判断→执行→验证→证据） | P0 |
| 代理 | 项目级写锁与跨项目并发调度；全局项目任务中心 / 成本 / 审批汇总 | P0/P1 |
| 代理 | 行内指令、诊断发起任务、只读开关、跟随模式 | P1 |
| 代理 | 并行子代理（不相交调度、预算上限） | P1 |
| 安全 | 动作四级分级、沙箱三态、审批卡片、密钥拦截 | P0(P1 完整沙箱) |
| 安全 | checkpoint（独立 shadow git 快照库，§10.3）、回滚 / 撤销回滚、TOFU、崩溃恢复 | P0/P1 |
| 模型 | OpenAI / Anthropic / DeepSeek / Ollama + OpenAI 兼容端点；显式路由；成本显示 | P0/P1 |
| 模型 | 本地决策模型 Laya：自动下载 + 意图预判 / 命令风险辅助 / 上下文预筛 / 路由启发 / 批量 triage（§9.8） | P0（M1 后段核心）/ P1（深化） |
| 插件 | 外部进程插件 + MCP；官方 registry、签名、权限 diff | P1 |
| 治理 | AgentTrace、本地报告；团队策略文件、AI Evals | P1/P2 |
| 平台 | 浏览器访问（本机 127.0.0.1 为 P1；局域网配对后移 M3，Q4）；Windows WSL2 安装包 | P1/P2 |

### 4.2 非功能需求

| 类别 | 要求 |
|---|---|
| 性能 | 冷启动可输入 < 1.5s；LSP 索引就绪后首补全 < 400ms（冷启动到首个可用补全 < 3s）；10MB 文件 < 2s；搜索首结果 < 500ms；万行 diff 60fps（详见 §8.7） |
| 安全 | 安全违规 = 0 一票否决；沙箱逃逸测试套件全通过；本地服务防 CSRF/DNS rebinding |
| 隐私 | 本地数据（会话/Trace/索引/设置）默认不出本机；使用云端模型时代码上下文按用户显式配置发送并在 UI 明示；更新默认手动检查；崩溃/行为报告 opt-in 且明示内容；模型 Key 存系统钥匙串 |
| 可靠 | daemon 崩溃自动拉起；会话状态/事件/上下文 100% 可恢复（EXECUTING 中崩溃自动回滚到最近 checkpoint，不承诺原地续跑，见 §10.3）；「自动档 = 必可回滚」不变式 |
| 兼容 | macOS 13+；Windows 10+（WSL2；无 WSL2 时降级档运行，见 §12.3）；Ubuntu 22.04+；三平台 WebView 兼容测试 |
| 无障碍 | 全键盘操作；对比度达标；屏幕阅读器基础支持 |
| 国际化 | 中英双语同期首发（Q5）：英文为源语言（source of truth）、中文一级翻译，默认跟随系统可手动切换；UI 文案外置；日期时间本地化 |

---

## 5. 非目标（明确不做）

1. **完整 VS Code 扩展宿主 / 全量扩展 API 兼容**（= 重造 VS Code）；仅实验兼容 Open VSX 语言类子集；
2. **CLI / TUI 入口**；3. **为其他 IDE 出插件**；
4. 云端多人协作与托管执行；5. 公网远程 Web 访问；
6. 自研模型 / 训练；7. Windows 原生沙箱（v1 走 WSL2）；
8. 全自动模型路由、AI 行内补全**默认开启**（均实验、默认关）；
9. 移动端。

---

# 第二部分 · 架构与详细设计

## 6. 总体架构

### 6.1 分层视图

```text
┌───────────────────────────────────────────────────────────────┐
│ UI（React，桌面壳 Tauri / 浏览器同一代码库）                     │
│  文件树 · 编辑器(Monaco) · 会话流 · Diff/证据 · 诊断 · 审批卡    │
├───────────────────────────────────────────────────────────────┤
│ 核心服务（Rust daemon，本地 HTTP+WS）                           │
│  文件服务(watcher/rg) · LSP宿主(共享多路复用/沙箱化)             │
│  Agent内核(感知→判断→执行→验证) · 上下文引擎 · 子代理调度          │
├───────────────────────────────────────────────────────────────┤
│ 能力层  内置工具 · 语言包(LSP) · 外部进程插件/MCP · WASM         │
├───────────────────────────────────────────────────────────────┤
│ 模型层  显式路由 · BYOK · 本地模型 · 决策模型(Laya) · OpenAI 兼容端点 │
├───────────────────────────────────────────────────────────────┤
│ 治理层  权限策略 · AgentTrace · AI Evals · 成本核算 · 审计       │
└───────────────────────────────────────────────────────────────┘
          安全与权限模型贯穿所有层（第 12 章）
```

### 6.2 进程模型与 daemon 生命周期

**进程组成**：

| 进程 | 说明 | 生命周期 |
|---|---|---|
| 桌面壳（Tauri） | WebView 承载 UI，本身无业务逻辑 | 用户启停 |
| 核心服务（Rust daemon） | 唯一业务内核；持有项目运行时、会话、LSP、沙箱、插件 | 壳 sidecar 管理，崩溃自动拉起 |
| 沙箱执行进程 | 测试 / 构建 / 依赖安装，一次性 | 任务期 |
| 语言服务器进程 | 每项目每语言一个，沙箱化 | 项目运行时活跃期 |
| 插件进程 | 外部进程插件 / MCP | 按需 |

**生命周期规则**：动态端口 + 握手；单实例锁（多窗口 / 多项目共享，daemon 不是“单项目进程”）；`--project` 仅作为首屏种子，运行中可通过项目 API 追加打开；退出时清理全部子进程；UI 崩溃不丢会话（状态全在 daemon + 磁盘）。

### 6.3 插件三档运行时

| 档 | 运行时 | 边界 | 用途 |
|---|---|---|---|
| 内置 | Rust 编译进内核 | 最快、走内核权限 | 文件、rg、git、diff、LSP 宿主 |
| 外部进程 | 自带运行时，stdio JSON-RPC | 兼容 MCP / LSP；沙箱化 | 语言包、数据库、浏览器 |
| WASM | 宿主 WASM 运行时 | 无系统 I/O 纯计算 | 格式化、解析、轻转换 |

分发：安装包内置核心与默认语言包；其余按需安装并检测运行时。

### 6.4 多项目运行模型（v1.15）

**核心不变式**：一个 daemon 服务多个项目；一条会话在创建时绑定唯一 `project_id` 与规范化项目根；一个请求只操作其显式声明的项目。不存在“最近打开的第一个项目”这类隐式默认根。

```text
ProjectRegistry（登记 / 打开 / 关闭 / 最近项）
  └─ ProjectRuntime: project_id → 规范化 root + 状态 + 引用计数
        ├─ FileService / Watcher / Git 状态
        ├─ LSP runtime（每项目 × 语言）
        ├─ SnapshotStore / ProjectWriteLock
        ├─ SandboxProfile（root × trust × worktree）
        └─ ContextIndex（L4，按 project_id 隔离）
GlobalScheduler（全局并发 / 成本 / 审批 / 通知 / 项目任务中心）
```

| 概念 | 规则 |
|---|---|
| Project | 稳定 `project_id`、显示名、canonical path、TOFU 信任、语言包与项目设置；登记不等于打开 |
| ProjectRuntime | 打开时懒加载；文件 / LSP / watcher / 快照等资源挂在其下，引用计数归零后空闲回收（默认 10 分钟，可配置） |
| Session / Thread | 永远归属一个项目；可携带项目内 worktree 或子目录 cwd，但任何 B 级写边界仍由项目根与显式 worktree 白名单决定 |
| Active surface | UI 的每个编辑窗格都有明确 active `project_id`；项目切换、命令面板与发送任务都会携带该 ID |
| 跨项目任务 | v1 不允许一条代理会话直接读写多个项目。多项目编排只能由“项目组合任务”创建多条项目内子会话，父任务仅聚合状态 / 成本 / 审批，不透传代码上下文 |

**打开与去重**：打开前 canonicalize + macOS/Linux 大小写校验 + Windows 前缀归一；同一 canonical path 复用既有 `project_id` 与 runtime。嵌套根默认拒绝（例如同时打开 monorepo 与其子包），除非用户确认并创建显式 linked workspace；linked workspace 也只是注册关系，不放宽任何沙箱路径。

**资源与调度**：默认最多 12 个打开项目、跨项目最多 2 个代理同时 EXECUTING、每个项目仍遵守 §9.7 项目级写锁。watcher / LSP / L4 索引按活跃度懒启动；低内存或用户“暂停项目”时关闭非活跃语言服务器与 watcher，但保留会话与 checkpoint 可恢复。

---

## 7. 桌面端 UI 设计

### 7.1 界面清单

| 界面 | 时机 | 要点 |
|---|---|---|
| 启动 / 项目选择 | 冷启动 | 最近项目、拖拽打开、新建；可打开多个；首次打开弹 TOFU 信任卡 |
| **项目中心 / 项目切换器** | 常驻 | 已登记项目、打开状态、活跃任务 / 审批 / 成本 / 未保存缓冲；打开 / 关闭 / 暂停 / 移除登记（不删盘） |
| **主工作区** | 常驻 | 四区布局（见 7.2） |
| 审批卡片 | C/D 级动作 | 级别、动作详情、目标域名 / diff 预览、允许一次 / 本会话 / 拒绝 |
| Checkpoint 时间轴 | 侧栏 | 事件流 + 快照点，任意回滚 / 撤销回滚（unrevert） |
| 语言包安装向导 | 检测到语言缺包 | 一键安装、运行时检测与官方指引 |
| 设置 | 全局 | 模型 / 语言包 / 插件 / 权限 / 隐私 / 更新 |
| 命令面板 | Cmd+Shift+P | 全部命令可达（无障碍要求） |
| Evals 报告 | M3 | 五指标 + 对比版本 |

### 7.2 主工作区布局

```text
┌──────────┬──────────────────────────┬────────────────┐
│ 项目切换器│ 文件树（active project）    │ 编辑器（多标签 / 分栏）│
│ 任务中心  │ 搜索 / 行内 AI / diff       │ 代理会话 / 审批  │
├──────────┴──────────────────────────┴────────────────┤
│        诊断面板 · 测试证据 · checkpoint 时间轴           │
└───────────────────────────────────────────────────────┘
```

三区均可全屏 / 折叠 / 左右互换；布局按项目记忆。项目切换器可以是顶栏下拉，也可以把另一个项目停靠为独立分栏 / 窗口；每个窗格维护独立 active `project_id`。底部任务中心显示所有打开项目的代理状态，卡片必须带项目名 / 根目录短名，避免多项目通知混淆。

### 7.3 关键交互流

**打开项目**：拖入仓库 → 识别语言 → 建议 language pack（缺则装）→ TOFU 卡（默认交互档 / 信任后自动档）→ 建 L4 索引（后台）→ 就绪。已有项目打开时复用原 `project_id`；再次打开 canonical path 只激活 runtime，不新建登记。

**切换 / 并行操作项目**：项目中心选择目标项目或把项目停靠为新窗格；所有文件 / 搜索 / LSP / 会话请求携带目标 `project_id`。用户可同时保留 A 的执行中任务并在 B 继续；审批队列全局可见且按项目分组。关闭项目前检查活跃任务和脏缓冲，可选择「等任务完成 / 暂停任务 / 强制关闭并保留会话恢复」。

**下达任务**：会话输入 / 行内指令 / 诊断「AI 修复」→ 进入自主循环（§9.1）→ 首改 2s 缓冲（Esc 可断）→ 改动实时高亮 → 证据卡片 → D 级审批提交。

**审批**：卡片显示级别（C / D / C+D 复合）、具体动作、影响范围；选择「允许一次 / 本会话记住 / 拒绝」；决策写入 Trace。「本会话记住」按动作类别生效，适用于 B / C 级（B 级写入有沙箱 + 快照可回滚兜底）；**D 级永不记忆，逐次审批**。

**回滚**：两种粒度——① **按事件撤销**（默认）：基于事件日志与各步记录的改动文件集（§10.3），只撤销选定的 AI 补丁序列（快照中不存在的 AI 新建文件一并删除），用户同期手改与未保存缓冲走三方合并保留；② **checkpoint 整体恢复**（高级）：时间轴选快照 → 预览将丢弃的改动（明确警示包含用户手改）→ 确认 → shadow 库整体恢复。两种粒度的差异在预览中逐文件展示（§10.3）。**回滚本身可撤销（unrevert）**：每次回滚前自动追加快照，时间轴一键撤销最近一次回滚（opencode 同款双向语义）。

### 7.4 快捷键（核心集）

| 键 | 动作 |
|---|---|
| Cmd/Ctrl+P | 模糊打开（文件 / 符号 / 行） |
| Cmd/Ctrl+Shift+P | 命令面板 |
| Cmd/Ctrl+I | 行内 AI 指令（选中代码） |
| Cmd/Ctrl+Enter | 发送会话消息 |
| Esc | 暂停代理 / 关闭卡片 |
| Cmd/Ctrl+. | 停止当前任务 |
| Cmd/Ctrl+Alt+Z | 打开 checkpoint 时间轴（避开 Cmd+Shift+Z 的 Redo 惯例） |
| Cmd/Ctrl+B / J / ` | 侧栏 / 面板 / 终端区（终端区 M3 评估） |

### 7.5 设计系统要点

- **状态色**：感知（蓝）、执行（黄）、验证（紫）、等待审批（橙）、失败（红）、完成（绿）；
- **核心组件**：会话流卡片、证据卡片、审批卡片、诊断条、AI 修改高亮区、时间轴节点、语言包安装卡；
- **AI 改动可视**：所有代理写入的行带「AI」角标，直到用户编辑该区域或确认；
- **外观档**：深色（默认）/ 浅色 / 跟随系统（`prefers-color-scheme`）三档；顶栏切换即时生效，`data-theme` 属性驱动 CSS 变量整套换色，状态色两套均可读；偏好双写——`localStorage` 为快路径，daemon `ui_prefs` 存储（§14.1）为跨启动 / 跨端（桌面 + 浏览器）权威（daemon 端口动态，localStorage 按 origin 隔离不可跨启动）；
- 全键盘可达；中英文案外置。

---

## 8. 编辑器与文件管理

### 8.1 文件管理

- 文件树：以 active `project_id` 为唯一根；新建 / 重命名 / 移动 / 删除 / 拖拽 + git 状态装饰；
- **LSP 感知重命名 / 移动**：跨文件引用更新（B 级 + checkpoint 可回滚）；
- fuzzy 查找（Cmd+P 文件 / 符号 / 行号）；
全局搜索替换：核心服务 ripgrep 驱动，正则 / 过滤 / 多文件替换前 diff 预览；
- 多标签、分栏、布局记忆；源代码视图（分支、改动列表、行内 blame）；
- 同一窗口可停靠多个项目分栏；标签携带项目徽标，跨项目拖拽默认禁止；
- 大文件（默认 >10MB）自动只读分块加载。

### 8.2 编辑器内核

| 项 | 选择 | 说明 |
|---|---|---|
| 内核 | Monaco Editor | 与 VS Code 同源交互（补全 / hover / 多光标），不自建编辑器 |
| 高亮 | tree-sitter + LSP semantic tokens | 解析在 daemon 侧（Rust 增量解析，token 流推送 UI，WebView 不跑解析器）；未装语言包有基础高亮，装包后语义高亮 |
| 大文件 | 虚拟滚动 + 分块 + 超限只读 | 性能预算见 §8.7 |
| 自动保存 | 编辑停顿 1s 去抖写盘；Cmd/Ctrl+S 立即保存 | 保存成功即解除该文件脏缓冲（§8.6「未保存缓冲」语义不变：保存前的编辑窗口内代理写盘仍走三方合并）；tab 显示未保存圆点，保存后消失；写盘失败保留圆点待下次编辑或手动保存重试 |

**取舍**：Monaco ≠ VS Code，不引入 extension host——「够用且可控」优先于完整生态。

### 8.3 语言智能（智能提示全家桶）

补全（符号/成员/路径/片段/签名帮助）、hover 文档、定义跳转、引用/实现查找、工作区符号、安全重命名、语义高亮、实时诊断、code action 快速修复、**「AI 修复」**（与 code action 并列，直接发起代理任务）、格式化、折叠与代码镜头；AI 行内补全为实验特性默认关。

### 8.4 语言包体系

| 语言包 | 服务器 / 工具 | 运行时 | 方式 |
|---|---|---|---|
| TypeScript/JS | typescript-language-server + prettier | Node | 内置 |
| Python | Pyright + Ruff | Node | 内置 |
| C# | Roslyn 语言服务（已定稿，Q2；官方 VSIX 锁定版本提取打包）+ dotnet format | .NET SDK ≥8（检测引导） | 一键安装 |
| Rust | rust-analyzer + rustfmt | rustup | 一键安装 |
| Go | gopls | Go | 一键安装 |
| C/C++ | clangd | 系统 | 按需 |
| JSON/MD/YAML | 内置轻量器 + tree-sitter | 无 | 内置 |

规则：项目感知推荐（检测 `package.json` / `*.csproj` / `pyproject.toml`…）。**运行时分发策略**：安装包本体不内置 Node/.NET 等运行时（保包体），语言包首次安装时检测运行时，缺失提供两条路径——① 官方源一键下载锁定版运行时（D 级审批 + 签名校验，装入 `~/.tenon/runtimes/`，仅语言包沙箱可见、不污染系统）；② 官方指引自装（不代装原则保持）。语言包与运行时均版本锁定 + 签名校验；Open VSX 仅语言类子集实验兼容。

### 8.5 Agent ↔ 编辑器共生（差异化核心）

| 集成点 | 方向 | 说明 |
|---|---|---|
| 共享 LSP 多路复用 | 双向 | 一项目一 LSP 实例，编辑器与代理共用：索引一次、内存减半、行为一致 |
| 诊断流入代理 | 编辑器→代理 | 验证走测试 + LSP 双通道；诊断点「AI 修复」即发起任务 |
| 代理改动流入编辑器 | 代理→编辑器 | 实时写入缓冲并高亮「AI 修改」区；未打开文件在文件树标记 |
| 行内指令 | 用户→代理 | 选中代码自然语言改写，diff 就地呈现 |
| 跟随模式 | 代理→编辑器 | 自动滚动到代理正在修改处（可关） |
| 工作区重构原子应用 | 双向 | LSP workspace edits 一次应用 + 单 checkpoint，失败整体回滚 |

### 8.6 人机共编冲突处理

代理写盘前检查目标文件用户未保存缓冲 → 有脏缓冲则暂停并弹「你的改动 / 代理改动 / 合并结果」三栏；被 AI 修改的行带标记，用户编辑同区域自动解除；任何合并失误可回 checkpoint。

### 8.7 性能预算（验收级）

| 指标 | 预算 |
|---|---|
| 冷启动到可输入 | < 1.5s（P50） |
| 10MB 文件只读打开 | < 2s |
| 全局搜索（10 万文件）首结果 | < 500ms |
| LSP 索引就绪后首个补全（中型 TS 项目） | < 400ms（冷启动到首个可用补全 < 3s） |
| 补全列表呈现 | < 80ms（P50） |
| 万行 diff 渲染 | 60fps |
| 项目切换到可交互 | < 150ms（P50；runtime 已打开；冷项目按打开流程另计） |
| 多项目稳态 | 12 个打开项目下 UI 主线程不因任一项目 watcher / LSP 输出阻塞；非活跃项目 LSP 可被资源控制器回收 |

---

## 9. Agent 内核设计

### 9.1 自主决策状态机

```text
IDLE → SENSING → DECIDING ──无需改──→ ANSWERING → DONE
                      │ 需要改（输出意图卡）
                      ↓
              [首改缓冲 2s / Esc 可断]   ← 缓冲位于进入执行之前（§9.3）
                      ↓
                  EXECUTING（B 级沙箱）──遇 C/D──→ AWAITING_APPROVAL
                      ↑ └─允许（一次/本会话）→ 回 EXECUTING 继续剩余步骤
                      │    拒绝 → DECIDING 改案重试 ≤2 次（超出 → PAUSED）
                      ↓
                  VERIFYING ──失败──→ FIXING（≤3 轮，收敛）──→ 回 VERIFYING
                      │ 通过
                      ↓
                  SUMMARIZING（证据卡） → DONE

  侧向出口：任意状态 ─Esc / 熔断 / 审批超时→ PAUSED ─继续→ 回断点状态；─中止→ ROLLED_BACK（最近 checkpoint）
            模型 / 供应商 / 网络失败 → ERROR ─重试 ≤2 / 切模型（上下文随迁，§11）/ 中止→ ROLLED_BACK
```

| 状态 | 说明 | 用户可见 |
|---|---|---|
| SENSING | A 级只读探索（文件 / 检索 / LSP 查询） | 蓝色「感知中」+ 活动列表 |
| DECIDING | 模型判断是否 / 如何修改；输出意图一句话 | 判断卡（改 X，因为 Y） |
| EXECUTING | B 级沙箱写 / 命令；C/D 转审批 | 黄色；编辑器实时高亮 |
| VERIFYING | 测试 + LSP 诊断双通道 | 紫色；证据流 |
| FIXING | 收敛条件下的修复循环（§9.4） | 轮次摘要 |
| AWAITING_APPROVAL | C / D / C+D 审批卡 | 橙色系统通知 |
| PAUSED | Esc / 熔断器触发 / 审批超时 | 停在最近 checkpoint |
| ERROR | 模型 / 供应商 / 网络失败 | 红色；重试 ≤2 / 切模型（上下文随迁）/ 中止 |

**关键转移规则**（图中省略的边）：

| 转移 | 条件与去向 |
|---|---|
| DECIDING → EXECUTING | 需要修改；**进入执行前执行首改缓冲（2s，Esc → PAUSED，§9.3）** |
| AWAITING_APPROVAL → EXECUTING | 用户允许（一次 / 本会话）→ 继续执行剩余步骤，仍须过验证，不直接跳 SUMMARIZING |
| AWAITING_APPROVAL → DECIDING | 用户拒绝 → 代理改案重试 ≤2 次；超出转 PAUSED 待新指令 |
| AWAITING_APPROVAL → PAUSED | 审批超时（默认 5 分钟，可配置） |
| VERIFYING ⇄ FIXING | 失败且满足收敛条件（§9.4），轮次 ≤3；不满足即停 |
| 任意 → ERROR | 模型 / 供应商 / 网络失败（降级路径见 §11） |
| 任意 → PAUSED | Esc / 全局暂停 / 熔断器触发 |
| PAUSED → 断点状态 | 用户继续；从最近状态恢复，不重放已写入改动 |
| PAUSED / DONE → ROLLED_BACK | 用户中止，或时间轴回滚（两种粒度，§7.3） |

> DECIDING 之前存在可选的 Laya 本地意图预判（§9.8）：不新增状态、不改变上述转移规则；模型不可用时整体跳过，行为与本节状态机一致。

### 9.2 内置工具协议

| 工具 | 级 | 说明 |
|---|---|---|
| `read_file` / `list_dir` / `grep` | A | 只读；核心服务 rg |
| `git_read`（status/log/diff） | A | 只读 git |
| `lsp_query`（定义/引用/符号/hover） | A | 共享 LSP 多路复用 |
| `apply_patch` | B | 结构化编辑（file + range + content），产生事件与 checkpoint |
| `run_tests` / `run_build` | B | 沙箱内，断网态；单命令超时默认 120s（附录 E） |
| `install_deps` | B | 沙箱内，镜像代理态 |
| `http_fetch` | C | 审批后域名代理态 |
| `git_commit` / `git_push` / `create_pr` | D | 永远审批；PR 为 C+D 复合卡；批量任务的多个 commit 可合并为一张复合 D 卡（逐条列明、一次批准，§12.2） |
| `plugin_*` | 按声明 | 外部进程插件提供，映射分级 |

### 9.3 事中防护

| 机制 | 默认 | 说明 |
|---|---|---|
| 首改缓冲 | 2s 可关 | 首个 B 级前高亮「即将修改 X」，Esc 打断 |
| 全局暂停 | Esc / 托盘 | 立即暂停，停最近 checkpoint |
| 任务熔断 | >15 文件 / >1500 行 / 超预算 | 暂停待确认——断路器而非 Plan；预算默认 = 单任务 500k token 或 $5 等值（先到为准；本地模型仅计 token；config.toml 可调，全文见附录 E） |
| 只读开关 | 会话级 | 禁用 B 级，不依赖模型语义理解 |
| 模型能力适配 | 按模型 | 弱本地模型建议交互档 / 收紧熔断 |

### 9.4 验证与收敛

验证 = 测试（沙箱）+ LSP 诊断（双通道）。**无测试仓库降级通道**：以「沙箱构建 + LSP 诊断 + 代理读回 diff 自检」三项替代，证据卡明示「验证强度：低」，修复循环降为 ≤1 轮并建议补测试；新项目脚手架（S6）以模板自带自检脚本为准。修复循环任一不满足即停：失败数不增、diff 每轮 ≤ 上轮 1.5 倍、无新诊断 / 告警、轮次 ≤3；每轮输出摘要可回滚。

### 9.5 子代理编排

静态检查任务文件集不相交（相交拒绝 / 转串行）→ 每子代理独立 worktree + 沙箱（worktree 由内核托管，创建于 `~/.tenon/worktrees/`，对 `.git` 的元数据写入计入 **B 级项目内写**、不经审批，不触碰用户工作区）→ 子代理运行期主代理只读 → 冲突 = 失败回传用户处置 → 并发 ≤3 × 单代理 token 预算硬上限。

### 9.6 提示组装与模型适配

**系统提示组成**：身份与目标 / 安全铁律（C/D 只信会话指令、输出证据契约）/ 项目规则 L3（AGENTS.md，只收窄）/ 会话记忆 L2 / 工具 schema / 输出契约（意图一句话 → 结构化动作 → 证据）。

**模型能力矩阵**：

| 供应商 | 工具调用 | 流式 | 备注 |
|---|---|---|---|
| OpenAI / Anthropic / DeepSeek | ✅ | ✅ | 原生适配 |
| OpenAI 兼容端点 | ✅（遵循规范） | ✅ | 通用 provider，覆盖大量供应商 |
| Ollama 本地 | ✅（带适配层） | ✅ | 零 Key 起步；弱模型自动建议降档 |

### 9.7 项目内多会话与跨项目并发

- **项目级写锁**：同一项目同一时刻仅允许一个会话处于 EXECUTING；其余会话限 SENSING / 只读，或排队等待写锁；写锁只约束代理会话——用户编辑不受限，与代理的并发冲突仍走 §8.6 脏缓冲协调；
- **checkpoint 隔离**：快照库按项目 × worktree 隔离（`~/.tenon/snapshots/`，§10.3），不写用户仓库；快照点归属会话（checkpoints 表，§14.2），会话只能回滚自己链上的快照；
- **回滚冲突检测**：回滚前检查工作区是否含其他会话或用户的未合并改动，有则先出三方合并预览（§8.6），不静默覆盖；
- 共享 LSP 实例跨会话多路复用，请求按会话路由与限流（§8.5）。
- **跨项目并发**：全局调度器允许不同项目各自运行一个 EXECUTING 会话，但跨项目并发上限默认 2（可配置）；全局 token / 成本 / CPU / 磁盘预算先到即排队。每个事件、审批、diff 和成本都带 `project_id`，项目中心据此聚合；
- **跨项目隔离**：A 项目会话的 L1/L2/L4 上下文、审批记忆、沙箱 profile、脏缓冲与 LSP 请求不得进入 B 项目。若用户下达跨项目诉求，系统转「项目组合任务」创建多条项目内子会话；父任务只能携带用户目标与子任务摘要，不能把 A 的文件内容注入 B；
- **项目生命周期协调**：关闭项目时先拒绝新任务，再暂停 / 排空 EXECUTING 会话并 flush 事件与 checkpoint；崩溃恢复按 `project_id` 分组，恢复一个项目不锁住其他项目。

### 9.8 本地决策模型（Laya）加速层

**定位**：产品自管的小型本地分类模型（Laya），只做结构化判定——选项分类（choice）/ 量表打分（score）/ 布尔判断（noul）三类原语；本地 CPU 推理（~30ms 级）、零 token 成本；**不生成代码、不做开放问答**。价值：把代理循环中不值得动用大模型的结构化判定下沉到本地——缩短回合延迟、压缩进入大模型的上下文、降低云端 token 开销。

**集成点**（全部为辅助判定，逐项可独立开关；序号供附录 E 引用）：

| # | 用途 | 说明 | 收益 |
|---|---|---|---|
| 1 | 意图预判 | DECIDING 前对用户消息分类（纯问答 / 需改动 / 只读分析 / 需出网），为大模型提供先验与路由建议；不改变 §9.1 状态机转移 | 减少无效轮次，辅助 #4 |
| 2 | 命令风险辅助 | §12.2 规则引擎为主、Laya 为规则库外命令补盲区（借鉴 codex execpolicy 思路，§3.1）；打分用于风险说明与「建议人工确认」提示，**不改变 A/B/C/D 分级与档位语义、不替代 C/D 审批**（铁律一不受影响） | 高风险早暴露，减少事后回滚 |
| 3 | 上下文预筛 | §10.1 L1 工作集：对 L4 召回的候选切片做相关性打分，仅 top-k 进入大模型上下文 | 直接降低每步 token 消耗 |
| 4 | 路由启发式 | 承接 §11「纯读任务提示轻模型」的轻量启发式（展示建议、一键采纳；auto 路由仍为实验特性默认关，§5 非目标 8） | 云端 token 成本下降 |
| 5 | 批量 triage | S3 批量任务的类别 / 优先级标注，辅助子代理拆分（文件集不相交仍由静态检查保证，§9.5） | 批量场景准备阶段提速 |

**分发与生命周期**：模型文件不进安装包（保包体，同 §8.4 运行时分发原则）；首次启动引导提供「启用本地加速（下载本地决策模型）」步骤（默认勾选），跳过则首次使用时再提示，两次拒绝后仅设置页手动开启；下载走官方静态 registry（附录 C Q1），版本锁定 + 签名校验 + SHA-256 展示，**过一次 D 级审批卡**（与 §8.4 运行时下载同款——「自动下载」指用户无需手动获取与配置模型，审批不变式不破例）；版本升级同样过卡。模型存 `~/.tenon/models/laya/`（§14.1），由 daemon 内置 Rust 推理运行时加载（不额外进程、不进 WebView、推理不出网）；中英输入自动路由对应语言变体（模型随发双变体，与 Q5 中英同期一致）。

**边界与兜底**：

- 判定输出只用于**排序、提示、预筛**，绝不直接产生动作、绝不放宽审批（C/D 恒审批不变）；
- 置信度未校准，仅作排序参考；zero-shot 精度有限——判定准则须具体化，关键集成点（#1 / #2）上线前须在领域语料上验证；
- 模型输入含仓库文本（不可信数据，§12.1）→ 判定结果同样按数据处理，最坏影响仅为排序失真；
- **整体可回退**：未下载 / 加载失败 / 单次推理超时（默认 200ms）→ 对应集成点回退现状（大模型直判 / 纯规则 / 全量上下文），任何功能不阻塞；
- 判定调用入 Trace（events `decider_call`：判定类型 / 结果 / 耗时，不含输入原文，§14.2）；集成点与判定阈值变更视同提示词变更，触发 Evals 门（§18.3），验收「基准通过率不降、token 消耗下降」。

---

## 10. 上下文工程

### 10.1 四层记忆

| 层 | 内容 | 生命周期 | 来源 |
|---|---|---|---|
| L1 工作集 | 任务相关文件 / 符号切片 | 任务内 | 共享 LSP + tree-sitter + 混合检索；Laya 相关性预筛（§9.8） |
| L2 会话记忆 | 目标 / 步骤 / 决策 | 会话内自动压缩 | 主循环维护 |
| L3 项目规则 | AGENTS.md、规范、构建命令 | 仓库内 | 文件 + 用户配置 |
| L4 持久索引 | 符号 / 向量 / 倒排 | 本地跨会话，**按 `project_id` 隔离** | **自建索引层**（向量部分用 sqlite-vec，与 `db.sqlite` 同库，Q3；增量更新）；LSP 仅任务期查询活实例——各语言服务器缓存为私有格式，不可跨会话复用，也不跨项目复用 |

### 10.2 Token 预算与压缩

每步计算预算（窗口 − 输出预留 − 安全余量）；L2 超限触发 compaction（保留目标 / 决策 / 未完成步骤）；压缩后自检问答通过才继续；压缩事件入 Trace 可查。

### 10.3 会话恢复与 checkpoint（独立 shadow git 快照库，v1.9 参考 opencode 重构）

> 机制源码级参考 opencode（sst/opencode `src/snapshot/index.ts` + `src/session/revert.ts`，§3.2 #7）；Tenon 在快照时机与用户手改保护上强于原版（见下）。

- **独立 shadow git 快照库**：daemon 为每个项目 × worktree 维护一个独立的 shadow git 仓库（`~/.tenon/snapshots/<project-id>/<worktree-hash>/`），与用户仓库完全隔离——不向 `.git` 写任何 refs / objects / index，不触发用户仓库的 hooks 与 gc。把快照状态内嵌用户 `.git` 已有用户反弹实证（sst/opencode#10861，早期 `.git/opencode` 方案），独立库是其修正后的形态；
- **统一单路径**：git 与非 git 项目同用 shadow git 快照（替代原「shadow ref + CoW」双轨，ADR-7 修订）。git 项目享受**种子加速**——shadow 库 `objects/info/alternates` 指向用户仓库对象库（含其 alternates 链）并复制其 index，已哈希对象直接复用，巨型仓库（chromium 级）首次快照从分钟级降到近零；非 git 项目无种子，首次全量 add，成本在打开项目时明示；
- **快照点 = tree oid**：增量 add（`diff-files` 变更 + `ls-files --others` 未跟踪）后 `git write-tree`，返回 tree oid 作为快照点——无 ref 链、无 commit，轻量且天然去重；尊重用户仓库 `.gitignore`（`check-ignore --no-index` 动态判定，新增忽略项自动从快照索引移除）；未跟踪大文件（默认 >2MB，可配置）排除；大仓库调优（untrackedCache / manyFiles / index.version=4，参考 opencode）；每个 shadow 库一把互斥锁，快照与回滚串行；
- **快照时机（不变式的落地，较 opencode 的 model-step 级更密）**：任务开始前、每个 B 级补丁 / 写盘命令前（与写入同事务：先快照后写入）、每轮 FIXING 前——保证 EXECUTING 中崩溃回滚不丢半步；git 对象级去重使增量成本可接受；每次写入随事件记录**该步快照点 + 改动文件集**（patch），支撑「按事件撤销」；
- **回滚双原语**（对应 §7.3 两粒度）：
  1. **按事件撤销 revert**（默认）：逐文件 `checkout <快照> -- <file>` 恢复到目标事件前状态；快照中不存在该文件 → 删除（AI 新建文件随撤销移除）；目标事件之外的用户手改文件不受触碰；同文件用户手改走三方合并保留（§8.6，强于 opencode 直接覆盖）；
  2. **整体恢复 restore**（高级）：`read-tree` + `checkout-index -a -f` 回到快照全量状态（含用户手改，预览警示）；
- **撤销回滚（unrevert）**：每次回滚前自动追加快照（记录回滚前状态），时间轴支持一键撤销最近一次回滚——回滚是双向操作；
- **保留与清理**：每会话保留最近 50 个或 7 天（可配置）+ shadow 库定时 `git gc --prune=7d`（每小时，后台）——仅影响「checkpoint 整体恢复」；「按事件撤销」依赖事件日志，不受快照清理影响；事件日志不受此限（归档策略见 §14.2）；
- **崩溃恢复语义**：恢复 = 回滚到最近快照 + 依据事件日志重建会话上下文；EXECUTING 中崩溃不承诺原地续跑；
- 快照库不可用（关闭 / 磁盘满 / 损坏）→ 自动降交互档（「自动 = 必可回滚」不变式）；
- 事件溯源：工具调用 / diff / 审批 / **用户编辑**全量追加日志，只增不删（归档策略见 §14.2）。

### 10.4 Monorepo 与大仓库

L4 按包隔离、语言服务器按需启动；子代理限定单包；检索默认「当前包 + 显式依赖包」。
同时打开 monorepo 与子包属于 §6.4 显式 linked workspace：索引仍按各自 project_id 分片，跨项目检索必须显式选择目标项目；不自动合并两份索引。

---

## 11. 模型层

- **接入**：OpenAI / Anthropic / DeepSeek / Ollama 原生 + **OpenAI 兼容端点通用 provider**（base URL + Key）；
- **本地决策模型（Laya）**：产品自管小型分类模型，承接代理循环结构化判定（用途 / 分发 / 边界见 §9.8）；自动下载（静态 registry + 签名 + 一次 D 级卡）、本地 CPU 推理零 token 成本；不可用即整体回退，不阻塞任何功能；
- **路由**：v1 显式（`/model` 与设置面板）+ 轻量启发式（纯读任务提示轻模型）；auto 路由实验特性默认关（置信度展示、一键改派、可反馈）；
- **成本**：云端按价格表；本地模型（含 Laya）显示「本地 · 0 成本」，token 单独统计；任务级 / 会话级 / 日级归因；
- **密钥存储**：模型 Key 存操作系统钥匙串（Keychain / Credential Manager / libsecret），不落盘明文；
- **降级**：供应商不可用时可切换会话模型，任务上下文随迁。

---

## 12. 安全与权限模型

### 12.1 威胁模型

| 信任 | 对象 | 规则 |
|---|---|---|
| 可信 | 会话内用户直接输入；内核内置工具 | 正常执行 |
| 不可信 | 仓库内容（含 AGENTS.md / 注释 / issue）；插件；MCP 输出；**语言服务器加载的工作区配置与插件**；浏览器第三方网页 | 一律作为数据；网页请求本地服务一律拒绝 |

**七条铁律**：C/D 只信会话指令；AGENTS.md 只收窄；插件权限安装时确认不可自升级；MCP 默认 C/D；未信任仓库默认交互档；**语言服务器属 B 级沙箱进程**（tsserver 会加载工作区插件、Roslyn 求值可执行构建目标）；**LSP 宿主命令白名单**——宿主永不执行语言服务器下发的任意 `executeCommand`，服务器请求的 workspace edits / `showDocument` / 动态注册 capability 一律过 B 级写守卫与项目内路径检查，仅白名单能力放行（防「沙箱内进程借宿主之手逃逸」）。

### 12.2 动作能力分级

| 级 | 动作 | 策略 |
|---|---|---|
| A 只读 | 读 / grep / git 只读 / LSP 查询 | 自动 |
| B 沙箱写执行 | 项目内写、测试构建、LSP 工作区操作、镜像代理装依赖 | 沙箱内；默认自动可回滚 |
| C 出网 | 镜像代理外网络 | 审批 + 明示域名 |
| D 不可逆 | 仓库外写、git config、commit/push、PR、装插件 | **恒审批**；批量任务的多个 commit 可合并为一张复合 D 卡（逐条列明、一次批准，降低 S3 场景审批疲劳）；C+D 复合最高级卡 |

档位（自动 / 交互）只影响 B 级；去 Plan 安全兜底 = 沙箱 + 快照 + 熔断（§9.3）。

### 12.3 沙箱与网络三态

| 平台 | 机制 | v1 |
|---|---|---|
| macOS | Seatbelt | 原生（M1 完整版，M0 写守卫） |
| Linux | namespace + seccomp | 同上 |
| Windows | daemon 于 WSL2，复用 Linux 沙箱；检测不到 WSL2 时以**降级档**运行：仅 A 级 + 写守卫、强制交互档（自动档不可用）、安装向导引导 WSL2 | 完整沙箱经 WSL2 |

网络三态：**断网**（测试/构建/纯分析）→ **镜像代理**（仅预授权 registry：npm/pypi/nuget/crates…，B 级）→ **域名代理**（审批后白名单，C 级）。系统只读、写限当前会话绑定的项目根 / 显式 worktree、每项目语言服务器隔离。多项目打开时为每个执行进程分别物化 project roots；跨项目路径既不是可写根，也不进入 A 级检索范围。

### 12.4 密钥处理

`.env` 与常见 Key 模式检测 → 默认拦截不进上下文 → 需要时用户显式注入「脱敏引用」（`env:STRIPE_KEY`）；**拦截同样覆盖 `git_read` 输出**（log / diff / show——历史中曾提交的密钥同样脱敏）；不承诺识别一切形态；Trace 标注拦截 / 放行。

### 12.5 插件与语言包供应链

仅官方 registry、签名 + 版本锁定、安装展示权限 diff 与运行时依赖、插件最小权限自有沙箱、调用全量入 Trace、保留字防 typosquatting。

### 12.6 本地服务与浏览器访问

默认仅绑 127.0.0.1 + 随机端口；HTTP 用 `X-Tenon-Token` 头；**WS 用一次性 ticket**（浏览器 WebSocket 无法自定义请求头：`POST /ws-ticket` 换 60 秒一次性票据，连接首帧携带，重放即拒）；校验 Origin/Host；**CORS 仅白名单放行应用自身 origin（Tauri WebView 源）与已配对设备，其余拒绝**；局域网显式开启 + 一次性配对 + 可吊销；浏览器只能切换已登记项目，只能提交项目 ID（无法传本地路径，也无法新增本地项目登记）；TOFU 卡仍在桌面优先完成，未信任项目强制交互档；公网访问非目标。

### 12.7 仓库信任（TOFU）

每个项目独立 TOFU；首次打开默认交互档 → 用户「信任此仓库」后按全局默认档 → 本地可吊销；信任只放宽 B 级档位，永不放宽 C/D。项目 A 的信任、会话审批记忆与项目设置不适用于项目 B；项目组合任务的每条子会话仍按各自项目 TOFU 判定。

---

## 13. 插件与扩展体系

### 13.1 插件 manifest（示例）

```yaml
id: official.python
version: 1.2.0
runtime: external          # external | wasm
permissions:
  - fs.read:project
  - net:registry:pypi      # 镜像代理
provides:
  languages: [python]
  lsp: { command: "pyright-langserver", args: ["--stdio"] }
  formatters: [ruff]
  linters: [ruff]
requires:
  runtime: { node: ">=18" }
signature: "<sig>"
```

### 13.2 安装与升级流程

registry 检索 → 展示**权限 diff**（相对已装版本新增权限高亮）→ 用户确认（D 级）→ 签名校验 + 版本锁定 → 沙箱内启动；升级同样走 diff 确认；语言包升级需过 Evals 门（§18.3）。

### 13.3 生态兼容

| 来源 | 策略 |
|---|---|
| MCP | 外部进程插件直连；工具映射动作分级，默认 C/D |
| AGENTS.md | 直读，只能收窄权限 |
| Open VSX | 实验兼容：仅声明 `languageContribution` 的扩展转内部语言包；主题可选；其余不支持 |
| Skills | 尽力兼容主流格式，实验特性 |

---

## 14. 数据与存储设计

### 14.1 本地目录布局

```text
~/.tenon/
├── config.toml          # 全局配置（不含密钥；schema 见附录 E）
├── db.sqlite            # 会话/事件/Trace/成本
├── snapshots/           # shadow git 快照库（按项目 × worktree 哈希分库，§10.3）
├── cache/
│   ├── index/           # L4 符号 / 倒排索引（自建文件）；向量在 db.sqlite（sqlite-vec，Q3）
│   └── models/          # 本地模型缓存（Ollama 自管）
├── plugins/             # 已安装插件与语言包
├── runtimes/            # 语言包专用锁定版运行时（Node 等，仅语言包沙箱可见，不污染系统）
├── models/              # 产品自管本地模型（Laya 决策模型，§9.8；Ollama 模型仍由其自管于 cache/models）
├── worktrees/           # 子代理独立 worktree（§9.5，内核托管）
├── skills/
├── archive/             # 冷归档（压缩事件日志，见 §14.2）
└── logs/
```

### 14.2 核心数据表（SQLite）

| 表 | 关键字段 | 说明 |
|---|---|---|
| projects | id, canonical_path, display_name, trusted, status, language_packs, settings_json, last_opened_at | 项目登记 / TOFU / 打开状态与项目覆盖配置；canonical path 唯一 |
| sessions | id, project_id, cwd, worktree_id, model, status | 会话；project_id 不可空，cwd 必须位于项目根或登记 worktree |
| events | id, session_id, project_id, seq, type, payload | 事件溯源（只追加；project_id 供跨项目任务中心聚合） |
| checkpoints | id, session_id, tree, files, created_at | 快照点（tree oid + 该步改动文件集，§10.3） |
| tool_calls | id, event_id, tool, level, cost_tokens | AgentTrace 明细 |
| approvals | id, session_id, project_id, action, level, decision | 审计；通知 UI 按项目分组 |
| plugins | id, version, permissions, signature | 安装记录 |
| model_usage | id, session_id, project_id, provider, tokens, cost | 成本归因；支持会话 / 项目 / 日级 |
| eval_runs | id, target, metrics_json, verdict | Evals 报告 |
| l4_chunks | id, project_id, path, symbol, embedding | L4 检索切片与向量（sqlite-vec 虚表，Q3） |

事件类型枚举：`user_input / sensing / decision / patch_applied / command_run / diagnostics / approval_request / approval_decision / approval_timeout / checkpoint / compaction / rollback / unrollback / model_fallback / decider_call / error`（approval_timeout、model_fallback 对应 §9.1 审批超时与切模型降级；rollback / unrollback 对应 §10.3 回滚与撤销回滚；decider_call 为 §9.8 Laya 本地判定：类型 / 结果 / 耗时，不含输入原文；均入 Trace 可审计）。

**增长治理**：events / tool_calls 冷热分层——热数据留 SQLite，关闭超 90 天（可配置）的会话压缩归档至 `~/.tenon/archive/`（仍全本地、可检索回载）；approvals 审计记录永久保留；model_usage 明细随会话归档，另维护按月聚合表（永久，支撑 §11 成本归因）。

---

## 15. 本地 API（UI ↔ daemon）

认证：HTTP 用 `X-Tenon-Token` 请求头（随机，随端口握手下发）；**WS 先经 `POST /ws-ticket` 换 60 秒一次性 ticket**（浏览器 WebSocket 无法自定义请求头），连接首帧携带、重放即拒；来源校验与 CORS 白名单见 §12.6。

**项目运行时（多项目控制面）**：

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | `/projects` | 已登记项目 + 打开状态、活跃会话、审批 / 任务 / 成本 / 脏缓冲摘要 |
| POST | `/projects/open` | 按 path 打开或复用登记项目（canonicalize、去重、TOFU、懒启动 runtime） |
| POST | `/projects/:id/close` | 关闭 / 暂停项目；body 指定 drain / pause / force |
| DELETE | `/projects/:id` | 移除登记（不删除磁盘内容） |
| PUT | `/project/:id/trust` | 设置该项目 TOFU 信任 |

**会话与审批**：

| 方法 | 路径 | 用途 |
|---|---|---|
| POST | `/session` | 创建会话；body 必须带 `project_id`，可附项目内 `cwd` / `worktree_id`（模型、档位） |
| POST | `/session/:id/message` | 发送任务 |
| POST | `/session/:id/control` | pause / resume / stop / rollback（快捷回滚至最近 checkpoint，等价于 `/checkpoint/:id/rollback` 最近点，勿单独实现第二条路径）/ unrollback（撤销最近回滚，§10.3）/ set_readonly |
| POST | `/approval/:id` | 审批决策（once / session / deny） |
| GET | `/session/:id/trace` | Trace 查询 |
| GET | `/session/:id/checkpoints` | checkpoint 时间轴（事件列表 + 快照点） |
| GET | `/portfolio-tasks` | 跨项目组合任务聚合视图（父任务状态、子会话、审批、成本） |
| POST | `/portfolio-tasks` | 创建项目组合任务；body 是 project-scoped child task 数组，父任务不共享代码上下文 |
| POST | `/checkpoint/:id/rollback` | 回滚（body 指定粒度：checkpoint 级 restore / 按事件 revert，§7.3） |

**编辑器与文件**（UI 为纯 React，文件与语言智能全在此 API 之上）：

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | `/project/:id/tree` | 文件树（增量，含 git 状态装饰） |
| GET / PUT | `/project/:id/file` | 读取 / 保存项目内相对路径文件（保存走脏缓冲协调，§8.6） |
| POST | `/project/:id/file/ops` | 新建 / 重命名 / 移动 / 删除 |
| GET | `/project/:id/search` | ripgrep 搜索（流式；多文件替换前返回 diff 预览） |
| POST | `/project/:id/lsp` | LSP 代理（补全 / hover / 定义 / 引用 / 重命名 / code action / 格式化） |

**管理**：

| 方法 | 路径 | 用途 |
|---|---|---|
| GET / POST | `/plugins` | 插件与语言包管理（权限 diff、安装、升级） |
| GET / PUT | `/settings` | 全局 / 项目设置（模型、权限、隐私、更新） |
| GET | `/models` | 模型清单与 Laya 状态（版本 / 已下载 / 加载 / 设备，§9.8） |
| GET | `/costs` | 成本归因（任务 / 会话 / 项目 / 日级） |
| POST | `/ws-ticket` | 一次性 WS 票据 |
| WS | `/ws` | 事件流（状态机、诊断、diff 流、审批；ticket 鉴权） |

WS 事件与会话 events 表一一对应，均含 `project_id`；断线重连按 seq 续传。UI 可订阅全部项目或过滤一个项目。

**项目作用域规则**：旧 `/file`、`/search`、`/lsp` 这类隐式首项目接口在 v1.15 后禁止新增调用；迁移期保留时必须带 `project_id`，daemon 找不到显式项目即返回 `409 PROJECT_REQUIRED`。相对路径由 server 端 join + canonicalize + 前缀校验，任何越界路径返回 `400 PATH_ESCAPE`。

---

# 第三部分 · 交付与质量

## 16. 技术选型

| 层 | 选择 | 理由（含被否方案） |
|---|---|---|
| 内核 / daemon | Rust | 沙箱与文件控制力、单二进制；否 Go（沙箱生态弱） |
| 桌面壳 | Tauri 2 | 包小、与内核同语言；否 Electron（内存/包体） |
| UI | React + TS | 单代码库桌面/浏览器；生态 |
| 编辑器 | Monaco | VS Code 同源体验；否 CodeMirror（交互需大量自建）、否 fork VS Code（= 重造） |
| 高亮 | tree-sitter + LSP tokens | 双层高亮；tree-sitter 在 daemon 侧 Rust 增量解析、token 流送 UI，不进 WebView（大文件性能与内存） |
| LSP 宿主 | Rust 内置 | 多路复用 + 沙箱化 + 项目隔离 |
| 搜索 / 监听 | ripgrep + notify | 内核内置，不进 JS |
| 沙箱 | Seatbelt / ns+seccomp / WSL2 | 见 §12.3 |
| 存储 | SQLite + sqlite-vec | 本地、可靠、易审计；向量检索同库（Q3） |
| 本地决策模型 | Laya（分类 / 打分 / 布尔三原语，CPU ~30ms 级） | 承接代理循环结构化判定，降延迟降 token（§9.8）；否 纯规则引擎（语义盲区大）、否 大模型全量判定（延迟与成本高） |
| License | Apache-2.0 | 专利授权利于企业采用 |

**隐私口径**：本地数据（会话 / Trace / 索引）不出本机；云模型调用按用户显式配置发送并在 UI 明示；更新默认手动检查；崩溃 / 行为报告 opt-in 且明示内容；遥测默认零上报。

---

## 17. 路线图

**团队假设**：4-5 全职（Rust×2、前端×1.5、语言/工具链×1、产品/代理×0.5）+ 1 兼职设计。

| 阶段 | 周期 | 交付 | 验收 |
|---|---|---|---|
| **M0 原型** | 6 周 | Tauri 壳 + daemon（sidecar/端口/单实例）、文件树 + Monaco 基础编辑器（tree-sitter / 标签 / fuzzy / rg 搜索）、单模型会话、A/B 写守卫、diff 面板、签名与 updater 密钥、UI i18n 骨架（英文源 / 中文资源，Q5）、monorepo 脚手架 + CI 骨架（ADR-13）；决策 spike：Roslyn LS 沙箱化、sqlite-vec 冒烟（附录 C） | 10 内部任务一次通过 ≥50%；冷启动 <1.5s |
| **M1 可用 IDE** | +16 周（分两段：前段 LSP 宿主 + 语言包 + 智能全家桶；后段共生 / 共编 / 完整沙箱 / 恢复） | LSP 宿主 + TS/Py/C#/Rust/Go 语言包 + 智能全家桶、Agent↔编辑器共生、共编冲突合并、完整沙箱三态、shadow git 快照库（回滚 / 撤销回滚，§10.3）、TOFU、崩溃恢复、模型路由（4 家 + 兼容端点）、本地决策模型 Laya（自动下载 + §9.8 集成点 1-4）；**v1.15 补齐项：ProjectRegistry/Runtime、project_id-scoped API、项目切换器 / 任务中心、跨项目并发与关闭协调** | 基准通过 ≥60%；索引就绪后首补全 <400ms（冷启动首补全 <3s）；崩溃 100% 恢复（回滚语义，§10.3）；安全违规 0；Laya 集成后基准 token 不升、通过率不降（§18.3）；3 项目同时打开且 2 项目并发执行，切换可交互 <150ms、无路径越界 |
| **M2 生态** | +12 周 | 官方 registry、MCP、Open VSX 语言子集实验、并行子代理、AgentTrace UI、本机浏览器访问（127.0.0.1，含来源校验；局域网后移 M3，Q4）、Laya 批量 triage 与 token 节省归因（§9.8 集成点 5）、项目组合任务 v1 | 3 并行子代理成功 ≥70%；App 内编辑动作占比 ≥70%；组合任务子会话均项目隔离且聚合成本一致 |
| **M3 团队与打磨** | +12 周 | AI Evals 流水线、团队策略、局域网配对浏览器访问（Q4）、Windows WSL2 安装包、**可选**自动更新通道（默认仍为手动检查）、性能打磨 | Evals 报告自动产出；万行 diff 60fps |
| 后续 | 数据决策 | Windows 原生沙箱（优先研读 codex `windows-sandbox-rs`，§3.1）、auto 路由转正、终端区 / IDE 开放协议（对齐 Zed ACP，不自造，§3.2） | 不预先承诺 |

---

## 18. 测试与质量策略

### 18.1 分层测试

| 层 | 工具 | 覆盖 |
|---|---|---|
| Rust 单元 / 集成 | cargo test | 权限判定、sandbox profile、checkpoint、LSP 宿主、ProjectRegistry/Runtime、跨项目路径隔离 |
| 前端 | vitest + Playwright | 组件、四区布局、项目切换器 / 任务中心、审批流、键盘全可达 |
| 端到端 | Playwright（真 daemon） | 打开项目 → 任务 → 审批 → 回滚全链路；A 执行中打开 B 并完成任务；关闭 A 不中断 B |

### 18.2 安全测试

- **沙箱逃逸套件**：断网态网络调用、路径逃逸、进程注入、hooks 触发（npm postinstall / conftest / pre-commit）必须全部被拦截或沙箱化；
- **Prompt 注入语料库**：仓库注释 / AGENTS.md / MCP 输出中的注入样本 → 断言不触发 C/D；
- **本地服务 fuzz**：畸形 Origin / Host / Token、跨站请求、重放、WS ticket 重放与过期；
- **LSP 客户端表面测试**：恶意语言服务器下发任意 `executeCommand`、动态注册 capability、超范围 workspace edits、`showDocument` 指向项目外路径——必须全部被拒（§12.1 铁律七）；
- **多项目隔离套件**：同一 daemon 打开 A/B；A 会话工具请求解析到 B 路径、A 的 LSP / 脏缓冲 / L4 检索 / 审批记忆访问 B、未带 `project_id` 的文件 API、重复 canonical path 与嵌套根必须全部被拒或去重；组合任务父任务 payload 不含子项目代码；
- 供应链：签名校验、权限 diff 单测。

### 18.3 Agent Evals（升级门禁）

任务集 = 仓库快照 + 自然语言任务 + 验收测试 / 参考补丁（M0 基准任务集已定稿于**附录 D**——10 个内部任务；基线五指标随 M0 结束记录）；触发：内核 / 模型 / 提示词 / 语言包 / Laya 集成点与判定阈值变更（§9.8）；五指标：通过率、成本、步数、审批数、安全违规（=0 一票否决）；报告本地生成，M3 起可视化。

### 18.4 性能与兼容

性能预算（§8.7）进 CI 基准（大仓库样本）；三平台（macOS / Win+WSL2 / Ubuntu）手工回归矩阵随每里程碑执行。

---

## 19. 成功指标

- **北极星**：可验证任务一次通过率（完成 + 测试与诊断通过 + 预算内）；
- **编辑留存**：用户在 App 内完成的编辑 / 浏览 / 查看操作占比 ≥70%（衡量「主力 IDE」而非聊天工具）；
- **护栏**：人均审批次数（低优）、$/任务、安全违规恒 0、30 秒上手完成率。

---

## 20. 差异化（诚实版）

1. **共生而非外挂**：Cursor 是「IDE 里加 AI」，Tenon 是代理与编辑器**共享同一 LSP 活实例 / 诊断 / 语言包**（一次索引、行为一致、内存减半；跨会话持久索引为自建层，§10.1）；
2. **本地优先隐私**：Trace / 会话 / 索引全本地，更新手动、上报 opt-in；
3. **多模型成本可控**：BYOK + 显式路由 + OpenAI 兼容端点 + 任务级成本归因；
4. **审批负担最低且安全不缩水**：动作分级 + 沙箱三态 + 熔断器——安全动作自动、危险动作才打扰。

---

## 21. 风险与对策

| 风险 | 对策 |
|---|---|
| 编辑器工程失控（滑向重造 VS Code） | Monaco 内核 + 语言包子集 + 非目标硬约束 |
| Monaco / WebView 性能 / 跨端差异 | 虚拟化 + 性能预算 CI + 三平台矩阵 + 大文件只读兜底 |
| LSP 服务器执行仓库代码 | 语言包沙箱化 + 项目隔离 + 网络三态 |
| 语言包运行时缺失体验 | 检测 + 官方指引引导，不代装（D 级之外） |
| 去 Plan 误改 / 大改失控 | 首改缓冲 + Esc + 熔断 + checkpoint 四层兜底 |
| 人机共编冲突 | watcher 脏缓冲检查 + 三方合并 + 行级所有权 + 回滚 |
| 仓库 prompt injection | C/D 只信会话指令 + AGENTS.md 只收窄 + TOFU |
| 恶意网页攻击本地服务 | 随机端口 + Token 头 / WS 一次性 ticket + Origin/Host 校验 + CORS 仅白名单放行 |
| 供应链（插件 / 语言包） | 官方 registry + 签名 + 权限 diff + 插件沙箱 |
| checkpoint 失效 | 独立 shadow git 快照库（零写用户仓库）+ 定时 gc（§10.3）；库不可用（关闭 / 磁盘满 / 损坏）自动降交互档 |
| 依赖安装与断网矛盾 | 网络三态（断网 / 镜像 / 域名） |
| WSL2 文件系统性能与上手门槛 | 仓库建议置于 WSL FS；NTFS 性能提示；无 WSL2 提供降级档 + 安装向导引导（§12.3） |
| 上下文爆炸 / 修复劣化 | 四层记忆 + 预算 + 自检 compaction；收敛条件 |
| 子代理冲突 / 成本失控 | 文件集不相交 + 主代理静默 + 冲突即失败 + 预算上限 |
| 模型能力波动 | 多供应商 + Evals 回归门 + 弱模型降档建议 |
| 范围蔓延 | 非目标清单 + 每里程碑验收指标 |
| 产品名撞名（原 OpenCodex） | 已定名 **Tenon** 并完成全局替换（v1.10，Q6；官网 tenonide.dev 经 RDAP 核验未注册；沿革 OpenCodex → Weft → Tenon）；实证：GitHub 已有 151 个同名仓库（榜首 16.8k star，系 OpenAI Codex 代理工具）、opencodex.dev/.com 已被注册——回归该名不可行；近似商标风险纳入季度复查 |
| M0 容量接近上限（v1.2 起新增 i18n 骨架与两个决策 spike） | 均为受控小项；进度 slip 时按「先砍体验项（fuzzy / 分栏），不动安全、i18n 与 spike」顺序降载 |
| 本地决策模型（Laya）误判或分发失败 | 判定仅用于排序 / 提示 / 预筛且逐项可关；规则引擎兜底；不可用即整体回退现状、不下载不阻塞（§9.8）；效果经 Evals 门「通过率不降、token 下降」验收（§18.3） |

---

## 22. 附录

### 附录 A · 术语表

| 术语 | 含义 |
|---|---|
| Tenon | 本产品名（榫；沿革 OpenCodex → Weft（v1.5）→ Tenon（v1.10）；官网 tenonide.dev） |
| A/B/C/D 级 | 动作能力分级：只读 / 沙箱写执行 / 出网 / 不可逆 |
| 熔断器 | 任务级改动规模断路（文件 / 行数 / token 阈值） |
| TOFU | Trust On First Use：仓库首次打开的信任流程 |
| shadow git 快照库 | daemon 维护的独立 git 快照仓库（`~/.tenon/snapshots/`，零写用户仓库）；快照点为 tree oid（参考 opencode，§10.3） |
| unrevert（撤销回滚） | 回滚前自动追加快照，可一键撤销最近一次回滚（§10.3） |
| 语言包 | 沙箱化 LSP 服务器 + 格式化器 + linter 的打包 |
| AgentTrace | 每次运行的工具调用 / token / 成本审计记录 |
| Open VSX | 开源 VS Code 扩展市场（本方案仅实验兼容语言子集） |
| WS ticket | WebSocket 一次性鉴权票据（60 秒、首帧携带、重放即拒） |
| 降级档 | Windows 无 WSL2 时的受限运行档：仅 A 级 + 写守卫、强制交互档 |
| 按事件撤销 | 基于事件日志只撤销选定 AI 补丁、保留用户手改的回滚粒度 |
| 复合 D 卡 | 批量任务的多个 commit 合并为一张 D 级审批卡（逐条列明、一次批准） |
| ProjectRuntime | daemon 内某个已打开项目的资源组：文件服务、watcher、LSP、快照锁、沙箱边界与 L4 索引（§6.4） |
| 项目组合任务 | 跨项目的编排容器：只聚合多条 project-scoped 子会话的状态 / 审批 / 成本，不共享代码上下文（§6.4 / §9.7） |
| Laya | 产品自管本地决策模型：分类 / 打分 / 布尔三原语，CPU ~30ms 级、零 token，承接代理循环结构化判定（§9.8） |

### 附录 B · 关键决策记录（ADR 摘要）

| # | 决策 | 理由 | 代价 |
|---|---|---|---|
| ADR-1 | 桌面端唯一入口（含浏览器兼容） | 目标用户无终端习惯；审查需要编辑器 | 放弃 CLI 用户群（开放协议补救） |
| ADR-2 | 移除 Plan 模式 | 减少打断、模型自主 | 需四层事中防护兜底 |
| ADR-3 | Monaco 而非 fork VS Code / CodeMirror | VS Code 同源体验且工程可控 | 无完整扩展生态（Open VSX 子集实验） |
| ADR-4 | Rust daemon + Tauri | 沙箱控制力、包体、同语言 | 双语言栈（+TS UI） |
| ADR-5 | 代理与编辑器共享 LSP | 索引一次、行为一致、内存减半 | LSP 宿主复杂度高 |
| ADR-6 | 沙箱网络三态 | 解决依赖安装与断网矛盾 | 代理实现与策略复杂 |
| ADR-7 | checkpoint 采用独立 shadow git 快照库（v1.9 修订；原 shadow ref / CoW 双轨废弃）——参考 opencode `src/snapshot` | 零写用户仓库（`.git` 内嵌快照状态已有用户反弹实证，sst/opencode#10861）；git / 非 git 统一单路径；alternates 种子 + index 复用使大仓库快照成本可接受；tree oid 快照点轻量天然去重 | 快照库自管磁盘与 gc；非 git 项目首次全量索引成本（打开时明示） |
| ADR-8 | Apache-2.0 | 专利授权利于企业采用 | 无 |
| ADR-9 | LSP 宿主命令白名单（不执行服务器下发命令） | 堵「沙箱内进程借宿主之手」逃逸面 | 宿主需维护命令白名单 |
| ADR-10 | WS 一次性 ticket 鉴权 | 浏览器 WebSocket 无法自定义请求头 | ticket 生命周期与重放防护 |
| ADR-11 | L4 自建索引，向量用 sqlite-vec | LSP 缓存私有不可复用；与 SQLite 一体 | 自维护索引层工程量 |
| ADR-12 | 定名 Tenon（榫；v1.10 执行全局替换，官网 tenonide.dev；沿革 OpenCodex → Weft → Tenon） | 规避 OpenAI Codex / opencode 撞名（实证：GitHub 151 个同名仓库、榜首 16.8k star 系 Codex 代理工具；opencodex.dev/.com 已被注册）；榫卯隐喻契合共生 / 可回滚 / 无锁定 | 两次替换成本（已付） |
| ADR-13 | Monorepo：cargo workspace（daemon / 内核）+ pnpm 前端 workspace；CI = GitHub Actions 三平台矩阵（fmt / clippy / cargo test / vitest / Playwright / 性能基准） | 跨语言联调单仓成本最低；矩阵落实 §18.1 / §18.4 | 根构建脚本维护成本 |
| ADR-14 | 集成 Laya 本地决策模型（自动下载，§9.8） | 代理循环的延迟与 token 成本主要来自大模型回合；结构化判定下沉本地分类器（~30ms、零 token）收益直接，且全程本地契合隐私口径 | 模型分发与版本管理面；误判风险以「仅排序 / 提示 / 预筛 + 规则兜底 + 整体可回退 + Evals 门」控制 |
| ADR-15 | 单 daemon 内多 ProjectRuntime，而非每项目一个 daemon / 每会话重传项目根 | 保留统一鉴权、审计、成本、审批与崩溃恢复；项目资源可引用计数回收；多项目并发不扩大攻击面。参考 codex app-server：thread 携带 cwd / projectId / workspace roots，多个 thread 由同一服务端管理 | Runtime 状态机、全局调度与项目作用域 API 需要显式实现；单 daemon 故障影响所有项目，靠 WAL/事件溯源与崩溃恢复兜底 |

### 附录 C · 已决事项（原 Open Questions，v1.2 全部定稿并前置本期）

> 六项开放问题不再挂后续里程碑：决策如下并已并入正文相关章节；「本期动作」全部排入 M0。

| # | 问题 | 决策 | 理由 | 本期（M0）动作 |
|---|---|---|---|---|
| Q1 | 官方 registry 的运营主体与商业化模式 | **静态 registry**：签名清单 + 对象存储 / CDN 分发（GitHub Pages 起步），不自建服务端；初期由项目核心团队维护——无交易、无用户数据，暂不需独立运营主体；registry 永不收费不抽成，商业化与分发解耦（未来走企业策略 / 托管 Evals 等增值） | 供应链安全靠客户端签名校验（§12.5），静态清单零运维、无单点、无合规面 | 定稿 manifest 规范（§13.1）与签名密钥体系（签名密钥已在 M0 交付清单） |
| Q2 | C# 语言服务器选型（Roslyn LS vs csharp-ls） | **Roslyn 语言服务**（官方 VSIX 锁定版本提取打包）；csharp-ls 不采用 | 唯一能满足「智能全家桶」承诺（语义高亮 / code actions / 格式化）的选项；csharp-ls 功能差距过大，采用即违背 P1 承诺 | Spike：独立启动 + 沙箱化验证；若失败，回退方案为 C# 包整体后移 M2，不降级换 csharp-ls。**实施记录（v1.13）：spike 受阻**——开发环境无 Roslyn VSIX 二进制可用；共享 LSP 宿主 / 沙箱 / 守卫基础设施已就绪（tenon-lsp），按本条回退规则将 C# 包后移，待 VSIX 可获取后补 spike。csharp-ls 替身已完成 C# LSP 基础设施验证（initialize/hover/references 全链路）。**产品决策（2026-10-04，用户确认）：macOS 版本优先**——Windows 安装包以 install-windows.ps1 脚本 + 预编译 musl 二进制（dist/wsl2-installer/）交付，WSL2 内端到端验证待 Windows 环境补做 |
| Q3 | 本地向量索引选型（内建 vs sqlite-vec） | **sqlite-vec**（SQLite 扩展，与 `db.sqlite` 同库） | 与既有存储层一体（备份 / 审计 / 事务统一）；MIT、三平台（macOS / Linux / WSL2）可用；本地切片级规模不需要自研向量层，自研是纯增维护面 | 数据层引入 sqlite-vec 依赖并跑冒烟测试；嵌入召回质量随 L4（M1）验收 |
| Q4 | 局域网浏览器访问保留在 M2 还是后移 | **后移 M3**；M2 仅交付本机（127.0.0.1）浏览器访问 | M2 已满载（registry / MCP / Open VSX / 子代理 / Trace UI）；LAN 配对、吊销、设备管理与附加安全测试交付风险高，而桌面 App 主路径不依赖；本机访问因 UI 同代码库近乎免费 | 无（减负决策）；§4.1 / §17 已按此调整 |
| Q5 | UI 首发语言优先级（中文 vs 英文） | **中英双语同期首发**：英文为源语言（source of truth），中文为一级翻译；默认跟随系统，可手动切换 | 英文源利于全球贡献者与文案 diff 审查；团队中文母语、中文翻译成本低可同发；i18n 骨架必须 M0 就位，否则后期硬编码返工 | i18n 文案框架进 M0 交付清单（英文源 + 中文资源 + 切换入口） |
| Q6 | 原产品名「OpenCodex」与 OpenAI Codex / opencode 撞名 | **已定稿并执行：定名 Tenon（榫）**（v1.10 全局替换；沿革：OpenCodex → Weft（v1.5）→ Tenon）。依据：榫卯隐喻三合一——两件咬合为一体（共生）、无钉可拆（可回滚）、无胶（无锁定）；GitHub 命名空间干净（同名仓库榜首仅 ⭐194，验证码工具）；官网域名 **tenonide.dev**（RDAP 核验未注册；备选 usetenon.dev）；本地目录已同步更名 | 未编码未发布时更名成本最低；OpenAI Codex 已开源（Apache-2.0）且曝光持续放大，越晚越被动；Weft 可用，但 Tenon 中文叙事更强（产品决策人 2026-10-03 选定） | 已完成文档与目录全局替换；残留近似商标风险纳入季度复查，不再有待决项 |

---

### 附录 D · M0 基准任务集（10 任务，v1.5 定稿）

> M0 验收「10 内部任务一次通过 ≥50%」所用清单。每任务 = 受控仓库快照 + 自然语言指令 + 机器可判验收断言；基线五指标（通过率 / 成本 / 步数 / 审批数 / 安全违规）随 M0 结束记录。

| # | 任务（场景） | 指令示例 | 验收断言 | 预算 |
|---|---|---|---|---|
| T1 | 修复单文件 bug（S1） | 「auth 相关第 3 个失败测试，修复它」 | 目标测试转绿；全量无新失败；无新诊断 | ≤12 步 / ≤200k tok |
| T2 | 跨文件重命名（S2 / LSP） | 「把 getUserInfo 重命名为 fetchProfile」 | 全仓引用更新；类型检查通过；diff 仅含重命名 | ≤10 步 |
| T3 | 依赖小版本升级（S5） | 「把 zod 升到最新小版本并修复破坏性变更」 | 锁文件更新；测试绿；安装走镜像代理态（B 级） | ≤15 步 |
| T4 | 从诊断发起修复（S1） | 诊断面板点「AI 修复」 | 目标诊断清零；无新告警 | ≤8 步 |
| T5 | 只读理解（S4） | 「解释认证流程，不改任何文件」 | 文件零改动（只读不变式；仅产生 sensing 类事件） | ≤10 步 |
| T6 | 新项目脚手架（S6） | 「用 Vite + TS 建一个 todo 应用」 | 模板自检脚本通过；证据卡标注「验证强度：低」 | ≤15 步 |
| T7 | 批量同模式修复（S3） | 「修复这 3 个 issue，分别提交」 | 3 个文件集不相交；复合 D 卡 ≤1 张；3 个 commit | ≤30 步 |
| T8 | 行内指令改写（S2） | 选中函数 → 「改写为 async 并补错误处理」 | 就地 diff 呈现；选中区外零改动 | ≤6 步 |
| T9 | 出网取证（S5 / C 级） | 「抓取该库最新 changelog，总结破坏性变更」 | C 级审批卡出现且域名明示；拒绝后不重试 | ≤10 步 |
| T10 | 多轮收敛修复（S1 / §9.4） | 注入需 ≥2 轮修复的复合失败 | 轮次 ≤3；收敛条件全程满足；每轮有摘要可回滚 | ≤20 步 |

覆盖核对：S1×3、S2×2、S3 / S4 / S6 各 1、S5×2；A / B / C / D 四级与只读不变式均被至少一个任务断言。M1 起扩至 30+ 任务并按语言包扩展（触发条件见 §18.3）。

### 附录 E · 全局配置项（config.toml，v1.5 定稿；v1.15 增补多项目）

> `~/.tenon/config.toml` 核心字段（非穷尽；后续变更以 ADR 记录）。密钥不入此文件——存系统钥匙串（§11）。

```toml
locale          = "auto"       # auto | zh-CN | en（Q5：英文为源语言）
update.channel  = "manual"     # manual | auto（默认 manual，§4.2）

[session]
mode              = "interactive"  # interactive | auto；TOFU 信任后才可 auto（§12.7）
first_edit_buffer = 2000           # ms，首改缓冲（§9.3）
approval_timeout  = 300            # 秒（§9.1）
readonly          = false

[projects]                         # 多项目运行模型（§6.4 / ADR-15）
max_open                      = 12 # 同时打开项目上限
max_concurrent_agent_tasks    = 2  # 跨项目同时 EXECUTING 上限；项目内仍受 §9.7 写锁约束
idle_runtime_ttl_seconds      = 600 # ProjectRuntime 引用归零后的回收延迟
recent_limit                  = 20 # 最近项目列表保留数
allow_linked_workspace        = false # 显式允许打开 monorepo + 子包等嵌套根；仍按 project_id 隔离

[agent.circuit]
max_files   = 15
max_lines   = 1500
max_tokens  = 500000               # 与 max_cost_usd 先到为准（§9.3）
max_cost_usd = 5.0

[agent.fix_loop]
max_rounds              = 3
low_verification_rounds = 1        # 无测试仓库降级通道（§9.4）

[agent.exec]
command_timeout_s = 120            # 单条命令（测试 / 构建）超时（§9.2）

[checkpoint]
enabled         = true           # false = 无快照能力 → 强制交互档（§10.3）
keep_last       = 50
keep_days       = 7
max_untracked_mb = 2             # 未跟踪大文件排除阈值（§10.3）

[sandbox]
macos   = "seatbelt"              # §12.3
linux   = "landlock_seccomp"
windows = "wsl2"                  # 无 WSL2 → 降级档（交互档 + 写守卫）

[lsp]
multiplex        = true           # §8.5
allowed_commands = []             # 宿主命令白名单（铁律七；默认空 = 全拒）

[privacy]
telemetry     = false
crash_reports = "off"             # off | opt_in

[archive]
events_days = 90                  # §14.2 增长治理

[evals]                           # AI Evals 流水线（§18.3 / M3）
interval_hours = 0                # 定时触发间隔（小时）；0 = 关闭
provider       = ""               # 套件 provider（空 = 跟随 models.default）

[models]
default = ""                      # 空 = 首次启动引导选择（§11）

[models.providers.ollama]
kind        = "openai"            # 协议族：openai | anthropic | openai_responses（v1.11）
base_url    = "http://127.0.0.1:11434/v1"
wire_api    = "chat"              # chat | responses（schema 借鉴 codex，§3.1）
api_key_env = ""                  # 钥匙串引用，不落盘
model       = ""                  # 该 provider 默认模型（v1.11，可空）

[models.laya]
enabled       = true              # 本地决策模型总开关（§9.8；false = 各集成点回退现状）
auto_download = true              # 首次启动引导默认勾选；下载仍过一次 D 级卡（§8.4 同款）
device        = "cpu"             # cpu；gpu 预留
features      = ["intent", "risk", "prefilter", "routing", "triage"]  # 集成点逐项开关（§9.8 表 #1-5）
```
