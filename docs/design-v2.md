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

**3.2 策略总线（Policy Pipeline）**：一切工具调用（内置/MCP/hooks 外部命令）统一过策略管线：`分级判定 → execpolicy 命令评估 → hooks → 档位检查 → 单调拒绝（任一环节拒绝后继不可翻案，DSH ToolGuard 同款不变式）→ 执行+审计`。取代 v1「分级即策略」的单薄模型；`tenon-core::install_policy`（v2.0 已落）是其在工具层的第一个消费者。**v2.37 简化（ADR-19，design.md §12.2）**：档位检查段随档位系统退役删除，管线收敛为四段——`分级判定（read_only 拒写）→ 命令评估（execpolicy：Block→Deny 硬底线、Hold 恒放行；install_policy 白名单违例→Deny）→ hooks → 执行+审计`；`PolicyDecision` 收敛二态 `{Allow, Deny{stage, reason}}`，单调拒绝不变式保留（序贯短路结构属性）。

## 4. 安全模型 v2（重心）

**4.1 全对话完整权限（v2.37 用户裁定，ADR-19——v2.1 档位系统整体退役）**：所有会话（交互 / headless `--exec` / 子代理 / evals / ACP 轨）固定 **full_access × never** 姿态——零审批（Hold / 确认卡 / 三键决议 / `set_approval` / `set_exec_mode` / settings `security` 默认档 / 顶栏徽标 / 切换器全链移除），命令沙箱降级恒生效（run_tests / run_build / install_deps / run_command 无沙箱直跑）；execpolicy 仅 Block 硬拦（Hold 判定恒放行，内置高危档与自定义 `hold` 同轨）；只读开关独立保留（evals 只读先验轮 / 用户只读模式）。安全兜底 = execpolicy Block + install_policy 白名单 + SSRF 守卫 + 快照回滚 + 熔断 + 审计。以下档位二维正交设计与落地记录留档（v2.1-v2.36 历史规格，已退役）：

```
ExecMode（执行边界）：read-only │ workspace-write │ full-access
Approval（确认策略）：never │ on-irreversible │ on-elevated │ always
```

- **默认档 `workspace-write × on-irreversible`**：日常零打扰（B/C 照旧直执），唯 D 级首次确认、本会话/本项目可记忆放行；
- `never` = v1 零审批（降为可选档）；`always` = 新手/企业档；
- 档位输入区旁一键切换 + 顶栏常驻指示；团队策略只能收窄。

**落地状态（v2.1 已收口）**：全链落地——`tenon-core::gates`（Gate / ExecMode / ApprovalGear / ConfirmDecision 纯判定 + 单测矩阵）→ 会话闸门（D / Composite Hold：`confirm_request` 事件 + `AwaitingConfirm` 会话态 + oneshot 决议通道，Esc / Pause / Stop 打断按拒绝处理并回注控制通道，等待中 `set_approval` 即时改档重判）→ daemon（`POST /session/:id/confirm` 三键决议、control `set_approval` / `set_exec_mode`、settings `security.exec_mode / approval` 新会话默认档、GET /session 增 `pending_confirm` + `gate` 载荷）→ UI（输入区上方确认卡：级别徽标 + 工具 + 参数预览 + 本次允许 / 本会话允许 / 拒绝；会话态上下文条双切换器五语言；`awaiting_confirm` 入 RUNNING_STATES 发送转排队）。无人值守链路（headless `--exec` / 子代理 / evals）显式 `never` 防 Hold 永等；既有 D 级直执测试经 harness 默认 never 保持 v1 语义。顶栏常驻安全姿态指示随 P1 后续（统一沙箱总线）一并落。**（v2.37 退役标注，ADR-19：上述档位全链移除，见 §4.1 顶部与 design.md §12.2。）**

**4.2 execpolicy + run_command 正名**：新工具 `run_command`（B 级沙箱）承载诚实的通用命令通道，execpolicy 规则引擎做命令级风险评估（高危模式拦截或触发 Approval；规则库开放用户/团队自定义，Claude Code permissions 形态）；`run_tests`/`run_build` 收敛为语义工具（显式覆盖归入 run_command 语义审计）。

**落地状态（v2.2 已收口·首期）**：`run_command` 工具全链接线（Tool 枚举 B 级 / 模型目录 schema / prompt 描述 / executor 处理器：command 必填 + 可选 timeout_s + execpolicy 前置 + 断网沙箱含 full_access 降级）；`tenon-core::execpolicy` 首期规则库（破坏性 git / 系统级与家目录递归操作 / 管道注入解释器 / 提权 / 电源与格式化 / 设备覆写 / fork 炸弹，6 组单测矩阵）——规则刻意保守只拦「几乎不可能合法」形态；run_tests/run_build 任意 command 覆盖面在 §9.2 如实标注并由 run_command 分流承载。**余项（P1 后续）**：规则库用户/团队自定义（Claude Code permissions 形态）、execpolicy 高危档触发 Approval（当前只 Block）、Laya 评分可选后端、策略总线统一管线（当前 install_policy / execpolicy / gates 为分立消费者）。

**4.3 统一外部进程沙箱总线**：语言服务器、MCP server、市场插件、hooks 外部命令全部进同一沙箱包装（Seatbelt/Landlock+seccomp），声明式权限（fs/net/exec）——消灭「语言服务器有沙箱、MCP 裸奔」倒挂。

**落地状态（v2.4 首期：MCP + LSP 入总线）**：`tenon-sandbox::external`——`spawn_external` 统一入口：`ExternalSpec { write_roots, network }` 声明式权限（写根自动追加系统临时目录与平台缓存目录 ~/.npm、~/.cache、~/Library/Caches——node/npx 与工具链缓存所需，面扩大如实列出）；macOS 经 `seatbelt::external_profile`（deny default + 写限根 + 网络二态，路径 canonicalize 防 /private 符号链接失配）、Linux 经 pre_exec 复用 `LinuxSandbox`、其他平台降级直跑；profile 随机名 0600 + keepalive 句柄随 Connection 存活（Connection 结构新字段）；stderr 经 `TENON_EXTERNAL_STDERR=<路径>` 落盘排障（补 v1 stderr:null 吞语言服务器错误的可观测缺口）；`TENON_SANDBOX_EXTERNAL=0` 逃生舱。接线：McpConnection（网络按服务器 `permissions` 是否含 `net:*`——**offline MCP 进程网络收紧为真新增防线**）+ LSP ProcessConnection（文件系统隔离；网络首期 Open——cargo metadata 等工具链联网，逐语言包收紧）。hooks 维持 v1 §13.6 用户信任语义（不过沙箱，破坏用户 hooks 的风险大于首期收益，如实留档）。验证：external 单测（offline 拒网 + 写限根外拒/根内过 + stdio 管道帧协议）+ tenon-mcp fake server 沙箱下全过 + tenon-lsp semantic 7 测试（真 tsserver 沙箱下）两连全过；**测试串行信号量**——沙箱冷启动开销放大 tsserver 并发冷启动的时序脆弱（诊断等待窗撞线，正确性无损），语义测试串行化确定性通过。**余项**：LSP 网络逐语言包收紧、hooks 入总线、市场插件（即 MCP 通道已覆盖）。

**4.4 域过滤代理进程**：镜像/域名态的 registry 白名单由本地代理进程强制执行（现状 Seatbelt 放行全网，见 §0.1）；在此之前 install_deps 命令白名单（§4.5）先行收窄任意命令面。

**落地状态（v2.3 已收口·macOS）**：`tenon-sandbox::proxy` 本地 registry 白名单代理（127.0.0.1 随机端口、进程内常驻单例、CONNECT 隧道 + 绝对 URI 转发两形态、非白名单 403 带理由、TLS 不解密零证书面）；Seatbelt MirrorProxy 态收紧为「deny network\* + 仅放行 `localhost:<代理端口>`」（实测Seatbelt 网络地址仅支持 `localhost:*` / `*:port` 形态）；`exec_command` 为镜像态命令注入 HTTP(S)_PROXY / NO_PROXY（大小写双写）；install_deps 接单例代理、不可用 fail-closed 拒绝。白名单扩充至 install_policy 全部包管理器官方源 + git 依赖 / SPM 所需 GitHub 域（面扩大如实列出）。验证：proxy 单测 5 例（隧道放行/拒绝、绝对 URI 双路、相对 URI 拒、单例稳定）+ macOS 真沙箱 2 例（非白名单域双路皆拒、沙箱内代理可达且 403 应答）+ fail-closed profile 1 例。**余项**：Linux 镜像态回环收紧（随统一沙箱总线 §4.3）、白名单 settings 可配置（镜像源自定义）。**v2.37 标注（ADR-19）**：install_deps 命令沙箱随全对话完整权限降级为无，代理通路不再生效；白名单校验（§4.5）保留为唯一命令级收窄。

**4.5 install_deps 命令白名单（v2.0 已收口）**：`tenon-core::install_policy::validate_install_command`——仅接受已知包管理器二进制 + 安装语义子命令（npm/pnpm/yarn/bun/pip/uv/poetry/cargo/go/dotnet/…，拒绝 run/exec/test 类脚本执行子命令与 shell 控制符/命令替换/未知二进制）。残余风险（包 postinstall / 构建系统执行项目代码 + 网络开放）由 §4.4 代理进程收口前如实标注，不静默。

**4.6 安全姿态明示**：顶栏档位指示随 v2.37 档位退役移除（ADR-19）；Windows 无 WSL2 降级、沙箱初始化失败信号 daemon 尚无暴露端点，统一姿态条的全信号面列余项（如实标注）；快照库不可用走会话终态暂停 + `snapshot_unavailable` 错误事件（design.md §12.2）。

## 5. 上下文引擎 v2

① **摘要式压缩**取代纯省略（DSH 口径：持久事件 `surfaceOp: replace`、回放可确定性重现；先确定性剪枝陈旧工具输出、再摘要保留早期工作语义；省略式保留为摘要失败兜底）；② **阈值自适应** `max(24k, provider 上下文 × 40%)`，per-provider 可配；③ **上下文观测**：输入构成 breakdown（系统提示/L1/历史/L5/工具输出）进审查面运行仪表；④ **Run 级 resume** + 队列落库（§3.1）。

## 6. UX v2：审查面是第二主区

三区回归（v1.78 被放弃的 Codex 形态复活并升级）：左任务流（+全局活动视图入口）/ 中线程（v1.162 纯文本形态保留）/ 右**常驻审查面**——会话运行中自动停靠（可折叠记忆宽度，空闲可隐藏）：diff 流（实时）+ 计划/子任务 + 证据/产物 + 上下文/成本仪表（对齐 Codex summary pane）。

**落地状态（v2.5 首期：常驻审查面）**：`ReviewPane` 组件 + App 三区接线——运行态（RUNNING_STATES 含 awaiting_confirm）自动停靠右栏、空闲自动收起、✕ 手动关闭后本次会话不再自动弹（reviewManual 覆盖，会话切换重置回自动）；窄屏不渲染；复用 DiffPanel 虚拟滚动渲染 latestDiff（onLatestDiff 既有管道，零后端改动）；运行状态点 + 状态标签 + 六语言（review.*）。测试：组件 3 例 + App 接线 2 例（运行中停靠→关闭保持/空闲不渲染）。**余项**：计划/子任务与证据/产物卡、上下文/成本仪表、宽度拖拽记忆、diff 级反馈回路（选中行注入指令）、全局活动视图。

- **diff 级反馈回路**（v1 缺失）：审查面选中 diff 行 → 注入「针对这处改动」的反馈指令（复用行内指令链路）；
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
