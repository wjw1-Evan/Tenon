# Tenon v2 升级方案（v2.0 立项：监督工作台）

| | |
|---|---|
| 版本 | **v2.0** |
| 日期 | 2026-10-10 |
| 状态 | **已立项**（缺陷评审 → 升级方案 → 用户令「再检查一下，开始实施」；四项方向裁定采纳推荐档，见 §13） |
| 关系 | 本文件是 [design.md](./design.md)（v1 现行规格）的继任立项：v2 各阶段落地时逐节改写 design.md 并照常记 changelog；落地完成前 v1 章节继续作为现行规格，仅本文件明确收口项（如 §4.5 install_deps 命令白名单）即时生效 |

---

## 0. 动机与复查结论

对 v1（design.md，changelog 至 v1.211）做了一轮以 DSH Desktop（DeepSeek Harness 社区桌面壳，Tauri v2、万物皆插件）与 Codex app（2026：summary pane 跟踪计划/来源/产物、GitHub review 评论接入、并行任务监督、审批提示）为参照的设计缺陷评审，共 18 项缺陷；实施前对其中安全关键项做了代码级复查，全部坐实：

1. **镜像代理是空承诺**（复查坐实）：`tenon-sandbox/src/seatbelt.rs` 对 MirrorProxy/DomainProxy 态生成 `(allow network*)`——`NetworkState` 的 registry 白名单结构已备但被显式丢弃（`let _ = registries`），域过滤代理进程从未落地。install_deps 经 `sh -c` 执行**任意命令 + 网络全开**，SSRF 守卫只覆盖 http_fetch/web_search；
2. **零审批无中间档**（v1.89/ADR-17）：D 级（commit/push/PR/装插件）直执无最后一关，唯一硬边界只读开关藏在命令面板；对照 Codex approval 档位与 DSH fail-closed 审批，Tenon 处于最激进端且不可配置；
3. **审查动线与第一原则矛盾**：v1.110 编辑器退场为浮层后，「编辑器是审查界面本身」（§2.4 原则 2）与「人改与 AI 改实时互见」（§2.1 定位句）在默认形态下不成立；
4. **上下文策略保守**：24k token 即省略式压缩（无摘要、早期工具输出全丢）——128K 模型实际只见 ~24k 历史，长任务质量结构性受限；
5. **生态封闭**：子代理仅 in-process，ACP 被 v1.201 降为「可选演进」，hooks 不兼容 Claude Code hooks.json；
6. 其余：run_tests/run_build 任意 command 面未在工具表明示、崩溃不续跑、消息队列内存态丢失、Laya 投入产出未验证、GLM 特化按 URL 嗅探、MCP 无沙箱、Windows 降级档无明示、UI 形态六版本五推翻暴露无验证环、design.md 规格/历史混杂等（完整 18 项见评审记录，git log `v2.0`）。

**核心判断**：v1 的工程地基（Rust daemon 单服务端、project_id 作用域、shadow git 快照、共享 LSP、事件溯源）是对的；错的是十项产品层决策，共同病根是 **v1 把「自动」当卖点，而 2026 年所有竞品都在自动侧内卷，审查侧是空白**。

## 1. 定位重塑

> **v2 一句话**：开源本地 agent 监督工作台——代理自主干活，你在这里审阅一切、随时介入、随时回滚、随时换脑。

差异化从「比 Codex 更自动」翻转为「**比谁都好审 + 模型开放**」。Codex app 绑 OpenAI（即便开源），DSH 绑 DeepSeek 生态且产品化弱，Cursor 闭源——「开放模型 × 最强审查 × 本地治理」交叉点上无成熟产品。画像价值主张重排：小林=修完看得懂改了什么；阿哲=档位在我手里、每条命令可审；Maya=任务级证据链 + 全局活动总览。

## 2. 推翻 / 保留清单

**推翻（十项）**：①零审批→档位化（§4）；②编辑器浮层→常驻审查面（§6）；③省略式压缩+固定 24k→摘要式+自适应（§5）；④生态封闭→ACP 双向（§8）；⑤Laya 独立模块→execpolicy 可选后端（§7）；⑥GLM URL 嗅探→quirks 配置化（§7）；⑦MCP 无沙箱→统一外部进程沙箱总线（§4.4）；⑧run_tests 任意命令面→run_command 正名+execpolicy（§4.2）；⑨UI 无验证环→原型门禁+feature flag（§9）；⑩design.md 规格混杂→规格单一事实源（§9）。

**保留（v1 资产）**：Rust daemon + Tauri 2 + React/Monaco 技术栈；project_id 作用域 / ProjectRegistry / 多项目运行时；shadow git 快照 + 双粒度回滚 + unrevert（强于 opencode，v2 审查叙事的技术底座）；共享 LSP 宿主；事件溯源 SQLite / AgentTrace / 成本归因；SSRF 守卫 / 网络三态 / 沙箱三平台；任务流侧栏 / 发送队列 / 子任务进度 / worktree 并行 / 五语言 i18n 等 UX 细节。

## 3. 架构 v2

```
┌─────────── 生态层 ───────────┐
│ ACP 客户端（接入外部 agent）    │  ← 新增（P4）
│ ACP 服务端（harness 外供）     │  ← 新增（P4）
│ MCP（沙箱化）/ Skills / hooks  │
├─────────── 形态层 ───────────┤
│ Tauri 桌面（主）/ 浏览器 / headless │
├─────────── 内核层（harness）──┤
│ 执行/沙箱/快照/上下文/事件/成本    │
│ + 策略总线（新增，§4.1）          │
│ + Task/Run 会话模型（新增，§3.1） │
└──────────────────────────────┘
```

**3.1 Task / Run 分离**：v1 Session 同时承担「对话线程」与「一次执行」两身份，崩溃只能回滚不能续跑。v2 拆开：Task=持久对话与上下文载体；Run=一次执行尝试（可重试/恢复/分支）。崩溃恢复升级为 Run 级 resume；发送消息队列随 Task 落库（还「daemon 重启队列蒸发」债）。

**3.2 策略总线（Policy Pipeline）**：一切工具调用（内置/MCP/hooks 外部命令）统一过六段管线：`分级判定 → execpolicy 命令评估 → hooks → 档位检查（ExecMode×Approval）→ 单调拒绝（任一环节拒绝后继不可翻案，DSH ToolGuard 同款不变式）→ 执行+审计`。取代 v1「分级即策略」的单薄模型；`tenon-core::install_policy`（v2.0 已落）是其在工具层的第一个消费者。

## 4. 安全模型 v2（重心）

**4.1 档位二维正交**（推翻 ADR-17「唯一零审批档」）：

```
ExecMode（执行边界）：read-only │ workspace-write │ full-access
Approval（确认策略）：never │ on-irreversible │ on-elevated │ always
```

- **默认档 `workspace-write × on-irreversible`**：日常零打扰（B/C 照旧直执），唯 D 级首次确认、本会话/本项目可记忆放行；
- `never` = v1 零审批（降为可选档）；`always` = 新手/企业档；
- 档位输入区旁一键切换 + 顶栏常驻指示；团队策略只能收窄。

**4.2 execpolicy + run_command 正名**：新工具 `run_command`（B 级沙箱）承载诚实的通用命令通道，execpolicy 规则引擎做命令级风险评估（高危模式拦截或触发 Approval；规则库开放用户/团队自定义，Claude Code permissions 形态）；`run_tests`/`run_build` 收敛为语义工具（显式覆盖归入 run_command 语义审计）。

**4.3 统一外部进程沙箱总线**：语言服务器、MCP server、市场插件、hooks 外部命令全部进同一沙箱包装（Seatbelt/Landlock+seccomp），声明式权限（fs/net/exec）——消灭「语言服务器有沙箱、MCP 裸奔」倒挂。

**4.4 域过滤代理进程**：镜像/域名态的 registry 白名单由本地代理进程强制执行（现状 Seatbelt 放行全网，见 §0.1）；在此之前 install_deps 命令白名单（§4.5）先行收窄任意命令面。

**4.5 install_deps 命令白名单（v2.0 已收口）**：`tenon-core::install_policy::validate_install_command`——仅接受已知包管理器二进制 + 安装语义子命令（npm/pnpm/yarn/bun/pip/uv/poetry/cargo/go/dotnet/…，拒绝 run/exec/test 类脚本执行子命令与 shell 控制符/命令替换/未知二进制）。残余风险（包 postinstall / 构建系统执行项目代码 + 网络开放）由 §4.4 代理进程收口前如实标注，不静默。

**4.6 安全姿态明示**：顶栏档位指示；Windows 无 WSL2 降级、沙箱初始化失败、快照库不可用统一走「安全姿态条」，降级期间默认收紧一档。

## 5. 上下文引擎 v2

① **摘要式压缩**取代纯省略（DSH 口径：持久事件 `surfaceOp: replace`、回放可确定性重现；先确定性剪枝陈旧工具输出、再摘要保留早期工作语义；省略式保留为摘要失败兜底）；② **阈值自适应** `max(24k, provider 上下文 × 40%)`，per-provider 可配；③ **上下文观测**：输入构成 breakdown（系统提示/L1/历史/L5/工具输出）进审查面运行仪表；④ **Run 级 resume** + 队列落库（§3.1）。

## 6. UX v2：审查面是第二主区

三区回归（v1.78 被放弃的 Codex 形态复活并升级）：左任务流（+全局活动视图入口）/ 中线程（v1.162 纯文本形态保留）/ 右**常驻审查面**——会话运行中自动停靠（可折叠记忆宽度，空闲可隐藏）：diff 流（实时）+ 计划/子任务 + 证据/产物 + 上下文/成本仪表（对齐 Codex summary pane）。

- **diff 级反馈回路**（v1 缺失）：审查面选中 diff 行 → 注入「针对这处改动」的反馈指令（复用行内指令链路）；
- **档位切换器**：输入区旁 ExecMode×Approval 快捷切换（§4.1）；
- **全局活动视图**：命令面板 + 侧栏入口的跨项目任务总览（常驻形态尊重 v1.98 克制裁定）；
- 窄屏浮层 / 快捷键 / i18n 继承 v1 资产；
- **流程门禁**（治六版本五推翻）：布局级改动强制可点击原型/双形态截图先行；新形态一律 feature flag 上线可一键回退。

## 7. 模型层 v2

① **quirks 配置化**：thinking 控制/温度精度/限流退避/上下文长度移入 provider 配置声明（`[providers.x.quirks]`），删除 `base_url 含 bigmodel.cn` 嗅探；② **免费档预期管理**：完成页明示 1 并发、14-25s 首响，首任务前自动测速选档；③ **Laya 处置**：移出 M1 核心交付，降级为 execpolicy 可选评分后端（feature flag 默认关），以开启/关闭基准对比报告定去留；④ 多模态 / prompt caching / fallback 链 / 成本归因（v1.171-199 成熟资产）保留。

## 8. 生态 v2：开放双向

① **ACP 客户端（P4 先行）**：Claude Code / Gemini CLI 等经 ACP 接入 Tenon——外部 agent 跑任务，Tenon 供审查面/共享 LSP/快照回滚；子代理提供方注册表（in-process / ACP / MCP）自然消解 v1 瓶颈；② **ACP 服务端（P4 后段）**：harness（档位/快照/上下文引擎）反哺 Zed / JetBrains（ACP 已是 Zed+JetBrains 官方共建标准，JSON-RPC 2.0）；③ MCP 沙箱化（§4.3）+ 市场条目 ref 钉 commit SHA + 内容哈希入 sidecar；④ hooks 兼容 Claude Code `hooks.json` 导入；⑤ 官方签名 registry 维持未来通道定位，不再进正文承诺。

## 9. 工程与文档 v2

① **design.md 规格分离**：正文只留现行规格（版本注记全部移入 changelog/附录，单节 ≤300 字为目标），文档头版本号由脚本从 changelog 表尾生成（消灭撞号）；② 里程碑不 big-bang（§10），v1 数据全兼容；③ feature flag 基建（ui-prefs 级）随 P0 落地。

## 10. 排期与验收

| 阶段 | 周期 | 交付 | 验收 |
|---|---|---|---|
| **P0 立项与基建** | 1-2 周 | 本文件 + design.md/changelog 接线、feature flag 基建、install_deps 命令白名单（✅ v2.0 同笔收口） | 新旧规格并行可查；install_policy 单测+执行器拒绝路径绿 |
| **P1 安全 v2** | 3-4 周 | 档位系统、策略总线、execpolicy + run_command、域过滤代理进程（§4.4）、统一沙箱总线 | 安全测试套件全绿；默认档 D 级确认可记忆；镜像态域过滤真实生效 |
| **P2 审查面 v2** | 3-4 周 | 常驻审查面、diff 反馈回路、档位切换器、全局活动视图 | E2E：运行中审查面实时 diff + 选中反馈改码闭环 |
| **P3 上下文 v2** | 3-4 周 | 摘要式压缩、阈值自适应、Task/Run 分离 + resume、队列落库、上下文观测 | 附录 D 扩 30 任务（含长任务/多 Run）通过率不降；崩溃 resume E2E |
| **P4 生态 v2** | 4-6 周 | ACP 客户端、MCP 沙箱收尾、hooks 兼容、ACP 服务端 | Gemini CLI 经 ACP 在 Tenon 审查面跑通全链 |

每阶段跑附录 D 扩充基准作回归门（提示词/策略/压缩变更即触发）。

## 11. 边界（v2 非目标）

不做云执行/托管；不做移动端；不 fork 编辑器；**computer use 类能力（Codex 已做）明确列 v2 非目标**；不为 ACP 牺牲本地安全姿态（ACP agent 在 Tenon 内受同一策略总线约束）。

## 12. 风险与对策

| 风险 | 对策 |
|---|---|
| 档位打断 v1 老用户 | 默认档日常零打扰（仅 D 级首次确认）；v1 零审批可设为个人默认档 |
| 摘要式压缩失真 | 省略式兜底 + 压缩后确定性自检（v1.105 机制沿用）；摘要质量进 Evals |
| ACP 生态不确定 | 客户端先行（失败面小）；ACP 层防腐封装，核心不依赖 |
| 推翻幅度大、并行会话在途 | feature flag 灰度 + 每阶段独立可发布；v1 冻结为 legacy 章节不作废 |
| 审查面挤占窄屏 | 窄屏仍浮层化（v1.74 资产复用） |
| 范围蔓延 | §11 非目标 + 每阶段验收指标 |

## 13. 裁定记录（2026-10-10）

用户令「再检查一下，开始实施」——复查通过（§0 四项代码级坐实），四项方向裁定按推荐档生效：

1. 安全默认档：**`workspace-write × on-irreversible`**（D 级首次确认可记忆；v1 零审批降为可选档）；
2. 审查面形态：**常驻右栏自动停靠**（运行中），空闲可隐藏；
3. Laya：**降级为 execpolicy 可选后端**（默认关，基准对比定去留）；
4. ACP：**客户端先行**（P4 前段），服务端后段。
