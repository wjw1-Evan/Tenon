# Tenon 完整设计方案

| | |
|---|---|
| 版本 | **v1.91** |
| 日期 | 2026-10-03（v1.11/v1.12）· 2026-10-04（v1.13-v1.85）· 2026-10-05（v1.86-v1.91） |
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
| **v1.19** | **§8.1 / §8.5 实施同步：UI 订阅 active project 的 WS 文件事件；文件树按事件版本刷新；已打开且无未保存编辑的 tab 回读外部 / Agent 改动；removed 事件关闭 tab 并修正 active path；自动保存中的缓冲不回读，避免覆盖用户输入** |
| **v1.20** | **§8.1 实施同步：文件树从单层根列表升级为懒加载层级树；`GET /project/:id/tree?path=` 按目录返回并 canonicalize + 项目前缀校验；UI 目录节点懒展开、watcher 版本刷新已展开目录，测试覆盖嵌套加载与越界拒绝** |
| **v1.21** | **§8.1 实施同步：文件树新增新建文件 / 新建目录 / 重命名 / 删除操作，复用 project-scoped `/file/ops` 写守卫；创建与重命名使用可访问 modal，删除显式确认；App 同步 tab path、active path、unsaved 标记与 AI 行标记；测试覆盖创建与删除回调** |
| **v1.22** | **§7.4 实施同步：Cmd/Ctrl+P 从命令面板分离为 fuzzy file finder；daemon 新增 project-scoped `/files/fuzzy`（gitignore-aware 全树 + 本地打分 + 100 默认上限）；UI 120ms 防抖、键盘上下 / Enter、`:line` 解析与 Monaco `revealLineInCenter`；命令面板继续专注命令** |
| **v1.23** | **§8.1 实施同步：全局替换从预览闭环为可用操作——`POST /search/replace` 只应用选定文件，写守卫 canonicalize / 项目边界 / 大小限制，dirty buffer 显式跳过；先全量物化再写盘并返回逐文件 diff / 失败；SearchPanel 提供命中行跳转、按文件选择、替换预览与应用；daemon endpoint 文件支持测试隔离 + 20s 心跳，修复 sender wait_for 编译错误** |
| **v1.24** | **§7.2 / §7.5 实施同步：SQLite schema v3 新增 project_ui_state；`GET/PUT /project/:id/ui-state` 持久化项目布局、打开 tab、active path、bottom tab 与 session；App 激活项目时恢复并在 600ms 防抖后保存；v1→current 迁移测试更新；§14.2 数据模型同步** |
| **v1.25** | **§7.4 / §8.5 实施同步：fuzzy finder 支持 `@query` workspace symbols；通过 active file 选择语言服务，调用共享 LSP `workspace_symbol`；解析 LSP `file://` location 到项目相对 path + 1-based line，Enter 直接跳转；缺失 active file 时明确报错；中英文案与 UI 测试同步** |
| **v1.26** | **§7.2 / §15 设置面板落地（本期子集）：外观 / 语言 / 会话默认档 / 代理参数（首改缓冲、审批超时、命令超时）；`PUT /settings` 校验并持久化 `~/.tenon/settings.json`（0600，启动合并），新会话即时生效；`mode=auto` 仅对已信任项目生效（未信任回退交互档）；入口 Cmd/Ctrl+, + 命令面板** |
| **v1.27** | **§7.2 / §7.5 UI 布局重设计：左侧新增 activity rail（文件 / 搜索 / 语言包三视图单显，active 再点折叠侧栏，`tenon:sideView` localStorage 记忆）；侧栏不再纵向堆叠三组件；顶栏精简（副标题移除、模型路由紧凑化——模型下拉 + ✦ 路由建议气泡）；Monaco 深浅双主题与设计令牌同步（tenon-dark / tenon-light）；状态点运行态呼吸动画（data-state 驱动）** |
| **v1.28** | **§10.1 / §14.2 实施同步：L4 本地索引落地——`tenon-fs::l4` 本地确定性 embedding + symbol / line chunking；SQLite `replace_l4_file / delete_l4_file / clear_l4_project / l4_search / l4_chunk_count`；ProjectRuntime 激活触发全量 gitignore-aware scan；watcher 变更 500ms 去抖原子增量替换；`GET /project/:id/l4/search|stats`；集成测试覆盖初始索引、命中与增量更新** |
| **v1.29** | **§10.1 / §10.2 实施同步：L4 检索接入 Agent 上下文——SQLite schema v4 为 `l4_chunks` 补齐 `start_line / end_line` 并迁移旧数据；`AgentSession::l4_working_set` 按任务查询 top slices，12k token 预算裁剪为 `ContextSlice`；L1 工作集以“不可信仓库内容”边界注入 user message，Sensing Trace 仅记录 path / range / score，不含切片原文；core / agent 测试同步** |

| **v1.30** | **§6.2 桌面壳重设计：macOS Overlay 隐藏标题栏 + 集成式拖拽顶栏（红绿灯内边距 / 双击缩放，capabilities 授权 start-dragging / toggle-maximize）；窗口 1560×980（最小 1080×680）、底色 #0E1015 防白闪；品牌启动屏（握手轮询期渲染，失败态同卡片呈现错误与重启示）** |
| **v1.31** | **§8.1 / §12.6 实施同步：文件树支持 HTML5 拖拽移动——文件拖入目录 / 根目录，调用 `/file/ops move`，经写守卫 + watcher 同步，并联动 open tab / active path / unsaved / AI 标记；同时落地本机 CORS 预检（OPTIONS 白名单源直接 2xx，显式 Allow-Headers / Methods / Max-Age），修复 dev server 跨源 `Failed to fetch`** |
| **v1.32** | **§10.1 / §15 实施同步：L4 索引可观测与运维——daemon 保存 worker 状态 `queued / indexing / ready / failed`；`GET /project/:id/l4/stats` 返回状态、时间与错误；`POST /project/:id/l4/rebuild` 异步入队全量重建；诊断面板轮询切片数 / 状态 / 错误并提供手动 rebuild；测试覆盖 queued → ready 与 stats** |
| **v1.33** | **§6.4 / §7.3 多项目管理面实施同步：项目中心支持路径添加、按项目关闭 / 重开、移除登记（不删盘）与同时打开容量回显；`max_open` 只计已打开 runtime；打开前容量 / 嵌套校验失败不写入 ProjectRegistry；关闭保留登记 / 会话 / UI 状态，继续参考 Codex 的同一服务端多项目 / 多线程管理模型** |
| **v1.34** | **§18.3 质量门禁同步：Evals 固定夹具自动走生产同源 `scan_root → chunk → embed` L4 入库；任务可声明 `expected_l4_path`，新增 `L4RecallPath` 断言；用例结果记录召回数 / 均分 / 命中，套件聚合命中率与均分并写入 verdict；期望路径未命中使用例与套件失败；报告面板展示 L4 命中 / 均分；Trace 仍只消费 path / range / score，不扩散切片原文** |
| **v1.35** | **§10.1 / §15 实施同步：`set_l4_status` 进入进程内 broadcast，WS 沿用一次性 ticket 与 `project_id` 过滤推送 `l4_status`；诊断面板仅做初始 + 订阅后权威 `/l4/stats` 快照，状态变化事件触发增量刷新，移除 1.5s 固定轮询；WS 集成与面板事件测试覆盖** |
| **v1.36** | **客户端 E2E 修复（真实 daemon + 浏览器全功能回归）：§8.5 LSP 诊断空推送宽限（tsserver 先空后真双推竞态不再吞掉真实类型错误）；§7.3 TOFU 仅未信任项目询问 + 挂载自动打开守卫（消除信任确认循环与会话堆积）；§7.2 项目摘要改用刷新后列表（重载复用 ui-state sessionId，不再每次新建会话）；§4.2 LOCALE_CHANGE 订阅落地（顶栏 / 设置语言切换即时生效 + localStorage 记忆）；§10.3 时间轴补「撤销最近回滚」入口；§8.2 Monaco 本地打包（esm + workers，去 CDN 依赖，离线可用）并禁用 WebView 下渲染错位的 minimap** |
| **v1.37** | **§7.2 / §7.3 多项目任务中心实施同步：底部新增“项目任务”页，聚合所有已打开项目的活跃会话 / 待审批 / 脏缓冲 / 成本摘要；审批卡片按项目分组并可就地 once / session / deny；portfolio 子任务可跳回对应 `project_id + session_id`；关闭登记不进入任务中心，测试覆盖隔离与操作路由** |
| **v1.38** | **§8.1 / §8.5 / 附录 D T4 诊断闭环实施同步：新增活动文件 LSP 诊断面板，按 project + path 去抖调用共享 LSP `diagnostics`，文件 watcher / 项目切换 / 手动刷新驱动更新；面板归一化 severity / range / source / message，支持行号跳转和「AI 修复」；修复指令经当前 `project_id` 会话注入并要求目标诊断清零；组件测试覆盖 LSP 归一化、失败重试与任务注入** |
| **v1.39** | **§7.4 / §8.5 共生集成点补全（S2 / T8）：① 行内 AI 指令 Cmd/Ctrl+I——编辑器选区（无选区退化为整文件）自然语言改写，弹出卡展示文件 / 行区间 / 选区摘要，指令带「选中区外零改动、无新诊断」约束经当前 `project_id` 会话注入，就地 diff 由既有 AI 行角标呈现；② 跟随模式——代理写入已打开文件时自动 reveal 首个改动行（默认开、可一键关闭，偏好存 localStorage）；③ §7.4 快捷键补 Cmd/Ctrl+J 底部面板开合（终端区仍 M3 评估）** |
| **v1.40** | **§7.1 / §11 / §15 设置面板「模型」分区落地：默认 provider 选择 + provider 清单增改（协议族 kind / base_url / 默认模型 / 密钥环境变量引用 api_key_env），内置常用提供商预设（OpenAI / Anthropic / DeepSeek / Ollama / 智谱 GLM / 自定义 OpenAI 兼容）；Laya 状态卡（版本 / 已下载 / 设备，只读，消费 §15 `GET /models`）；`PUT /settings` 新增已知键 `models.default` / `models.providers`，校验后持久化 settings.json 并即时重建 provider 表——**新会话**按新默认 provider 构建（既有会话保持各自 provider）；密钥不变式不破例：设置链路永不接受 / 回显 `api_key` 明文，仅存 `api_key_env` 引用（§11 密钥存储）** |
| **v1.41** | **§18.1 端到端基建落地：引入 Playwright + 真 daemon harness——本地 OpenAI-compatible mock server、隔离 SQLite/config/快照、daemon 托管构建产物、临时 A/B 仓库；UI E2E 覆盖项目登记、全局任务中心、interactive B 级审批、apply_patch 落盘、checkpoint rollback / unrevert 与 B 项目磁盘零污染；`pnpm ui:test:e2e` 一键构建运行（CI 先安装 Chromium）** |
| **v1.42** | **§8.1 多标签分栏实施同步：编辑器工具栏新增 Split；右栏独立选择同项目已打开文件并独立编辑 / 自动保存，左栏保留 AI 行标、选区上下文与跳转；主编辑器切到原右栏路径时自动换右栏，关闭 / 重命名 / 删除同步清理；`splitPath` 进入 project_ui_state 跨启动恢复；组件测试覆盖打开 / 切换 / 关闭**；daemon 新增 `--settings` 覆盖文件路径，daemon 集成测试与 Playwright harness 的 settings.json 不再竞争全局状态** |
| **v1.43** | **§6.4 / §7.1 / §15 项目中心「添加项目」模态对话框落地：点击添加弹出模态（遮罩 + Esc / 取消关闭），桌面壳经 Tauri 原生目录对话框选择路径（浏览器模式手输绝对路径），项目名手输、缺省自动取路径末段；`display_name` 落库（SQLite schema v4→v5 补 §14.2 projects 表既定字段），`PUT /project` 与 `POST /projects/open` 接受可选 `display_name`（空串清除自定义名回退派生，重开可改名）；store / daemon / UI 测试覆盖** |
| **v1.44** | **§7.2 顶栏收敛：移除「打开路径」输入框 + Open 按钮（与侧边项目中心的登记 / 添加入口重复）；打开项目统一走项目中心——已登记项目点开、或经 v1.43 添加模态登记新项目；顶栏仅余品牌印记、spacer 拖拽区、模型路由、外观档、语言** |
| **v1.45** | **§8.1 / §8.2 大文件只读分块实施同步：`FileService::read_file_view` 按字节 offset / limit 读取、丢弃截断的 UTF-8 尾部并返回 total / next_offset / truncated / read_only；`GET /project/:id/file` 默认大文件首块 512KB（上限 2MB），UI 以只读 Monaco 打开并可加载下一块；写盘拒绝 >10MB 旧文件以防截断；fs / daemon / API 测试覆盖分块边界、拒绝写入与客户端参数** |
| **v1.46** | **§7.2 / §11 顶栏模型路由交互重做（参考主流 coding agent 的 model picker）：原生下拉替换为「当前模型按钮 → 模型选择对话框」——按钮显示当前 provider · 模型（无会话时显示默认 provider 并禁用切换），对话框列出 `GET /models` 全部 provider（名称 / 默认模型 / 本地 0 成本标记，当前项高亮 + ✓），点击即经 `POST /session/:id/model` 切换并关对话框（上下文随迁提示不变），Esc / 点击遮罩 / 关闭按钮退出；✦ 路由建议气泡保留并入对话框头部；切换语义不变：会话级热切换，全局默认仍在设置面板模型分区** |
| **v1.47** | **§8.3 / §8.5 编辑器共享 LSP 集成推进：Monaco 注册 completion / hover / definition / references / signature help provider，全部显式绑定 `project_id` 与活动 model 相对路径并复用 `/lsp` 共享宿主；新增 LSP 响应归一化（range、file URI → model path、completion、hover、location、signature）；UI 纯函数测试覆盖协议转换，为大文件 / 多栏 / 项目隔离下的智能提示建立基础** |
| **v1.48** | **§8.3 / §8.5 / §10.3 LSP 写操作闭环落地：daemon 归一化 WorkspaceEdit `changes / documentChanges`，UTF-16 列映射到 Unicode scalar byte offset，项目根外 / 越行 / 重叠拒绝；新增 `POST /project/:id/lsp/apply`——dirty buffer 显式 409，shadow git before / after snapshot，失败整体 restore，checkpoint 关联当前会话；Monaco 接入 format / rename / code action，写盘前 flush 未保存缓冲，应用后刷新 tabs / 大文件状态 / watcher；pure tests + 真 daemon 测试覆盖 UTF-16、原子应用、checkpoint 回滚与 dirty guard** |
| **v1.49** | **§8.2 / §16 tree-sitter 基础高亮管线接入 UI：新增 project-scoped `GET /project/:id/highlight`，daemon 限制 2MB、spawn_blocking 解析 Rust 语法并把 byte column 转换为 UTF-16 column；不支持语言显式 fallback；UI 以 250ms 防抖请求活动文件 token 流，watcher / 项目切换刷新，Monaco decorations 增加 function / type / keyword / comment / string / number 层级；daemon 集成测试覆盖 Rust tokens 与 txt fallback** |
| **v1.50** | **§8.1 源代码视图落地：新增只读 project-scoped `GET /project/:id/git/view`，聚合当前分支 / branches / porcelain changes / recent commits / active-file line-porcelain blame；路径经写守卫校验且 git 参数固定无用户拼接；底部新增 Source 页，changes 点击打开文件、blame 行可跳转，活动文件 / watcher 刷新；fs 纯测试覆盖 branches / changes / commits / blame / 逃逸路径，daemon 集成测试覆盖 repository / branch / changes / commits / blame** |
| **v1.51** | **§7.2 / §11 模型选择入口移入任务输入框（参考 ZCode 客户端输入区）：右区代理面板输入区改造为组合容器——textarea 在上，底行左下为当前模型 pill（→ 模型选择对话框）、右下发送按钮；顶栏不再放模型路由，仅余品牌印记、spacer 拖拽区、外观档、语言；v1.46 对话框交互原样保留（provider 清单 / 当前项高亮 / 会话级热切换 / Esc · 遮罩 · 关闭按钮 / ✦ 路由建议入对话框），切换提示 toast 不变；无活动会话时选项禁用、提示走设置面板；全局默认模型仍在设置面板模型分区（v1.40）** |
| **v1.52** | **§8.3 AI 行内补全（ghost text）落地：默认关闭，命令面板可开关（localStorage 记忆）；Monaco InlineCompletions Provider 输入停顿 350ms 后才请求；daemon 新增 project-scoped `/project/:id/inline-complete`，只绑定当前项目 active session、仅发送 24k char before/after 光标窗口、单轮 max_tokens=256、无工具目录；失败静默回退；用量写入 model_usage。Agent / daemon 测试覆盖单轮无工具、用量归因、项目隔离与跨项目拒绝** |
| **v1.53** | **§9.1 代理循环「截断续跑」：模型回合 `finish_reason=length`（输出被 max_tokens 截断）且无工具调用时，不再误判为「纯回答 → Done」——推送截断的 assistant 消息并注入续写指令（从中断处继续、勿重复已输出内容）进入下一回合；连续截断 ≥3 次按模型失败语义转 ERROR；决策回合 max_tokens 4096 → 16384（长推理 + 工具 JSON 不再截断）。动机：GLM 真实任务中长规划文本被 4096 截断，工具调用未生成即被判 Done，任务静默未做（changed_files 空）。agent 测试覆盖「首轮截断 → 续跑完成改动」与「连续截断 → ERROR」** |
| **v1.54** | **§9.6 / §11 / §14.2 / §7.3 模型流式输出落地：`ModelProvider::chat_stream` 成为任务回合首选；OpenAI Chat SSE 覆盖文本 / reasoning / tool-call 索引拼装 / usage，Anthropic Messages SSE 覆盖 text_delta / input_json_delta / message usage；流末尾必须携带权威 Final（tool calls + usage）；Agent 侧按 64 字符 / 120ms 合并为 `model_delta` 事件，Trace / WS / UI 单卡续写渲染，避免 token 级 SQLite 洪泛；非流式后端自动降级整响应模拟流。本地 fake-server SSE 测试覆盖 OpenAI / Anthropic 协议与 Unicode 分块，Agent / Vitest 覆盖无损聚合与 UI 呈现** |
| **v1.55** | **§7.1 / §7.2 左栏「项目」视图重排（参考 ZCode 客户端侧栏）：常驻项目平铺列表改为顶部「当前项目切换器」——行内显示当前项目名 + 运行中会话 / 脏缓冲徽标；点开下拉列出全部已登记项目（状态徽标、行内打开 / 关闭 / 移除操作 hover 显现、已打开容量回显），底部「添加项目」入口（v1.43 模态不变）；切换器下方主体只呈现当前项目，新增「对话 | 文件」视图 tab（localStorage `tenon:peTab` 记忆，默认对话）：对话页整栏滚动（会话 + 组合任务子项），文件页整栏文件树；移除对话列表 168px 高度上限与「项目 / 对话 / 文件」三段固定堆叠挤压。全局多项目监控仍由底部「项目任务」页承担（v1.37）** |
| **v1.56** | **§8.7 / §18.4 性能门禁与内核基准挂 CI：服务层 release 门禁覆盖「10MB+1B 文件只读首块 <2s」与「10 万文件搜索 max_hits=1 首结果 <500ms」，超阈值直接失败；新增沙箱 offline noop / file-read 基准并与 merge / snapshot 基准一起进入 performance job。普通测试保持 ignored，避免开发者本机噪声；WebView 帧率与三平台手工矩阵仍按 §18.4 执行** |
| **v1.57** | **§8.7 万行 diff 虚拟化落地：底部 DiffPanel 不再把完整统一 diff 渲染成单个巨型 `<pre>`——按固定 18px 行高建立总高度，只物化可视行 + 16 行 overscan，ResizeObserver 跟随底部面板高度，滚动经 rAF 合帧；新增渲染区间指示与 add/remove/hunk 行色。Vitest 断言 20k 行初始 DOM ≤80 行、滚动能无丢失到达尾部、解析耗时 <16ms；真实 WebView 帧率仍按 §18.4 手工矩阵复核** |
| **v1.58** | **§7.2 右区代理面板头部收敛：移除常驻「停止任务」按钮——按钮无论会话状态始终可点（误导性常驻），且 stop 为协作式暂停、仅在下一工具调用检查点（§9.1）生效，空闲误点的命令会残留控制队列、使后续任务首个工具调用即被暂停；停止能力保留经 Cmd/Ctrl+.（§7.4）与命令面板，交互语义不变** |
| **v1.59** | **§7.2 / §14.2 / §15 对话标题自动生成：会话首条用户消息提交后 daemon 后台经单轮无工具模型调用（max_tokens=48、temperature 0.2，系统提示带 `TENON_TASK_TITLE` 标记）生成 ≤16 字符短标题，落库 `sessions.title`（schema v5→v6）并经新事件 `session_title` 即时推送（既有 2s 项目轮询兜底）；对话列表（v1.55 对话页）标题优先显示，无标题回退模型名 / 短 id；生成失败或历史遗留无标题会话回退首条消息本地截断（不调模型），全链路不阻塞任务循环、不新增端点；MockProvider 与 E2E fake server 识别标记请求返回固定标题、不消耗脚本队列（脚本化测试的调用次数 / 序列断言零扰动）；标题调用用量照常入 model_usage** |
| **v1.60** | **§7.2 发送按钮状态化：代理推进态（sensing / deciding / executing / verifying / fixing）时右下发送按钮变为「停止」——点击即发 stop 控制命令（§9.1 下一工具调用检查点协作暂停），按钮转错误红填充区分；其余状态（idle / done / paused / error / awaiting_approval / rolled_back）显示「发送」。作为 v1.58 移除常驻停止按钮的收尾：停止入口仅在运行态出现、空闲不可点，封死陈旧 stop 命令残留队列的误触路径；stop 受理后按钮即禁用，防状态轮询间隙连点重复入队（状态离开推进态解锁）；Cmd/Ctrl+. 与命令面板入口语义不变** |
| **v1.61** | **§6.4 / §7.1 / §7.2 / §15 移除项目打开 / 关闭生命周期与底部「项目任务」页（对齐 Codex / ZCode 的极简多项目模型，登记即用）：ProjectRegistry 只管登记（添加 / 改名 / 移除），`POST /projects/:id/close` 路由删除，ProjectRuntime 降为 daemon 内部缓存——首次使用隐式激活，`max_open` 从「超限 409 拒绝」改为逐出无活跃会话的最久未用 runtime（无候选暂超限，空闲回收兜底）；UI 切换器下拉不再有打开 / 关闭按钮、已关闭徽标与容量回显，点击任意登记项目即切换（隐式激活）；底部「项目任务」页整体删除（撤销 v1.37 的页面，审批就地决策回到各项目代理面板，组合任务仍在所属项目对话列表呈现）；`GET /projects` 与 `POST /projects/open` 响应去 `open` 字段。daemon / UI 测试同步** |
| **v1.62** | **§7.2 / §7.4 底部面板可见开合：补齐 Cmd/Ctrl+J（v1.39）之外的鼠标入口——展开态 bottom-tabs 行右端新增收起按钮，点击收起整个底部 footer；收起态底部保留细条（当前 bottom tab 名 + 展开箭头），点击即按记忆高度恢复展开；可见性沿用项目 ui-state `timelineOpen` 持久化（§7.2 项目状态持久化），不新增端点；命令面板 `open.timeline` 改 `toggle.bottom` 切换语义（对齐 toggle.sidebar）；快捷键不变** |
| **v1.62** | **§7.1 / §7.2 活动栏底部新增「设置」齿轮按钮（用户反馈：设置仅有 Cmd/Ctrl+, 与命令面板入口、无可见入口，新用户无从发现）：置于 rail-spacer 之下常驻活动栏底部，点击即开设置面板，title / aria-label 沿用既有 `settings.open` 文案（中「打开设置」/英「Open settings」）；顶栏维持 v1.44 收敛后的极简不回潮；Cmd/Ctrl+, 与命令面板入口语义不变** |
| **v1.63** | **§7.1 / §7.2 左栏「项目」视图改文件夹形态（用户反馈：下拉切换器藏住项目全貌，参考 ZCode 客户端侧栏项目文件夹）：移除 v1.55 顶部切换器与下拉面板——「对话」页主体改为全部登记项目的文件夹树，每项目一行：折叠箭头 + 文件夹图标 + 项目名 + 运行中会话 / 脏缓冲徽标，行尾 hover 显现「移除」（存在持久会话仍禁用）；点击文件夹行即激活该项目（v1.60 隐式激活）并展开，已展开再点仅收起（激活不变）；展开后行下缩进内嵌该项目会话列表（v1.58 标题优先、同名附短 id 后缀）与组合任务子项，点击会话行切换会话；多项目可同时展开，active 项目默认展开，展开集合记忆于 localStorage `tenon:peExpanded`；「添加项目」自下拉底部移至列表底部常驻行（v1.43 模态不变）；「对话 | 文件」tab 与 `tenon:peTab` 记忆不变，文件页仍为 active 项目文件树；Esc 仅保留关闭添加模态。UI 测试同步（下拉相关用例改文件夹树语义）** |
| **v1.64** | **§7.2 编辑器窗格默认隐藏（用户反馈：空态中区仅显示「Tenon」占位，常驻挤压对话 / 代理区）：中区编辑器（多标签 / 分栏）仅当当前项目存在打开文件 tab 时渲染——默认空态与关闭最后一个 tab 后中区整体不渲染，右区分隔把手随之隐藏，代理面板转为弹性主区（flex:1、解除 720px 上限）直接与侧栏相邻；经文件树 / 模糊打开（Cmd+P）/ 搜索命中 / 符号与诊断跳转等任一入口打开文件即出现编辑器；ui-state 恢复的打开 tab 照常视为已打开文件（布局按项目记忆不变），关闭全部 tab 即再次隐藏；无新增端点与持久化键** |
| **v1.65** | **§18.1 开发模式定为「以 Web 版为主」（用户决策：vite 热重载反馈快、便于测试）：日常开发与调试一律走浏览器——`pnpm ui`（vite :5173 热重载）+ 本地 daemon，以 `http://localhost:5173/?port=&token=` 握手直连（与桌面壳同一 API/WS 链路）；`pnpm ui:build` 后 daemon 同源托管 `ui/dist` 仅作产物验证（无热重载）；桌面壳 `tenon-app` 仅用于壳链路验证（sidecar 握手、Tauri 原生目录对话框、WebView 导航）与发版前回归；README 开发指南与快速开始同步** |
| **v1.66** | **§8.7 / §18.4 项目切换性能门禁落地：release CI 对「runtime 已打开且有可复用会话」的核心切换链路（/projects → /ui-state → /tree）采样 9 次并断言 P50 <150ms；预热连接 / watcher / 目录读取，排除冷启动噪声；本地样例 P50=12ms。该门禁与大文件、搜索首结果、内核基准同入 performance job，WebView 实际交互帧率仍按手工矩阵复核** |
| **v1.67** | **§7.3 / §12.7 打开项目不再弹 TOFU 信任确认（用户反馈：每次询问打扰，要求直接信任）：UI 打开项目时对未信任项目静默 `PUT /project/trust` 置信任后再激活（建会话前生效，`mode=auto` 不被回退），`window.confirm` 信任卡移除；后端语义不变——信任只放宽 B 级档位、C/D 恒审批，未信任项目仍回退交互档（纵深防御：经 API 登记或本地吊销的场景不受影响）；`GET /projects/open` 响应 `trusted` 字段保留** |
| **v1.68** | **§6.2 / §18.1 开发热重载落地（承接 v1.65 Web 优先决策的握手痛点：daemon 随机端口 + 随机 token 使每次重启都令 `?port=&token=` 链接失效）：daemon 新增 `--port` / `--token` 开发专用固定绑定（默认随机路径与 §12.6 安全姿态不变）；`scripts/dev-daemon.sh`——固定 `127.0.0.1:9876` + token `dev`（对齐 UI `import.meta.env.DEV` 回落握手，浏览器裸开 `localhost:5173` 免参直连）、HOME 隔离 `.tenon-dev/`（不污染真实 daemon 数据 / endpoint 文件 / 单实例锁）、监听 `crates/**.rs` 自动 cargo build + 重启（握手不变，UI 免刷新重连，会话状态全在 daemon + 磁盘）；`scripts/dev.sh` 一键 = dev daemon 后台 + Vite HMR 前台；桌面壳 debug 构建启动时探测 dev server——5173 在线则 WebView 导航 dev server（HMR 直达桌面窗口，握手仍经 URL 参数直传），离线回落 daemon 同源托管 UI，release 构建行为不变；`pnpm dev` / `pnpm dev:daemon` 入口与 daemon 固定绑定守护测试** |
| **v1.69** | **§8.1 / §8.2 / §8.7 移除大文件只读分块（用户决策：任意大小文件都要可打开可编辑，性能由实现兜底）：`FileService` 取消 10MB 读帽与 >10MB 拒写，`read_file_view` 全量读取并删除 read_only / truncated / next_offset；`GET /project/:id/file` 去掉 offset / limit 参数并改 spawn_blocking（大文件 IO 不阻塞异步运行时）；LSP apply 的 10MB 守卫删除——全量读写闭环后不存在半文件覆盖；UI 移除只读横幅 / 加载下一块 / 只读 Monaco，Agent 侧 `read_file` 工具新增 10MB 上下文预算（LLM token 保护，与编辑器无关），`apply_patch` 改无上限读防截断；tree-sitter 高亮维持 2MB 帽（改按 total_bytes 判定）且 UI 对超帽文件跳过请求；性能预算改为「10MB 文件全量打开 < 2s」** |
| **v1.70** | **§7.1 移除「项目」视图「对话 \| 文件」tab（用户反馈：源码浏览不必独占整栏视图，看文件要来回拨 tab）：侧栏项目视图收敛为单一项目文件夹树整栏滚动；每项目文件夹行尾 hover 操作区（与「移除」并列）新增「源码」按钮，点击切换该行下内嵌的该项目文件树——project_id 作用域（§8.1），任意登记项目可看可操作、不限 active；再点收起；内嵌树限高内部滚动，文件树展开集合独立记忆于 localStorage `tenon:peFiles`（与 `tenon:peExpanded` 互不影响），watcher 刷新沿用 `refreshToken` 透传；`tenon:peTab` 记忆随 tab 移除废弃。UI 测试同步（tab 切换用例改源码切换语义）** |
| **v1.71** | **§9.8 Laya 真·自动下载并启用（用户决策：去掉引导与 D 级卡，装好即用）：daemon 启动即后台拉取官方静态 registry 签名清单，版本锁定 + ed25519 签名 + SHA-256 校验通过即下载安装热装载，清单版本新于已装自动升级、相同跳过，全程无审批卡——决策模型是产品自管、签名钉扎的静态资产（推理不出网），不是代理动作，§5 审批铁律不涉；失败静默回退（日志留痕、下次启动重试）；`auto_download = false` 或 `enabled = false` 不下载。`POST /models/laya/download` 去审批化：两阶段 D 卡流改直接下载安装，作手动重下 / 升级入口（同链路无卡）；`DaemonOptions` 新增 `laya_registry_url`（测试注入本地 registry）、`laya_public_key`（清单验签公钥 per-daemon 注入，不读进程 env——`TENON_LAYA_PUBLIC_KEY` 是 §12.5 插件验签链回退项，同进程并行测试下写 env 会串扰）与 `laya_models_dir`（测试注入临时模型目录，不触真实 `~/.tenon`），`in_memory()` 测试基座默认关自动下载（测试不出网）；§16 分发风险缓解、附录 E `[models.laya]` 注释、registry 模块文档同步** |
| **v1.72** | **§8.1 / §15 移除文件树「新建文件 / 新建目录」（用户决策：项目内创建文件 / 目录一律由会话大模型决策执行，人不手工建）：文件树收敛为重命名 / 删除 / 拖拽移动 + git 状态装饰；`/project/:id/file/ops` 与 `FileOp` 去掉 `create_file` / `create_dir`（API 收敛为重命名 / 移动 / 删除），`OpOutcome::Created` 随之移除；UI 工具栏只剩重命名 / 删除，prompt modal 仅剩 rename 模式，locales 删 `tree.new_file` / `tree.new_folder`；AI 经自身工具创建文件不受影响** |
| **v1.73** | **§8.7 / §18.1 WebView 冷启动门禁落地：AgentPanel 任务输入首帧后等待两帧才标记 ready，`performance.now()` 以导航时钟记录，样本仅存 localStorage 与 `window.__TENON_COLD_START__`（本机不外报）；Playwright 真 daemon E2E 用 5 个全新 BrowserContext 采样并断言 P50 <1.5s，进程回收移入 global teardown。Vitest 覆盖样本上限 / P50 / 两帧时序与 storage 禁用兜底** |
| **v1.74** | **§7.2 视口自适应三档布局（用户需求：小屏幕显示效果与多尺寸自适应）：`useViewport` hook（resize 监听）按视口宽分三档——wide ≥1180px 维持现状；middle 800–1179px 渲染期收敛，左栏 ≤24vw（下限 160px）、右栏 ≤34vw（下限 280px）、底栏 ≤40vh（下限 100px），clamp 只作用于渲染不改写用户记忆尺寸与项目 ui-state，回宽屏即复原；narrow <800px 浮层模式，工作区转 compact——侧栏与代理面板改互斥浮层（绝对定位覆盖主区 + 遮罩，宽 ≤82vw、下限 220px），进入 narrow 默认展开代理浮层（会话 / 审批是核心动线）；rail 点视图切侧栏浮层（同视图再点收起），顶栏新增代理浮层切换按钮（仅 narrow 渲染），遮罩点击收起；浮层开合不写持久化状态（`sidebarOpen` / ui-state 不被窄屏污染，回宽屏原样恢复）；narrow 下编辑器分屏渲染期禁用（单栏），bottom-tabs 横向滚动，设置面板 <640px 单列表单，顶栏下拉 ≤34vw；档位判定与 clamp 为纯函数（`lib/viewport.ts`），UI 测试覆盖档位判定 / clamp 边界 / 浮层互斥与遮罩收起** |
| **v1.75** | **§7.5 / §8.2 保存模式设置 + 编辑器撤销/重做（用户需求：保存方式可设、编辑可撤销重做）：设置面板新增「编辑器」分区——保存方式选自动保存（默认，停顿 1s 去抖写盘）或手动保存（仅显式保存写盘），偏好存 daemon ui_prefs（§7.5，键 `editor.saveMode`）即时生效；统一 saveNow 保存路径——Cmd/Ctrl+S 与 LSP 写盘前 flush 共用（有待写盘条目走 AutoSaver flush，否则未保存缓冲直接写盘 + clearBuffer），§8.6 脏缓冲推送、watcher 脏缓冲不回读与未保存圆点两模式语义不变；撤销/重做以 Monaco 原生栈为权威（Cmd/Ctrl+Z / Cmd/Ctrl+Shift+Z 聚焦生效），每 tab 独立栈、path 模型缓存跨 tab 切换保留，外部内容刷新（Agent 写盘 / watcher 回读 / LSP 应用回读）经 @monaco-editor/react 的 executeEdits + pushUndoStop 入栈——外部改动本身可撤销，关闭 tab 释放模型即丢弃历史；命令面板新增「编辑器：撤销 / 重做」全局入口（EditorPane 绑定 editor api 触发 active editor，不依赖焦点）；UI 测试覆盖绑定、门控与面板入口** |
| **v1.76** | **§6.2 / §18.1 Web 一键启动（参考 opencode `web`：单命令起服务 + URL 直出 + 自动开浏览器）：daemon 新增 `--web` 模式——UI 产物解析链升级为 `TENON_UI_DIST` → cwd 及其祖先目录的 `ui/dist` → 可执行文件同级的 `ui/dist`（普通模式语义不变：命中即同源托管、未命中静默跳过，桌面壳 sidecar 不受影响），`--web` 下解析失败即报错退出并给出修复指引（`pnpm ui:build` / `TENON_UI_DIST`）；启动后 stderr 直出 `Tenon Web → http://127.0.0.1:{port}/`（stdout 仍仅一行握手 JSON，sidecar 契约不变）并自动打开系统默认浏览器（macOS `open` / Linux `xdg-open` / Windows `cmd /c start`，失败仅告警不阻断 daemon）；`--web` 遇单实例锁被活实例持有时不走报错退出，改读 endpoint 文件复用（§6.2 20s 心跳，mtime 超过 45s 视为陈旧残留拒绝），直接打印该实例 URL 并打开浏览器后退出 0——重复执行等价于「打开」；endpoint 缺失或陈旧则报错。`pnpm web` = `pnpm ui:build` + daemon `--web` 一键入口；安全姿态不变（127.0.0.1 + 随机端口 + 随机 token + /pairing 同源握手；`--lan` 配对语义不变）；README 快速开始与解析链 / endpoint 复用测试同步** |
| **v1.77** | **§9.2 / §9.3 `git_push` 执行通道落地：D 级恒审批后可推送当前 / 指定分支到已配置远端，默认 origin，支持可选 upstream；remote 短名与分支短名走白名单（拒绝 option、refspec、force 语义、shell 元字符与控制字符），输出经密钥脱敏并设置 `GIT` 非交互执行面；本地 bare remote 集成测试覆盖成功推送，非法 refspec / option / 空白分支全部拒绝。`create_pr` 仍显式要求平台凭据 / CLI，不静默执行** |
| **v1.78** | **§7.2 复刻 Codex 客户端形态（用户决策：复刻 Codex；参照 Codex app 官方三区结构 projects sidebar / active thread / review pane）：① 代理对话（线程）恒为弹性主区（flex:1）——不再因打开文件被挤成 420px 右栏；② 编辑器窗格角色反转为右侧「审查窗格」：存在打开文件 tab 时停靠在线程右侧、有界宽度（沿用 rightWidth 记忆与 middle 档 clamp，260–720px），关闭全部 tab 即隐藏（v1.64 显隐语义保留、主次互换）；分隔把手移至线程与编辑器之间，拖拽改控编辑器宽度；diff 审查即编辑器窗格（AI 行角标 + 跟随模式不变）；③ 窄屏（<800px）编辑器窗格转互斥浮层（`floatPane` 增 `editor`，与侧栏浮层互斥、遮罩收起；线程在窄屏恒为在流主区，进入窄屏不再默认开代理浮层），顶栏浮层切换钮由「代理」改「编辑器」，窄屏打开文件（文件树 / 搜索 / 诊断跳转）自动唤出编辑器浮层；④ 底部面板默认收起（`timelineOpen` 初始 false，Codex 形态无常驻底栏），v1.61 细条 / Cmd+J / 命令面板入口与项目 ui-state 记忆语义不变。侧栏项目文件夹树（v1.63）、活动栏三视图、审批 / 会话动线、checkpoint 时间轴能力全部保留；UI 测试同步（responsive 窄屏语义、bottomPanel 默认收起）** |
| **v1.79** | **§9.2 / §12.2 `create_pr` C+D 复合审批与执行闭环落地：新增 `Composite` 审批级（Trace / 审批卡显示 `cd` / C+D，恒审批且只读拒绝），工具模型不再误标 D；`create_pr` 经本机 `gh` CLI argv 直执创建 PR——支持 title / body / base / head / draft / repository，无 shell 注入面、非交互与超时执行、输出统一脱敏；凭据仍由用户本机 `gh` 配置提供。Rust 测试覆盖 C+D 权限判定、PR 参数非法先检、argv 无 shell 执行** |
| **v1.80** | **§8.6 / §16 文件监听切换原生事件后端（目检实测缺陷修复：大仓库激活挂死）：`FileWatcher` 默认改用 notify `recommended_watcher`（macOS FSEvents 延迟 0 / Linux inotify / Windows ReadDirectoryChangesW），注册即返回；M0 的 PollWatcher + `compare_contents` 会在 `watch()` 同步全树扫描并对每个文件做内容哈希——`should_ignore` 只过滤事件、不过滤扫描，含 build 产物（target/ 等）的仓库激活即分钟级阻塞，`activate_project` 持锁期间 `/tree` 等状态请求全部挂起（本仓库真 daemon 复现：注册即卡死、sample 栈锁定 PollWatcher `scan_all_path_data`→`get_content_hash`）；原生注册在 setup 线程执行并设 10s ready 上限，超时显式降级无 watcher；PollWatcher 保留为原生后端构建失败 / 注册超时的回退（保守 2s 间隔 + 内容比对）与测试确定性通道（`watch_with_poll_interval` 维持 100ms 语义）；事件过滤（.git / target / node_modules / dist / .tenon）、去抖与 `next_batch` 接口不变；§16「搜索 / 监听」行同步** |
| **v1.81** | **§11 模型密钥操作系统凭据库接入落地：daemon provider 表从仅环境变量升级为 `ChainKeyStore`——`api_key_env` 指向的环境变量优先（开发 / CI 兼容），缺失时按平台读取 macOS Keychain / Linux libsecret / Windows PasswordVault；密钥不进入 daemon 进程环境，持久写入只到链尾 OS 凭据库；跨平台适配与链路优先级 / 持久写层测试覆盖** |
| **v1.82** | **§8.3 / §8.7 补全呈现性能门禁落地：Monaco completion provider 将 LSP 归一化 + Monaco suggestion 映射抽为纯函数，5,000 项 × 5 轮 P50 <80ms 进入 Vitest 门禁；请求返回后继续完成归一化与映射，处理耗时保留最近 20 个本地样本并暴露 `window.__TENON_COMPLETION_PERF__`（P50 / latest / samples，不上报）。该门禁覆盖 WebView 呈现前的数据准备热路径；真实语言服务器首补全仍按环境实测** |
| **v1.83** | **§7.1 / §4.2 隐私与更新设置面板落地：`/settings` 支持 `privacy.telemetry` / `privacy.crash_reports`（off | opt_in）与 `update.channel`（manual | auto），校验、0600 持久化并合并回显；设置面板新增隐私 / 更新分区，默认保持遥测关、崩溃报告关、更新手动。当前 v1.83 持久化用户偏好与合并视图，自动更新执行器仍按 M3「可选通道」另行评审** |
| **v1.84** | **§7.1 / §13.2 插件管理面板落地：设置面板新增插件分区，展示已装插件 / 权限；接入静态 registry 检索与两阶段 D 级安装——首次返回权限 diff，用户显式批准后携 approval_id 安装并刷新清单；新增前端 API 封装与组件测试覆盖已装清单、权限 diff、两阶段安装。后端签名 / SHA-256 / 保留字 / D 级审批语义不变** |
| **v1.85** | **§7.1 / §12.2 / §15 权限高级策略面板落地：设置面板新增「权限策略」分区，管理 TeamPolicy 的强制交互档、全局工具黑名单与单任务成本上限；`GET/PUT /team-policy` 原子校验并持久化 `~/.tenon/policy.toml`（0600），新建会话注入黑名单、force_interactive 收窄 B 级档位、成本上限取全局配置更严值。A/B/C/D 固定分级与 C/D 恒审批不提供放宽开关** |
| **v1.86** | **§4.2 / §6.2 / §15 自动更新执行器落地：ed25519 签名更新清单按平台锁定版本 / URL / SHA-256，下载写入 `~/.tenon/updates/staged/` 后原子 staging；daemon 启动绑定前将已验证产物原子替换当前可执行文件并继续启动。默认 manual；auto 仅按用户设置周期检查，缺公钥 / 坏签名 / 坏哈希 / 降级版本一律拒装；daemon 侧 staging + 壳/进程监督重启边界清晰，不做运行中原地热替换** |
| **v1.87** | **多项目并行操作与管理（参考 codex 多线程 / managed worktree / 侧栏线程流模型）：① §9.7 写锁改按 `(project_id, worktree_scope)`——项目主根互斥不变，会话级受管 worktree（`~/.tenon/worktrees/`，§9.5 同款内核托管）可与主根会话及彼此并行 EXECUTING，收尾为「合并」（先 checkpoint 项目根再三方合入，冲突走 §8.6 预览）或「丢弃」；② 侧栏「项目」视图顶部新增全局活动条：跨项目聚合运行中 / 待审批计数，点开全局会话列表（跨项目平铺、状态点、筛选、点击跳转、运行中就地停止），审批决策面仍唯一在各项目代理面板；③ §3.1 / §4.1 / §6.4 / §7.1-7.3 / §9.7 / §14.1 / §15 / §18 / 术语表 / ADR-16 / 附录 E 同步。**已落地**：daemon 写锁作用域化 + `POST /session` `worktree:"managed"` + `worktree/merge`（两阶段：任一冲突整体不写盘，脏缓冲显式跳过，合并前项目根 shadow 快照）/ `worktree/discard`（confirm 必填）；侧栏全局活动条（跨项目计数 + 平铺列表 + 筛选 + 就地停止）与受管会话合并 / 丢弃行内操作；Rust 369 + Vitest 141 + Playwright 多项目 E2E 全绿** |
| **v1.88** | **§7.1 / §7.2 左栏「项目」视图内容布局 Codex 化（用户决策：与 Codex projects sidebar 对齐）：移除仪表盘式顶部计数胶囊，改为项目索引结构——顶部紧凑搜索 + 添加入口，`Name / Updated` 列头统一密度；项目行采用两行内容（项目名 + 状态摘要 / 最近更新时间），展开后默认显示最近 10 条会话并显式展开全部；全局活动降为同构「All activity」列表组（筛选 chips / 跨项目平铺 / 跳转 / 停止能力不变），不再占据仪表盘首屏；文件夹展开、源码内嵌、隐式激活、受管 worktree 收尾、审批决策唯一在代理面板等语义不变；中英文案与 UI 测试同步** |
| **v1.89** | **移除审批功能（用户决策）：Agent 不再进入 `AWAITING_APPROVAL`，也不再有请求 / 决策 / 超时与 `POST /approval/:id`；A/B/C/D 只保留风险分级、沙箱与审计语义，非只读动作直接执行——C 级按 URL 自动放行域名，D 级直接落 Trace；安全边界改为只读开关、工具黑名单、沙箱、熔断器与对话节点快照。插件 / 语言包安装取消两阶段确认，一步执行；历史 `approvals` 表只作旧库兼容审计，不新增记录。每个 B 级写前仍建对话节点快照，Checkpoint 时间轴可回滚任一节点并可撤销最近回滚** |
| **v1.90** | **GitHub Release 发布与桌面壳自动更新：tag 推送触发三平台矩阵构建 daemon sidecar 与 Tauri 安装包，聚合签名 `latest.json` 到 GitHub Release；桌面壳在用户选择 auto 后用 Tauri Updater 从 `releases/latest/download/latest.json` 检查 / 下载 / 校验 / 安装完整包并重启，manual 默认不出网。v1.86 的 daemon-only 执行器继续服务无壳 Web / headless，桌面壳内禁用 auto 以避免双通道** |
| **v1.91** | **对话标题质量修复（对齐 Codex 短标题体验）：标题必须由模型基于首条用户请求生成；系统提示要求同语言、2-12 词、CJK ≤16 字符 / 拉丁 ≤32 字符、只输出任务目标标题。请求带 `reasoning_effort=low`（OpenAI 兼容端点映射 `reasoning_effort`；Anthropic 兼容端点关闭 thinking）并设 `max_tokens=128`。实测推理型 GLM 默认可消耗上百 reasoning token，48 会以 `length` 结束且正文为空，导致全量退回本地截断。模型失败仍回退本地截断，不阻塞任务循环** |







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

1. **安全与自动的矛盾**：等待确认打断心流，无边界放开又不可控；
2. **代理与 IDE 割裂**：代理自建一套代码理解，编辑器另一套 LSP，重复索引、行为不一致；
3. **成本不透明**：模型调用像黑盒，任务级花费无法归因；
4. **信任缺失**：改了什么、为什么改、能否回滚，证据链不完整；
5. **形态两难**：终端工具上手难，AI IDE 闭源且代理弱。

### 1.3 机会

一个**桌面原生、Agent 优先、开源本地、模型自由**的开发环境，让代理与编辑器**共享同一套语言智能**，用动作分级 + 沙箱 + 熔断器 + 对话节点回滚同时拿到「自动」与「安全」。

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
| S5 | 依赖升级 / API 迁移 | 会话 | C 级网络直执审计、镜像代理、验证闭环 |
| S6 | 新项目脚手架 | 会话 / 启动页 | 沙箱内创建、语言包推荐 |
| S7 | 团队治理与升级决策 | 设置 / Evals 报告 | 策略文件、Trace、Evals 报告 |

### 2.4 设计原则

1. **30 秒上手**：安装 → 贴 Key（或本地模型零 Key）→ 开聊；编辑器零配置可用，语言包按项目自动建议。
2. **编辑器可用是代理可信的前提**：用户必须能在 App 里看懂、审查、修正每一步改动——编辑器是审查界面本身。
3. **先自主、后审查 + 事中熔断**：无确认等待；安全靠只读开关 / 黑名单 + 动作分级 + 沙箱 + 熔断 + 回滚。
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
| 执行策略 | 沙箱模式（read-only / workspace-write / danger-full-access）与分级正交组合 | §12.2 只读开关与黑名单是硬边界 | 印证「边界 × 分级正交」设计；v1.89 不引入审批词汇表 |
| 命令风险判定 | `execpolicy`：规则引擎评估 shell 命令风险 | §9.2 工具分级 | M1 后评估作为命令级分类补充（不替代只读开关 / 工具黑名单） |
| 模型接入 | `model-provider` / `model-provider-info` / `ollama` / `config-schema`：config.toml `model_providers`（base_url + wire_api）接任意 OpenAI 兼容端点 | §11 通用 provider | config schema 直接参考 |
| 会话持久化 | `rollout` / `rollout-trace` / `thread-store`：JSONL 会话回放 + resume | §10.3 / §14.2 事件溯源 | 回放与 resume 语义参考 |
| 多项目 / 多线程 | app-server 的 thread 可携带 `cwd`、`projectId` 与 runtime workspace roots，thread 摘要持久化 cwd / git 信息；同一服务端可管理多个活跃 thread，多线程经 managed worktree 隔离并行 | §6.4 项目运行时 + §9.7 并发（v1.87 增会话级受管 worktree 并行） | 会话强绑定项目根与项目 ID；沙箱权限、事件与成本按项目归因；同项目并行以 worktree 隔离保证写边界互不重叠 |
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
| 6 | Gemini CLI | Apache-2.0 / TS | 直执审计 × 沙箱策略组合、遥测 opt-in 实践、ACP 接入实现 | §12.2 / §12.3、§4.2、§17 后续 |
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
| 项目与文件 | LSP 感知重命名/移动；任意大小文件全量打开编辑 | P1 |
| 编辑器 | Monaco 内核、tree-sitter 高亮、主题、虚拟化 | P0 |
| 编辑器 | LSP 补全/hover/定义/引用/重命名/诊断/code action/格式化/语义高亮 | P1 |
| 编辑器 | AI 行内补全（ghost text，默认关） | P2 |
| 语言包 | TS/JS、Python 内置；C#、Rust、Go 一键安装；运行时检测引导 | P1 |
| 语言包 | Open VSX 语言类扩展子集实验兼容 | P1 |
| 代理 | 自主决策循环（感知→判断→执行→验证→证据） | P0 |
| 代理 | 项目级写锁与跨项目并发调度；全局成本汇总 | P0/P1 |
| 代理 | 会话级受管 worktree 并行（同项目多任务同时执行，合并 / 丢弃收尾）；全局活动条跨项目监控与跳转 | P1（v1.87） |
| 代理 | 行内指令、诊断发起任务、只读开关、跟随模式 | P1 |
| 代理 | 并行子代理（不相交调度、预算上限） | P1 |
| 安全 | 动作四级分级、沙箱三态、直执审计、密钥拦截 | P0(P1 完整沙箱) |
| 安全 | checkpoint（独立 shadow git 快照库，§10.3）、回滚 / 撤销回滚、TOFU、崩溃恢复 | P0/P1 |
| 模型 | OpenAI / Anthropic / DeepSeek / Ollama + OpenAI 兼容端点；显式路由；成本显示 | P0/P1 |
| 模型 | 本地决策模型 Laya：自动下载 + 意图预判 / 命令风险辅助 / 上下文预筛 / 路由启发 / 批量 triage（§9.8） | P0（M1 后段核心）/ P1（深化） |
| 插件 | 外部进程插件 + MCP；官方 registry、签名、权限 diff | P1 |
| 治理 | AgentTrace、本地报告；团队策略文件、AI Evals | P1/P2 |
| 平台 | 浏览器访问（本机 127.0.0.1 为 P1；局域网配对后移 M3，Q4）；Windows WSL2 安装包 | P1/P2 |

### 4.2 非功能需求

| 类别 | 要求 |
|---|---|
| 性能 | 冷启动可输入 < 1.5s；LSP 索引就绪后首补全 < 400ms（冷启动到首个可用补全 < 3s）；10MB 文件打开 < 2s；搜索首结果 < 500ms；万行 diff 60fps（详见 §8.7） |
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
│  文件树 · 编辑器(Monaco) · 会话流 · Diff/证据 · 诊断            │
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

**GitHub 发布与桌面更新（v1.90，参考 Codex 桌面端后台更新体验）**：tag `v*` 推送触发 GitHub Actions 的 macOS arm64 / macOS x86_64 / Linux x86_64 / Windows x86_64 矩阵。每个构建先按目标三件套产出 release daemon + `ui/dist` + Tauri 包，Tauri Updater 私钥只在 Actions Secret 中出现，构建器生成安装包与其 minisign `.sig`；发布任务聚合为静态 `latest.json`（`version` / `notes` / `pub_date` / `platforms.<os-arch>.{url,signature}`），连同安装包和签名上传到当前 GitHub Release。客户端固定消费该仓库 `releases/latest/download/latest.json`：GitHub 的 HTTPS 保证传输完整性，Tauri minisign 公钥内置于桌面壳保证发布者真实性；任一签名失败即中止且不触达安装器。桌面壳每 5 分钟读 daemon `/settings` 生效通道，且每 6 小时最多检查一次；只有 `update.channel=auto` 才出网，manual 默认静默不出网。发现新版本后后台下载、校验、安装并经 Tauri 进程重启收尾。发布 tag 必须与 Cargo workspace / `tauri.conf.json` 语义化版本一致。v1.86 的 daemon-only 执行器继续服务无壳 Web / headless；桌面壳 spawn 时设置 `TENON_UPDATE_SURFACE=shell`，daemon auto 循环在该表面让位给完整包更新，避免同时下载 sidecar 与完整包。

**桌面窗体（v1.30）**：macOS `titleBarStyle=Overlay + hiddenTitle` 隐藏原生标题栏，红绿灯悬于 UI 顶栏之上（顶栏左内边距 78px，`is-tauri` 根类驱动，浏览器态自动豁免）；顶栏 / 品牌区 / 弹性区为 `data-tauri-drag-region` 拖拽区（capabilities 授予 `start-dragging` / `toggle-maximize`，双击顶栏 = 系统缩放）；窗口默认 1560×980、最小 1080×680、底色 `#0E1015` 与 §7.5 令牌一致（配置层 + HTML 双保险防首帧白闪）。启动体验：握手轮询期间即渲染品牌启动屏（渐变印记 + 脉冲连接指示），失败态同一卡片呈现错误与重启示；Windows/Linux 回退原生标题栏（macOS 优先决策不变）。

**开发热重载（v1.68，承接 §18.1 Web 优先）**：`pnpm dev` 一键拉起 dev daemon 与 Vite dev server。dev daemon 经 `--port 9876 --token dev` 固定握手（对齐 UI `import.meta.env.DEV` 回落约定，浏览器裸开 `http://localhost:5173` 免参直连；跨源请求走 §12.6 CORS 白名单 + 预检），HOME 隔离至 `.tenon-dev/`，`crates/**.rs` 变更自动重建重启——握手不变，UI 免刷新重连。桌面壳 debug 构建启动时探测 dev server：在线则 WebView 导航 dev server（HMR 直达桌面窗口，握手仍经 URL 参数直传），离线回落 daemon 同源托管 UI。`--port` / `--token` 仅为开发便利：默认随机端口 + 随机 token 的安全姿态不变。

**Web 一键启动（v1.76，参考 opencode web）**：`tenon-daemon --web` 单命令起 Web 版——UI 产物解析链（`TENON_UI_DIST` → cwd 及祖先目录 `ui/dist` → 可执行文件同级）命中即同源托管；`--web` 下解析失败报错退出并给出修复指引（普通模式维持「未命中静默跳过」，sidecar 无 UI 需求）。启动后 stderr 直出 `Tenon Web → http://127.0.0.1:{port}/` 并自动打开系统默认浏览器（失败仅告警，URL 已直出；stdout 握手 JSON 行不变，sidecar 契约不受影响）。单实例锁被活实例持有时不报错退出，改读 endpoint 文件（20s 心跳重写、mtime 超过 45s 视为陈旧残留）直接打开该实例的 Web 地址后退出——重复执行等价于「打开」。`pnpm web` = `pnpm ui:build` + `--web` 一键入口。安全姿态与普通模式完全一致（127.0.0.1 + 随机端口 + 随机 token + /pairing 同源握手）。

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
ProjectRegistry（登记 / 显示名 / 最近项；无打开状态）
  └─ ProjectRuntime: project_id → 规范化 root + 状态 + 引用计数
        ├─ FileService / Watcher / Git 状态
        ├─ LSP runtime（每项目 × 语言）
        ├─ SnapshotStore / ProjectWriteLock
        ├─ SandboxProfile（root × trust × worktree）
        └─ ContextIndex（L4，按 project_id 隔离）
GlobalScheduler（全局并发 / 成本 / 通知）
```

| 概念 | 规则 |
|---|---|
| Project | 稳定 `project_id`、显示名、canonical path、TOFU 信任、语言包与项目设置；登记即可用（v1.60：无打开 / 关闭状态） |
| ProjectRuntime | daemon 内部缓存，首次使用隐式懒加载；文件 / LSP / watcher / 快照等资源挂在其下，空闲回收（默认 10 分钟，可配置）；`max_open` 为内部 LRU 上限，超限逐出无活跃会话的最久未用 runtime，无用户可见开关（v1.60） |
| Session / Thread | 永远归属一个项目；可携带项目内 worktree 或子目录 cwd，但任何 B 级写边界仍由项目根与显式 worktree 白名单决定；可绑定会话级受管 worktree，与项目主根会话并行执行（v1.87，§9.7） |
| Active surface | UI 的每个编辑窗格都有明确 active `project_id`；项目切换、命令面板与发送任务都会携带该 ID |
| 跨项目任务 | v1 不允许一条代理会话直接读写多个项目。多项目编排只能由“项目组合任务”创建多条项目内子会话，父任务仅聚合状态 / 成本，不透传代码上下文 |

**打开与去重**：打开前 canonicalize + macOS/Linux 大小写校验 + Windows 前缀归一；同一 canonical path 复用既有 `project_id` 与 runtime。嵌套根默认拒绝（例如同时打开 monorepo 与其子包），除非用户确认并创建显式 linked workspace；linked workspace 也只是注册关系，不放宽任何沙箱路径。 嵌套校验必须发生在写入 registry 之前：被拒绝的路径不得留下登记记录；登记不受容量限制（`max_open` 只约束内部运行时缓存，v1.60）。

**资源与调度**：ProjectRuntime 为内部缓存——激活时若达 `max_open`（默认 12）则逐出无活跃代理会话的最久未用 runtime（无候选可逐出时允许暂超限，由空闲回收兜底）；全局同时 EXECUTING 上限默认 2，跨项目并行与同项目受管 worktree 并行共用该配额、每个项目仍遵守 §9.7 并行写锁（v1.87）。watcher / LSP / L4 索引按活跃度懒启动；低内存时自动关闭非活跃语言服务器与 watcher，保留会话与 checkpoint 可恢复（v1.60：无显式「暂停项目」，回收全自动）。

---

## 7. 桌面端 UI 设计

### 7.1 界面清单

| 界面 | 时机 | 要点 |
|---|---|---|
| 启动 / 项目选择 | 冷启动 | 最近项目、拖拽打开、新建；可打开多个；首次打开即自动信任（v1.67：不再弹 TOFU 信任卡） |
| **项目中心 / 项目文件夹树** | 常驻 | Codex projects sidebar 同构内容布局（v1.88）：顶部搜索 + 添加入口，`Name / Updated` 列头；已登记项目按文件夹行平铺，项目名 + 状态摘要 + 最近更新时间，展开默认内嵌最近 10 条项目会话；移除登记（不删盘）；添加项目走模态对话框（目录选择 + 项目名，v1.43）；无打开 / 关闭状态——登记即可用，点击行即切换并隐式激活（v1.60）；列表内「All activity」组（v1.87/v1.88）跨项目聚合运行中 / 完成计数，点开平铺会话监控与跳转 |
| **主工作区** | 常驻 | 四区布局（见 7.2） |
| 直执风险事件 | C/D 级动作 | 级别、动作详情、目标域名 / 参数、执行结果；不等待确认 |
| Checkpoint 时间轴 | 侧栏 | 事件流 + 快照点，任意回滚 / 撤销回滚（unrevert） |
| 语言包安装向导 | 检测到语言缺包 | 一键安装、运行时检测与官方指引 |
| 设置 | 全局（Cmd/Ctrl+, 或命令面板） | 外观档（§7.5）/ 语言（§4.2）/ 编辑器（保存方式 v1.75）/ 会话默认档 / 代理参数 / 模型（v1.40）/ **隐私与更新（v1.83）：遥测开关、崩溃报告 off | opt_in、更新通道 manual | auto**；**权限策略（v1.85）：强制交互、工具黑名单、单任务成本上限**；插件管理面已接入（v1.84） |
| 命令面板 | Cmd+Shift+P | 全部命令可达（无障碍要求） |
| Evals 报告 | M3 | 五指标 + 对比版本 |

### 7.2 主工作区布局

```text
┌──┬─────────┬───────────────────────────┬────────────────┐
│R │ 侧栏视图  │ 代理会话（线程主视图）         │ 编辑器（审查窗格，  │
│a │（单视图）  │ 恒为弹性主区（v1.78）        │ 默认隐藏，v1.78）  │
│i ├─────────┴───────────────────────────┴────────────────┤
│l │   诊断面板 · 测试证据 · checkpoint 时间轴（默认收起）      │
└──┴────────────────────────────────────────────────────────┘
```

**Activity rail（v1.27）**：最左侧 46px 图标栏承载侧栏三视图——项目 / 全局搜索 / 语言包向导（v1.87 校正：文件树自 v1.70 起内嵌于「项目」视图「源码」展开，不再是独立 rail 视图），**单视图显示**（不再纵向堆叠）；点击图标切换视图并展开侧栏，再次点击 active 视图折叠侧栏；当前视图记忆于 `localStorage("tenon:sideView")`。侧栏顶部显示当前视图名（小写字距标签）。顶栏保持单行精简：品牌印记、spacer 拖拽区、外观档、语言；模型路由已移入右区代理面板的任务输入框（v1.51：输入区为组合容器，底行左下当前模型 pill → 模型选择对话框，右下发送按钮；参考 ZCode 客户端输入区）；「打开路径」输入与 Open 按钮已移除（v1.44）——打开项目统一经项目中心（登记列表点开，或 v1.43 添加模态）。「项目」视图（v1.55 重排、v1.60 收敛、v1.63 文件夹化、v1.70 移除视图 tab，参考 ZCode 客户端侧栏）：主体为全部登记项目的文件夹树，单一树整栏滚动（v1.63 移除 v1.55 顶部切换器与下拉，v1.70 移除「对话 | 文件」tab，项目全貌常驻可见）：每项目一行——折叠箭头 + 文件夹图标 + 项目名 + 运行中会话 / 脏缓冲徽标，行尾 hover 显现「源码」与「移除」（v1.70：「源码」点击切换该行下内嵌的该项目文件树，缩进对齐会话列表、限高内部滚动，project_id 作用域任意登记项目可看、不限 active，展开集合记忆于 `localStorage("tenon:peFiles")`；「移除」存在持久会话禁用）；点击行即激活该项目（隐式激活，v1.60）并展开，已展开再点仅收起；展开后行下缩进内嵌该项目会话列表与组合任务子项，对话行优先显示自动生成的会话标题（v1.58，无标题回退模型名 / 短 id，同名多会话仍附短 id 后缀），点击会话行切换会话；多项目可同时展开，active 项目默认展开，展开集合记忆于 `localStorage("tenon:peExpanded")`；列表底部常驻「添加项目」行（v1.43 模态不变）。

**项目视图内容布局（v1.88，对齐 Codex projects sidebar）**：视图主体为紧凑项目索引——顶部搜索框按项目名 / 路径 / 会话标题即时过滤，右端「添加项目」常驻；其下以 `Name / Updated` 列头建立表格化节奏。项目行 = 项目标记 + 项目名 + 状态摘要（活跃 / 脏缓冲），右列显示该项目最近会话更新时间；行尾 hover 显现「源码」与「移除」（v1.70 语义不变）。展开后内嵌最近 10 条会话与组合任务子项，超过 10 条显式「显示全部 / 收起」；文件树内嵌、展开集合与隐式激活语义不变。**全局活动条（v1.87，v1.88 降为同构列表组）**：以「All activity」可折叠组置于项目列表内，不再渲染仪表盘式顶部胶囊；计数、跨项目平铺、筛选、跳转与就地停止能力不变，数据源与边界不变式不变。位于「项目」视图内（仅「项目」视图渲染，全局搜索 / 语言包视图不显示），一行聚合全部登记项目实时计数——`运行中 N · 已完成 K`（K 为最近未读完成的会话数）；数据源 = `GET /projects` 摘要 + WS 全项目订阅增量更新（既有链路，不新增端点）。点开展开全局会话列表：所有登记项目的会话**跨项目平铺**（无需逐项目展开文件夹树），按最近更新倒序，每行 = 状态点（颜色沿用 §7.5 状态色）+ 项目名 + 会话标题（v1.58 自动标题语义沿用）+ 相对时间；顶部筛选 chips：全部 / 运行中 / 已完成。行交互：点击 = 激活目标项目并切换到该会话（复用 §7.3 项目切换链路与项目 ui-state 恢复）；运行中行 hover 显现「停止」（确认后走 `/session/:id/control stop`，与 §7.4 Cmd/Ctrl+. 同语义）；**边界不变式**：全局活动条只做聚合监控 / 导航 / 就地停止；后台项目出现完成 / 失败时项目行徽标与全局组计数同步高亮；不做系统级推送通知，attention 一律以 UI 徽标与全局组为准。

三区均可全屏 / 折叠 / 左右互换；布局按项目记忆。**线程主视图 + 编辑器审查窗格（v1.78，复刻 Codex app 三区形态：projects sidebar / active thread / review pane）**：代理对话（线程）恒为弹性主区（flex:1），不再因打开文件被挤成固定右栏；编辑器窗格（多标签 / 分栏）仅在当前项目存在打开文件 tab 时停靠在线程**右侧**——有界宽度（沿用 rightWidth 记忆与 middle 档 clamp，260–720px），关闭全部 tab 即整体隐藏（v1.64 显隐语义保留、主次互换：此前编辑器占中、代理被挤右）；文件树 / 模糊打开（Cmd+P）/ 搜索命中 / 符号与诊断跳转任一入口打开文件即出现编辑器；ui-state 恢复的打开 tab 视为已打开文件，布局按项目记忆不变；diff 审查即编辑器窗格（AI 行角标 + 跟随模式，§8.5）。分隔把手位于线程与编辑器之间，拖拽控制编辑器宽度。项目切换器可以是顶栏下拉，也可以把另一个项目停靠为独立分栏 / 窗口；每个窗格维护独立 active `project_id`。底部不含「项目任务」页（v1.60 移除 v1.37 页面）：跨项目会话状态由各项目代理面板呈现（v1.87 全局活动条仅聚合监控与导航），跨项目并发 / 成本仍由 GlobalScheduler 统一调度。**底部面板默认收起（v1.78，Codex 形态无常驻底栏）**：`timelineOpen` 初始 false；开合有可见入口（v1.61）——展开态 tabs 行右端为收起按钮，收起后底部保留细条（显示当前 bottom tab 名），点击细条或 Cmd/Ctrl+J、命令面板恢复展开；项目 ui-state 记忆优先于新默认（§7.2 项目状态持久化）。

**视口自适应（v1.74）**：布局随视口宽度分三档自适应（`useViewport` resize 监听，档位判定与 clamp 为 `lib/viewport.ts` 纯函数）。四区结构在**宽屏（≥1180px）**不变；**中屏（800–1179px）**守护收敛——左 / 右 / 底栏尺寸在渲染期按视口比例 clamp（左栏 ≤24vw、下限 160px；右栏 ≤34vw、下限 280px；底栏 ≤40vh、下限 100px），clamp 只作用于渲染、不改写用户记忆尺寸与项目 ui-state，回宽屏即复原；**窄屏（<800px，半屏窗口 / 小屏设备）**浮层模式（v1.78 同步：线程恒为在流主区）——工作区转 compact，侧栏与编辑器审查窗格改为互斥浮层（绝对定位覆盖主区 + 遮罩，宽 ≤82vw、下限 220px），代理线程留在文档流占据主区（会话与回滚是核心动线，v1.74「进入窄屏默认展开代理浮层」随之取消），侧栏经 rail 唤出（点视图切浮层、同视图再点收起），编辑器浮层经顶栏切换按钮（仅窄屏渲染）或任意打开文件入口（文件树 / 模糊打开 / 搜索 / 诊断跳转）唤出，遮罩点击收起；浮层开合不写持久化状态——`sidebarOpen` 与项目 ui-state 不被窄屏污染，跨档位往返原样恢复；窄屏下编辑器分屏渲染期禁用（单栏），bottom-tabs 横向滚动，设置面板 <640px 单列表单，顶栏下拉 ≤34vw；命令面板 / 文件查找 / 模型选择等浮层按 min(内容宽, 94vw) 自适应。

**项目状态持久化**：每个项目独立保存左 / 右 / 底部尺寸、侧栏与底部可见性、bottom tab、打开 tab、active path 和可复用 session；激活项目时恢复，状态变更 600ms 防抖写入 daemon；缺失文件自动剔除，多项目状态互不污染（§6.4）。

### 7.3 关键交互流

**打开项目**：拖入仓库 → 识别语言 → 建议 language pack（缺则装）→ 自动信任（v1.67：打开即静默置信任，不再询问；建会话前生效，不阻断 `mode=auto`）→ 建 L4 索引（后台）→ 就绪。已有项目打开时复用原 `project_id`；再次打开 canonical path 只激活 runtime，不新建登记。

**切换 / 并行操作项目**：项目中心选择目标项目或把项目停靠为新窗格；所有文件 / 搜索 / LSP / 会话请求携带目标 `project_id`。用户可同时保留 A 的执行中任务并在 B 继续；风险动作在所属项目的代理面板审计（v1.89：无审批面板）。任意登记项目点击即切换并隐式激活，无打开 / 关闭步骤（v1.60）。 项目中心（v1.33）暴露路径添加与移除登记（不删盘）。 添加项目（v1.43）为模态对话框：桌面壳弹出系统目录选择器取得绝对路径（浏览器模式手输），项目名可手输、缺省自动取路径末段；登记 / 重开均携带可选 `display_name` 落库（空串回退派生），项目列表按自定义名展示。**同项目并行任务（v1.87）**：项目主根已有会话 EXECUTING 时，新任务可勾选「在独立 worktree 中运行」——daemon 为该会话创建受管 worktree 并行执行，完成后在会话列表就地「合并 / 丢弃」收尾（§9.7）；**跨项目监控（v1.87）**：全局活动条实时呈现各项目运行中状态，点击即跳转，后台任务无需逐项目展开检查。

**下达任务**：会话输入 / 行内指令 / 诊断「AI 修复」→ 进入自主循环（§9.1）→ 首改 2s 缓冲（Esc 可断）→ 改动实时高亮 → 证据卡片；C/D 直执动作生成 direct_action 审计事件。

**直执审计**：C / D / C+D 动作不再出卡等待；级别、具体动作、影响范围与执行结果写入 Trace。用户可在 Checkpoint 时间轴回滚相关节点；只读开关或工具黑名单是事前硬边界。

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

- **状态色**：感知（蓝）、执行（黄）、验证（紫）、风险直执（橙）、失败（红）、完成（绿）；
- **核心组件**：会话流卡片、证据卡片、直执风险卡、诊断条、AI 修改高亮区、时间轴节点、语言包安装卡；
- **AI 改动可视**：所有代理写入的行带「AI」角标，直到用户编辑该区域或确认；
- **外观档**：深色（默认）/ 浅色 / 跟随系统（`prefers-color-scheme`）三档；顶栏切换即时生效，`data-theme` 属性驱动 CSS 变量整套换色，状态色两套均可读；偏好双写——`localStorage` 为快路径，daemon `ui_prefs` 存储（§14.1）为跨启动 / 跨端（桌面 + 浏览器）权威（daemon 端口动态，localStorage 按 origin 隔离不可跨启动）；
- **Monaco 主题同步（v1.27）**：编辑器注册 `tenon-dark` / `tenon-light` 两套主题（背景 / 行高亮 / 行号 / 光标与 §7.5 令牌一致），`data-theme` 属性变化经 MutationObserver 联动切换；
- 全键盘可达；中英文案外置。
- **L4 索引诊断（v1.35）**：诊断区显示当前 project 的切片数、worker 状态、最近更新与错误；支持手动 rebuild；daemon 状态变化经 project-scoped WS 推送，面板以 `/l4/stats` 为权威快照并事件触发刷新（初始 / 订阅后 / 状态变化），重建 / 索引中禁用重复触发；
- **Fuzzy finder**：Cmd/Ctrl+P 打开 project-scoped 文件查找；120ms 防抖调用 `/files/fuzzy`（gitignore-aware + fuzzy score），方向键 / Enter 全键盘可达；`path:line` 解析行号并由 Monaco `revealLineInCenter` 定位；
- **Workspace symbols**：`@query` 切换符号模式；以 active file 选择共享 LSP provider，请求 `workspace_symbol`；LSP URI 归一为项目相对路径 + 1-based line 后直接跳转；无 active file 时不猜测 provider，明确提示；

---

## 8. 编辑器与文件管理

### 8.1 文件管理

- 文件树：以 active `project_id` 为唯一根；重命名 / 移动 / 删除 / 拖拽 + git 状态装饰；
- **项目内 CRUD**：文件树提供重命名 / 删除（v1.72 移除新建文件 / 新建目录——项目内创建一律由会话大模型决策执行，AI 经自身工具建文件不涉 `/file/ops`）；操作走 `/file/ops` 写守卫与 watcher 同步；重命名用可访问 modal，删除显式确认；打开的 tab、unsaved 标记与 AI 行标记随路径变化同步；
- **拖拽移动**：文件可拖入目录或根目录；禁止拖入自身 / 子树；HTML5 drag data 使用私有 MIME，drop 后仍走 `/file/ops move` 写守卫；成功后同步 open tab / active path / unsaved / AI 标记；
- **懒加载层级树**：目录节点展开时按 `path` 请求子层；daemon canonicalize + 项目前缀校验，watcher 事件版本刷新已展开层，避免大仓库首屏全量遍历；
- **watcher 驱动 UI 同步**：文件树订阅 active project 的 ProjectRuntime 文件事件并即时刷新；已打开且无未保存编辑的 tab 回读修改，removed 事件关闭 tab；自动保存中的缓冲不回读（§8.6）；
- **LSP 感知重命名 / 移动**：跨文件引用更新（B 级 + checkpoint 可回滚）；
- fuzzy 查找（Cmd+P 文件 / 符号 / 行号）；
全局搜索替换：核心服务 ripgrep 驱动，正则 / 过滤 / 多文件替换前 diff 预览；
- **安全多文件替换**：用户可选文件后应用；daemon 走写守卫与大小限制，先物化全部替换结果；dirty buffer 显式跳出（§8.6），不静默覆盖；响应包含逐文件 replacements、diff、失败 / skipped 原因，成功后触发 watcher 刷新；
- **诊断面板（v1.38）**：活动文件通过共享 LSP 实例查询 `diagnostics`，显示 severity / 位置 / source / message；watcher 保存事件、项目切换、活动 tab 与手动刷新都会重新拉取；支持按行跳转和「AI 修复」——指令注入当前项目会话并要求目标诊断清零、无新回归；
- **多标签分栏（v1.42）**：同一项目打开的多文件可拆为主 / 右两栏；右栏从打开 tab 中独立选择文件并支持编辑 / 自动保存；主栏继续承载 AI 行标、选区指令与行号跳转；`splitPath` 按项目持久化，关闭 / 重命名 / 删除会同步清理；
- **源代码视图（v1.50）**：底部 Source 页聚合当前分支、本地 branches、working changes 与 recent commits；活动文件显示 line-porcelain inline blame，可按行跳转；Git 查询只读、路径过写守卫、参数固定；
- 布局记忆；
- 同一窗口可停靠多个项目分栏；标签携带项目徽标，跨项目拖拽默认禁止；
- 任意大小文件全量打开并编辑（v1.69 取代 v1.45 只读分块）：读取无大小上限、全量返回，编辑 / 自动保存不再有大小区限；daemon 文件 IO 走 spawn_blocking，大文件不阻塞其它请求；tree-sitter 基础高亮维持 2MB 帽、超帽回退 Monaco 原生高亮（§8.7）。

### 8.2 编辑器内核

| 项 | 选择 | 说明 |
|---|---|---|
| 内核 | Monaco Editor | 与 VS Code 同源交互（补全 / hover / 多光标），不自建编辑器 |
| 高亮 | tree-sitter + LSP semantic tokens | 解析在 daemon 侧（Rust 增量解析，token 流推送 UI，WebView 不跑解析器）；未装语言包有基础高亮，装包后语义高亮 |
| 大文件 | Monaco 原生虚拟滚动全量加载，任意大小可编辑（v1.69 移除只读分块） | 性能预算见 §8.7 |
| 保存 | 模式可设（v1.75，ui_prefs `editor.saveMode`，设置面板「编辑器」分区即时生效）：自动保存（默认，编辑停顿 1s 去抖写盘）或手动保存（仅显式保存写盘：Cmd/Ctrl+S、LSP 应用前 flush） | 统一 saveNow：有待写盘条目走 AutoSaver flush，否则未保存缓冲直接写盘；保存成功即解除该文件脏缓冲（§8.6「未保存缓冲」语义不变：保存前的编辑窗口内代理写盘仍走三方合并）；tab 显示未保存圆点，保存后消失；写盘失败保留圆点待下次编辑或手动保存重试；两模式下 watcher 均不回读脏缓冲（§8.1） |
| 撤销 / 重做 | Monaco 原生撤销栈为权威（v1.75） | Cmd/Ctrl+Z / Cmd/Ctrl+Shift+Z（编辑器聚焦）；每 tab 独立栈，path 模型缓存跨 tab 切换保留；外部内容刷新（Agent 写盘 / watcher 回读 / LSP 应用回读）经 executeEdits + pushUndoStop 入栈——外部改动本身可撤销；关闭 tab 释放模型即丢弃历史；命令面板提供不依赖焦点的全局入口 |

**取舍**：Monaco ≠ VS Code，不引入 extension host——「够用且可控」优先于完整生态。

### 8.3 语言智能（智能提示全家桶）

补全（符号/成员/路径/片段/签名帮助）、hover 文档、定义跳转、引用/实现查找、工作区符号、安全重命名、语义高亮、实时诊断、code action 快速修复、**「AI 修复」**（与 code action 并列，直接发起代理任务）、格式化、折叠与代码镜头；AI 行内补全为实验特性默认关。 **v1.52 已接入**：默认关闭，命令面板开关；Monaco ghost text 经 project-scoped active-session provider，单轮无工具、前后窗口受限、用量归因。 **v1.46 已接入**：Monaco completion / hover / definition / references / signature help 均经共享 LSP 实例与显式 `project_id`；workspace symbols、诊断与「AI 修复」此前已接入。 **v1.48 已接入**：format / rename / code action 通过 daemon 原子 WorkspaceEdit 应用，未保存缓冲显式冲突，shadow checkpoint 支持回滚。 **v1.49 已接入**：Rust daemon tree-sitter 基础高亮 token 流与 Monaco decorations；多字节列已转换为 UTF-16。

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

规则：项目感知推荐（检测 `package.json` / `*.csproj` / `pyproject.toml`…）。**运行时分发策略**：安装包本体不内置 Node/.NET 等运行时（保包体），语言包首次安装时检测运行时，缺失提供两条路径——① 官方源一键下载锁定版运行时（D 级直执 + 签名校验，装入 `~/.tenon/runtimes/`，仅语言包沙箱可见、不污染系统）；② 官方指引自装（不代装原则保持）。语言包与运行时均版本锁定 + 签名校验；Open VSX 仅语言类子集实验兼容。

### 8.5 Agent ↔ 编辑器共生（差异化核心）

| 集成点 | 方向 | 说明 |
|---|---|---|
| 共享 LSP 多路复用 | 双向 | 一项目一 LSP 实例，编辑器与代理共用：索引一次、内存减半、行为一致 |
| 诊断流入代理 | 编辑器→代理 | 验证走测试 + LSP 双通道；诊断点「AI 修复」即发起任务 |
| 代理改动流入编辑器 | 代理→编辑器 | 实时写入缓冲并高亮「AI 修改」区；未打开文件在文件树标记 |
| 行内指令 | 用户→代理 | 选中代码自然语言改写，diff 就地呈现 |
| 跟随模式 | 代理→编辑器 | 自动滚动到代理正在修改处（可关） |
| 工作区重构原子应用 | 双向 | **v1.48 已落地**：LSP workspace edits 一次应用 + 单 checkpoint，失败整体 restore；dirty buffer 409 不覆盖 |

### 8.6 人机共编冲突处理

代理写盘前检查目标文件用户未保存缓冲 → 有脏缓冲则暂停并弹「你的改动 / 代理改动 / 合并结果」三栏；被 AI 修改的行带标记，用户编辑同区域自动解除；任何合并失误可回 checkpoint。

### 8.7 性能预算（验收级）

| 指标 | 预算 |
|---|---|
| 冷启动到可输入 | < 1.5s（P50） |
| 10MB 文件打开并编辑（全量读取到可输入） | < 2s |
| 全局搜索（10 万文件）首结果 | < 500ms |
| LSP 索引就绪后首个补全（中型 TS 项目） | < 400ms（冷启动到首个可用补全 < 3s） |
| 补全列表呈现 | < 80ms（P50） |
| 万行 diff 渲染 | 60fps |
| 项目切换到可交互 | < 150ms（P50；runtime 已打开；冷项目按打开流程另计） |
| 多项目稳态 | 12 个打开项目下 UI 主线程不因任一项目 watcher / LSP 输出阻塞；非活跃项目 LSP 可被资源控制器回收 |

**v1.56 CI 分层**：服务层 release 门禁显式断言大文件全量打开与 10 万文件首结果预算；`merge / snapshot / sandbox-exec` 基准输出趋势基线。**v1.57 起**万行 diff 使用固定行高虚拟窗口，20k 行 DOM 物化量有 UI 门禁；**v1.66 起**已打开项目切换核心链路有 daemon P50 门禁。**v1.82 起**补全归一化 / Monaco 映射热路径有 5k 项 P50 <80ms UI 门禁；冷启动、真实 LSP 首补全与实际帧率继续由手工矩阵 / 后续 UI 基准验收。

---

## 9. Agent 内核设计

### 9.1 自主决策状态机

```text
IDLE → SENSING → DECIDING ──无需改──→ ANSWERING → DONE
                      │ 需要改（输出意图卡）
                      ↓
              [首改缓冲 2s / Esc 可断]   ← 缓冲位于进入执行之前（§9.3）
                      ↓
                  EXECUTING（B 级沙箱；C/D 直接执行并审计）
                      ↓
                  VERIFYING ──失败──→ FIXING（≤3 轮，收敛）──→ 回 VERIFYING
                      │ 通过
                      ↓
                  SUMMARIZING（证据卡） → DONE

  侧向出口：任意状态 ─Esc / 熔断→ PAUSED ─继续→ 回断点状态；─中止→ ROLLED_BACK（最近 checkpoint）
            模型 / 供应商 / 网络失败 → ERROR ─重试 ≤2 / 切模型（上下文随迁，§11）/ 中止→ ROLLED_BACK
```

| 状态 | 说明 | 用户可见 |
|---|---|---|
| SENSING | A 级只读探索（文件 / 检索 / LSP 查询） | 蓝色「感知中」+ 活动列表 |
| DECIDING | 模型判断是否 / 如何修改；输出意图一句话 | 判断卡（改 X，因为 Y） |
| EXECUTING | B 级沙箱写 / 命令；C / D 直接执行并记录风险级别 | 黄色；编辑器实时高亮 |
| VERIFYING | 测试 + LSP 诊断双通道 | 紫色；证据流 |
| FIXING | 收敛条件下的修复循环（§9.4） | 轮次摘要 |
| PAUSED | Esc / 熔断器触发 | 停在最近 checkpoint |
| ERROR | 模型 / 供应商 / 网络失败 | 红色；重试 ≤2 / 切模型（上下文随迁）/ 中止 |

**关键转移规则**（图中省略的边）：

| 转移 | 条件与去向 |
|---|---|
| DECIDING → EXECUTING | 需要修改；**进入执行前执行首改缓冲（2s，Esc → PAUSED，§9.3）** |
| VERIFYING ⇄ FIXING | 失败且满足收敛条件（§9.4），轮次 ≤3；不满足即停 |
| 任意 → ERROR | 模型 / 供应商 / 网络失败（降级路径见 §11） |
| 任意 → PAUSED | Esc / 全局暂停 / 熔断器触发 |
| PAUSED → 断点状态 | 用户继续；从最近状态恢复，不重放已写入改动 |
| PAUSED / DONE → ROLLED_BACK | 用户中止，或时间轴回滚（两种粒度，§7.3） |

> DECIDING 之前存在可选的 Laya 本地意图预判（§9.8）：不新增状态、不改变上述转移规则；模型不可用时整体跳过，行为与本节状态机一致。

### 9.2 内置工具协议

| 工具 | 级 | 说明 |
|---|---|---|
| `read_file` / `list_dir` / `grep` | A | 只读；核心服务 rg；`read_file` 超 10MB 拒读（LLM 上下文预算，agent 侧承担，与编辑器无大小限制无关，v1.69） |
| `git_read`（status/log/diff） | A | 只读 git |
| `lsp_query`（定义/引用/符号/hover） | A | 共享 LSP 多路复用 |
| `apply_patch` | B | 结构化编辑（file + range + content），产生事件与 checkpoint |
| `run_tests` / `run_build` | B | 沙箱内，断网态；单命令超时默认 120s（附录 E） |
| `install_deps` | B | 沙箱内，镜像代理态 |
| `http_fetch` | C | 直接执行；目标域名写入审计事件 |
| `git_commit` / `git_push` | D | 直接执行；命令与结果全量入 Trace |
| `create_pr` | C+D | 直接执行；目标平台与 PR 元数据全量入 Trace；经本机 `gh` CLI 凭据执行（v1.79） |
| `plugin_*` | 按声明 | 外部进程插件提供，映射分级 |

### 9.3 事中防护

| 机制 | 默认 | 说明 |
|---|---|---|
| 首改缓冲 | 2s 可关 | 首个 B 级前高亮「即将修改 X」，Esc 打断；不是审批 |
| 全局暂停 | Esc / 托盘 | 立即暂停，停最近 checkpoint |
| 任务熔断 | >15 文件 / >1500 行 / 超预算 | 暂停待确认——断路器而非 Plan；预算默认 = 单任务 500k token 或 $5 等值（先到为准；本地模型仅计 token；config.toml 可调，全文见附录 E） |
| 只读开关 | 会话级 | 禁用 B 级，不依赖模型语义理解 |
| 模型能力适配 | 按模型 | 弱本地模型建议交互档 / 收紧熔断 |

### 9.4 验证与收敛

验证 = 测试（沙箱）+ LSP 诊断（双通道）。**无测试仓库降级通道**：以「沙箱构建 + LSP 诊断 + 代理读回 diff 自检」三项替代，证据卡明示「验证强度：低」，修复循环降为 ≤1 轮并建议补测试；新项目脚手架（S6）以模板自带自检脚本为准。修复循环任一不满足即停：失败数不增、diff 每轮 ≤ 上轮 1.5 倍、无新诊断 / 告警、轮次 ≤3；每轮输出摘要可回滚。

### 9.5 子代理编排

静态检查任务文件集不相交（相交拒绝 / 转串行）→ 每子代理独立 worktree + 沙箱（worktree 由内核托管，创建于 `~/.tenon/worktrees/`，对 `.git` 的元数据写入计入 **B 级项目内写**，不触碰用户工作区）→ 子代理运行期主代理只读 → 冲突 = 失败回传用户处置 → 并发 ≤3 × 单代理 token 预算硬上限。

### 9.6 提示组装与模型适配

**系统提示组成**：身份与目标 / 安全铁律（只读开关与工具黑名单不可放宽、输出证据契约）/ 项目规则 L3（AGENTS.md，只收窄）/ 会话记忆 L2 / 工具 schema / 输出契约（意图一句话 → 结构化动作 → 证据）。

**模型能力矩阵**：

| 供应商 | 工具调用 | 流式 | 备注 |
|---|---|---|---|
| OpenAI / Anthropic / DeepSeek | ✅ | ✅ | 原生适配 |
| OpenAI 兼容端点 | ✅（遵循规范） | ✅ | 通用 provider，覆盖大量供应商 |
| Ollama 本地 | ✅（带适配层） | ✅ | 零 Key 起步；弱模型自动建议降档 |

### 9.7 项目内多会话与跨项目并发

- **并行写锁（v1.87，参考 codex 多线程 managed worktree 隔离）**：写锁键为 `(project_id, worktree_scope)`——绑定项目主根的会话仍互斥（同一时刻仅一个 EXECUTING，其余会话限 SENSING / 只读，或排队等待写锁）；绑定不同受管 worktree 的会话可与主根会话及彼此并行 EXECUTING；写锁只约束代理会话——用户编辑不受限，与代理的并发冲突仍走 §8.6 脏缓冲协调；全局 `max_concurrent_agent_tasks`（默认 2）约束全部 EXECUTING 会话，跨项目并行与同项目 worktree 并行共用同一配额；
- **会话级受管 worktree（v1.87）**：创建会话时可选「独立 worktree 运行」——daemon 在 `~/.tenon/worktrees/<project_id>/<session_id>/` 创建内核托管 worktree（创建机制与 §9.5 子代理同款，对 `.git` 的元数据写入计 B 级项目内写）；写边界 / 沙箱 profile / 快照分片均按该 worktree 隔离（§12.3 / §10.3），L4 索引、脏缓冲与事件仍按 `project_id` 归属；任务完成后两条收尾路径——**合并**：先对项目根 checkpoint，再按会话改动文件集三方合入（冲突出 §8.6 合并预览，不静默覆盖；B 级、可回滚），**丢弃**：显式确认后删除受管 worktree 与其快照分片（不动用户根）；未收尾的 worktree 会话常驻会话列表并计入全局活动条，daemon 不自动合并 / 自动删除；
- **checkpoint 隔离**：快照库按项目 × worktree 隔离（`~/.tenon/snapshots/`，§10.3），不写用户仓库；快照点归属会话（checkpoints 表，§14.2），会话只能回滚自己链上的快照；
- **回滚冲突检测**：回滚前检查工作区是否含其他会话或用户的未合并改动，有则先出三方合并预览（§8.6），不静默覆盖；
- 共享 LSP 实例跨会话多路复用，请求按会话路由与限流（§8.5）。
- **跨项目并发**：全局调度器允许不同项目各自运行 EXECUTING 会话，但全局并发上限默认 2（可配置）——跨项目并行与同项目受管 worktree 并行共用（v1.87）；全局 token / 成本 / CPU / 磁盘预算先到即排队。每个事件、直执风险、diff 和成本都带 `project_id`，全局活动条与项目中心据此聚合；
- **跨项目隔离**：A 项目会话的 L1/L2/L4 上下文、沙箱 profile、脏缓冲与 LSP 请求不得进入 B 项目。若用户下达跨项目诉求，系统转「项目组合任务」创建多条项目内子会话；父任务只能携带用户目标与子任务摘要，不能把 A 的文件内容注入 B；
- **项目生命周期协调**：运行时回收（空闲 TTL / `max_open` LRU 逐出，v1.60）前先拒绝新任务，再暂停 / 排空 EXECUTING 会话并 flush 事件与 checkpoint；崩溃恢复按 `project_id` 分组，恢复一个项目不锁住其他项目。

### 9.8 本地决策模型（Laya）加速层

**定位**：产品自管的小型本地分类模型（Laya），只做结构化判定——选项分类（choice）/ 量表打分（score）/ 布尔判断（noul）三类原语；本地 CPU 推理（~30ms 级）、零 token 成本；**不生成代码、不做开放问答**。价值：把代理循环中不值得动用大模型的结构化判定下沉到本地——缩短回合延迟、压缩进入大模型的上下文、降低云端 token 开销。

**集成点**（全部为辅助判定，逐项可独立开关；序号供附录 E 引用）：

| # | 用途 | 说明 | 收益 |
|---|---|---|---|
| 1 | 意图预判 | DECIDING 前对用户消息分类（纯问答 / 需改动 / 只读分析 / 需出网），为大模型提供先验与路由建议；不改变 §9.1 状态机转移 | 减少无效轮次，辅助 #4 |
| 2 | 命令风险辅助 | §12.2 规则引擎为主、Laya 为规则库外命令补盲区（借鉴 codex execpolicy 思路，§3.1）；打分用于风险说明与执行前提示，**不改变 A/B/C/D 分级语义、不替代只读开关 / 工具黑名单**（v1.89 铁律不受影响） | 高风险早暴露，减少事后回滚 |
| 3 | 上下文预筛 | §10.1 L1 工作集：对 L4 召回的候选切片做相关性打分，仅 top-k 进入大模型上下文 | 直接降低每步 token 消耗 |
| 4 | 路由启发式 | 承接 §11「纯读任务提示轻模型」的轻量启发式（展示建议、一键采纳；auto 路由仍为实验特性默认关，§5 非目标 8） | 云端 token 成本下降 |
| 5 | 批量 triage | S3 批量任务的类别 / 优先级标注，辅助子代理拆分（文件集不相交仍由静态检查保证，§9.5） | 批量场景准备阶段提速 |

**分发与生命周期**：模型文件不进安装包（保包体，同 §8.4 运行时分发原则）；**daemon 启动即自动下载并启用（v1.71，用户决策）**——`models.laya.enabled` 且 `models.laya.auto_download`（默认开）时后台拉取官方静态 registry（附录 C Q1）签名清单，版本锁定 + ed25519 签名校验 + SHA-256 校验通过后下载安装至 `~/.tenon/models/laya/`（§14.1）并热装载，清单版本新于已装即自动升级、相同即跳过；**全程无确认卡**——决策模型是产品自管、版本锁定、签名钉扎的静态资产，经官方 registry 分发、推理不出网，不是代理动作；下载失败静默回退现状（日志留痕、下次启动重试，不做重试风暴），不阻塞任何功能。`auto_download = false` 时不自动下载，仅 `POST /models/laya/download` 手动触发（同链路、同样无卡）。由 daemon 内置 Rust 推理运行时加载（不额外进程、不进 WebView）；中英输入自动路由对应语言变体（模型随发双变体，与 Q5 中英同期一致）。

**边界与兜底**：

- 判定输出只用于**排序、提示、预筛**，绝不直接产生动作、绝不放宽只读开关或工具黑名单；
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

- **对话节点与独立 shadow git 快照库**：对话节点是事件日志中的一个可回溯步骤；B 级写 / 命令前创建节点，UI 用该节点呈现 Checkpoint 时间轴。daemon 为每个项目 × worktree 维护一个独立的 shadow git 仓库（`~/.tenon/snapshots/<project-id>/<worktree-hash>/`），与用户仓库完全隔离——不向 `.git` 写任何 refs / objects / index，不触发用户仓库的 hooks 与 gc。把快照状态内嵌用户 `.git` 已有用户反弹实证（sst/opencode#10861，早期 `.git/opencode` 方案），独立库是其修正后的形态；
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
- 事件溯源：工具调用 / diff / 风险级别 / **用户编辑**全量追加日志，只增不删（归档策略见 §14.2）。

### 10.4 Monorepo 与大仓库

L4 按包隔离、语言服务器按需启动；子代理限定单包；检索默认「当前包 + 显式依赖包」。
同时打开 monorepo 与子包属于 §6.4 显式 linked workspace：索引仍按各自 project_id 分片，跨项目检索必须显式选择目标项目；不自动合并两份索引。

---

## 11. 模型层

- **接入**：OpenAI / Anthropic / DeepSeek / Ollama 原生 + **OpenAI 兼容端点通用 provider**（base URL + Key）；设置面板模型分区（v1.40）提供常用提供商预设（OpenAI / Anthropic / DeepSeek / Ollama / 智谱 GLM——后两者经 OpenAI 兼容接入），新增 provider 只填名称 / 协议族 / base_url / 默认模型 / 密钥环境变量引用；
- **本地决策模型（Laya）**：产品自管小型分类模型，承接代理循环结构化判定（用途 / 分发 / 边界见 §9.8）；启动自动下载并启用（静态 registry + 签名 + 版本锁定，无确认卡，v1.71）、本地 CPU 推理零 token 成本；不可用即整体回退，不阻塞任何功能；
- **路由**：v1 显式（`/model` 与设置面板）+ 轻量启发式（纯读任务提示轻模型）；auto 路由实验特性默认关（置信度展示、一键改派、可反馈）；
- **成本**：云端按价格表；本地模型（含 Laya）显示「本地 · 0 成本」，token 单独统计；任务级 / 会话级 / 日级归因；
- **密钥存储**：`api_key_env` 名先查 daemon 环境变量，缺失时读取操作系统凭据库（macOS Keychain / Linux libsecret / Windows PasswordVault）；持久写入只进入 OS 凭据库，不落盘明文；
- **降级**：供应商不可用时可切换会话模型，任务上下文随迁。

---

## 12. 安全与权限模型

### 12.1 威胁模型

| 信任 | 对象 | 规则 |
|---|---|---|
| 可信 | 会话内用户直接输入；内核内置工具 | 正常执行 |
| 不可信 | 仓库内容（含 AGENTS.md / 注释 / issue）；插件；MCP 输出；**语言服务器加载的工作区配置与插件**；浏览器第三方网页 | 一律作为数据；网页请求本地服务一律拒绝 |

**铁律（v1.89 修订）**：只读开关与工具黑名单只信会话 / 团队策略，模型与仓库内容不可放宽；AGENTS.md 只收窄；插件签名 / 哈希 / 保留字校验不可跳过；MCP 仍按声明分级并全量审计；**语言服务器属 B 级沙箱进程**（tsserver 会加载工作区插件、Roslyn 求值可执行构建目标）；**LSP 宿主命令白名单**——宿主永不执行语言服务器下发的任意 `executeCommand`，服务器请求的 workspace edits / `showDocument` / 动态注册 capability 一律过 B 级写守卫与项目内路径检查，仅白名单能力放行（防「沙箱内进程借宿主之手逃逸」）。审批门禁已移除，直接执行的后果由 Trace 与对话节点回滚承接；不可逆外部副作用无回滚承诺，用户开启只读或黑名单才是硬拒绝。

### 12.2 动作能力分级

| 级 | 动作 | 策略 |
|---|---|---|
| A 只读 | 读 / grep / git 只读 / LSP 查询 | 自动 |
| B 沙箱写执行 | 项目内写、测试构建、LSP 工作区操作、镜像代理装依赖 | 沙箱内；写前建对话节点，默认可回滚 |
| C 出网 | 镜像代理外网络 | 直接执行；目标域名入事件 |
| D 不可逆 | 仓库外写、git config、commit/push、PR、装插件 | 直接执行；命令 / 结果入事件，无快照回滚承诺 |

A/B/C/D 仅是风险与执行边界标记，不再是审批门槛；去 Plan 安全兜底 = 只读开关 + 工具黑名单 + 沙箱 + 快照 + 熔断（§9.3）。

**权限策略（TeamPolicy，v1.85；v1.89 修订）**：全局约束只允许收窄——`denied_tools` 在所有会话的工具入口前拒绝；`max_cost_usd` 与全局配置取更小值。字段经 `/team-policy` 校验后原子持久化到 `~/.tenon/policy.toml`（0600），仅对新会话生效；既有会话不回写放宽或收窄策略，避免运行中边界漂移。`force_interactive` 仅保留旧配置兼容，语义为无操作；没有 A/B 升级或快照关闭开关。

**更新执行器（v1.86）**：更新清单必须包含平台 triple、大于当前版本、artifact SHA-256 与对该哈希的 ed25519 签名；配置必须钉扎公钥，无公钥直接 fail closed。下载限长、限时，先落同目录 `.tmp`，SHA-256 通过且权限收敛后才原子改名 staging；daemon 启动绑定端口前用 staged 产物原子替换当前可执行文件，替换失败保留旧版并记录状态。默认 manual 不出网；`auto` 只做周期检查与 staging，不运行不可信安装脚本，也没有运行中原地热替换——可用更新在 daemon/壳重启时生效。

### 12.3 沙箱与网络三态

| 平台 | 机制 | v1 |
|---|---|---|
| macOS | Seatbelt | 原生（M1 完整版，M0 写守卫） |
| Linux | namespace + seccomp | 同上 |
| Windows | daemon 于 WSL2，复用 Linux 沙箱；检测不到 WSL2 时仍启用写守卫与项目边界，完整沙箱经 WSL2 补齐 | 完整沙箱经 WSL2 |

网络三态：**断网**（测试/构建/纯分析）→ **镜像代理**（仅预授权 registry：npm/pypi/nuget/crates…，B 级）→ **域名代理**（请求 URL 的 host 直接放行并入事件，C 级）。系统只读、写限当前会话绑定的项目根 / 显式 worktree、每项目语言服务器隔离。多项目打开时为每个执行进程分别物化 project roots；跨项目路径既不是可写根，也不进入 A 级检索范围。

### 12.4 密钥处理

`.env` 与常见 Key 模式检测 → 默认拦截不进上下文 → 需要时用户显式注入「脱敏引用」（`env:STRIPE_KEY`）；**拦截同样覆盖 `git_read` 输出**（log / diff / show——历史中曾提交的密钥同样脱敏）；不承诺识别一切形态；Trace 标注拦截 / 放行。

### 12.5 插件与语言包供应链

仅官方 registry、签名 + 版本锁定、安装权限 diff 写入 Trace、插件最小权限自有沙箱、调用全量入 Trace、保留字防 typosquatting。安装不再等待批准；校验失败或保留字命中仍拒绝。

### 12.6 本地服务与浏览器访问

默认仅绑 127.0.0.1 + 随机端口；HTTP 用 `X-Tenon-Token` 头；**WS 用一次性 ticket**（浏览器 WebSocket 无法自定义请求头：`POST /ws-ticket` 换 60 秒一次性票据，连接首帧携带，重放即拒）；校验 Origin/Host；**CORS 仅白名单放行应用自身 origin（Tauri WebView 源）与已配对设备，其余拒绝**；白名单源跨源 dev server 的 `OPTIONS` 预检由本地服务直接 2xx，并显式 `Allow-Headers` / `Allow-Methods` / `Max-Age`；局域网显式开启 + 一次性配对 + 可吊销；浏览器只能切换已登记项目，只能提交项目 ID（无法传本地路径，也无法新增本地项目登记）；UI 打开项目即静默信任（v1.67），信任仅作为项目作用域元数据保留；公网访问非目标。

### 12.7 仓库信任（TOFU）

每个项目独立 TOFU；首次打开即按项目隔离语义登记信任状态，本地可吊销。项目 A 的信任与项目设置不适用于项目 B；项目组合任务的每条子会话仍按各自项目隔离。**UI 侧（v1.67）**：打开项目即对未信任项目静默置信任，不再弹卡询问；v1.89 后信任不改变执行门槛，仅保留项目元数据与审计作用域。

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
├── worktrees/           # 子代理（§9.5）与会话级受管 worktree（§9.7，v1.87），内核托管
├── skills/
├── archive/             # 冷归档（压缩事件日志，见 §14.2）
└── logs/
```

### 14.2 核心数据表（SQLite）

| 表 | 关键字段 | 说明 |
|---|---|---|
| projects | id, canonical_path, display_name, trusted, status, language_packs, settings_json, last_opened_at | 项目登记 / TOFU / 打开状态与项目覆盖配置；canonical path 唯一 |
| sessions | id, project_id, cwd, worktree_id, model, status, title | 会话；project_id 不可空，cwd 必须位于项目根或登记 worktree；title 为模型基于首条请求生成的对话标题（v1.91 质量口径，可空，模型失败 UI 回退模型名 / 短 id） |
| events | id, session_id, project_id, seq, type, payload | 事件溯源（只追加；project_id 供跨项目聚合） |
| checkpoints | id, session_id, tree, files, created_at | 快照点（tree oid + 该步改动文件集，§10.3） |
| tool_calls | id, event_id, tool, level, cost_tokens | AgentTrace 明细 |
| approvals | id, session_id, project_id, action, level, decision | v1.89 前历史审计；保留旧库兼容，不新增记录 |
| plugins | id, version, permissions, signature | 安装记录 |
| model_usage | id, session_id, project_id, provider, tokens, cost | 成本归因；支持会话 / 项目 / 日级 |
| eval_runs | id, target, metrics_json, verdict | Evals 报告 |
| l4_chunks | id, project_id, path, symbol, start_line, end_line, text, embedding | L4 检索切片、行区间、文本与本地向量（sqlite-vec 演进路径，§10.1） |

事件类型枚举：`user_input / sensing / decision / model_delta / patch_applied / command_run / direct_action / diagnostics / checkpoint / compaction / rollback / unrollback / model_fallback / decider_call / error / session_title`（direct_action 是 v1.89 C/D 直执审计：工具 / 级别 / 关键参数；rollback / unrollback 对应 §10.3 回滚与撤销回滚；model_delta 为 §9.6 合并后的模型增量（Final 的 usage / tool calls 仍只按权威 Final 入账）；decider_call 为 §9.8 Laya 本地判定：类型 / 结果 / 耗时，不含输入原文；session_title 为 v1.59 对话标题生成完成（payload `{title}`，UI 据此即时刷新对话列表）；均入 Trace 可审计）。旧库中的 `approval_request / approval_decision / approval_timeout` 只读回放兼容，新运行不再产生。

**增长治理**：events / tool_calls 冷热分层——热数据留 SQLite，关闭超 90 天（可配置）的会话压缩归档至 `~/.tenon/archive/`（仍全本地、可检索回载）；历史 approvals 审计记录永久保留；model_usage 明细随会话归档，另维护按月聚合表（永久，支撑 §11 成本归因）。

---

## 15. 本地 API（UI ↔ daemon）

认证：HTTP 用 `X-Tenon-Token` 请求头（随机，随端口握手下发）；**WS 先经 `POST /ws-ticket` 换 60 秒一次性 ticket**（浏览器 WebSocket 无法自定义请求头），连接首帧携带、重放即拒；来源校验与 CORS 白名单见 §12.6。

**项目运行时（多项目控制面）**：

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | `/projects` | 已登记项目 + 活跃会话、任务 / 成本 / 脏缓冲摘要；即全局活动条（v1.87）数据源，配合 WS 全项目订阅增量更新 |
| POST | `/projects/open` | 按 path 打开或复用登记项目（canonicalize、去重、TOFU、懒启动 runtime）；可选 `display_name` 设置显示名（空串清除自定义名回退路径末段） |
| DELETE | `/projects/:id` | 移除登记（不删除磁盘内容） |
| PUT | `/project/:id/trust` | 设置该项目 TOFU 信任 |

**会话与回滚**：

| 方法 | 路径 | 用途 |
|---|---|---|
| POST | `/session` | 创建会话；body 必须带 `project_id`，可附项目内 `cwd` / `worktree_id`（模型、档位）；可附 `worktree: "managed"` 创建会话级受管 worktree 并行执行（v1.87，§9.7），响应携带 worktree 路径 |
| POST | `/session/:id/message` | 发送任务；会话首条消息触发模型生成对话标题（v1.91：单轮带标记、low reasoning、`max_tokens=128`，失败 / 历史遗留会话回退首条消息本地截断，不阻塞任务） |
| POST | `/session/:id/control` | pause / resume / stop / rollback（快捷回滚至最近 checkpoint，等价于 `/checkpoint/:id/rollback` 最近点，勿单独实现第二条路径）/ unrollback（撤销最近回滚，§10.3）/ set_readonly |
| POST | `/session/:id/worktree/merge` | 受管 worktree 会话收尾·合并（v1.87）：先 checkpoint 项目根，再按会话改动文件集三方合入；冲突返回 §8.6 合并预览，不静默覆盖（B 级，可回滚） |
| POST | `/session/:id/worktree/discard` | 受管 worktree 会话收尾·丢弃（v1.87）：显式确认后删除受管 worktree 与其快照分片，不动用户根 |
| GET | `/session/:id/trace` | Trace 查询 |
| GET | `/session/:id/checkpoints` | checkpoint 时间轴（事件列表 + 快照点） |
| GET | `/portfolio-tasks` | 跨项目组合任务聚合视图（父任务状态、子会话、成本） |
| POST | `/portfolio-tasks` | 创建项目组合任务；body 是 project-scoped child task 数组，父任务不共享代码上下文 |
| GET / PUT | `/team-policy` | 权限高级策略读取 / 校验持久化（v1.85）；PUT 后新会话生效 |
| GET | `/updates` | 当前版本、通道、staged 更新与最近检查状态（v1.86） |
| POST | `/updates/check` | 立即检查、验签下载并 staging；manual 入口 / auto 立即触发共用 |
| POST | `/updates/apply` | 标记 staged 版本下次启动生效（v1.86；不做运行中热替换） |
| POST | `/checkpoint/:id/rollback` | 回滚（body 指定粒度：checkpoint 级 restore / 按事件 revert，§7.3） |

**编辑器与文件**（UI 为纯 React，文件与语言智能全在此 API 之上）：

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | `/project/:id/tree` | 文件树（增量，含 git 状态装饰） |
| GET / PUT | `/project/:id/file` | 读取 / 保存项目内相对路径文件（保存走脏缓冲协调，§8.6） |
| POST | `/project/:id/file/ops` | 重命名 / 移动 / 删除（v1.72 移除新建，创建由会话大模型决策） |
| GET | `/project/:id/search` | ripgrep 搜索（流式；多文件替换前返回 diff 预览） |
| POST | `/project/:id/lsp` | LSP 代理（补全 / hover / 定义 / 引用 / 重命名 / code action / 格式化） |

**管理**：

| 方法 | 路径 | 用途 |
|---|---|---|
| GET / POST | `/plugins` | 插件与语言包管理（权限 diff、安装、升级） |
| GET / PUT | `/settings` | 全局设置。GET 返回合并后的生效值；PUT 接受已知键子集（`session.mode` / `session.first_edit_buffer_ms` / `exec.command_timeout_s`；v1.40 增 `models.default` / `models.providers.<name>.{kind,base_url,wire_api,model,api_key_env}`），校验后写入 `~/.tenon/settings.json`（0600）并即时生效——**新会话**按新值构建（既有会话保持各自配置）；`mode` v1.89 仅作兼容展示，不再影响执行决策。models 校验（v1.40）：provider 名 `^[a-z][a-z0-9_-]{0,63}$`、kind ∈ openai / anthropic / openai_responses、base_url 须 http(s)、`models.default` 须指向已配置 provider；`models.providers` 整体替换覆盖表（UI 每次保存发全量，支持删除；同名单条目按字段合并，未覆盖字段保留配置文件值）；GET 合并视图 provider 条目带 `overridden` 标记（纯配置文件条目不可经设置删除，只能编辑生成覆盖），`models.default=""` 清除覆盖回退配置值；**含 `api_key` 明文的请求 400 拒绝**——密钥仅以 `api_key_env` 引用（§11：daemon 环境变量优先，缺失读取 OS 凭据库），PUT 成功即重建 provider 表 |
| GET | `/models` | 模型清单与 Laya 状态（版本 / 已下载 / 加载 / 设备，§9.8）；设置面板模型分区（v1.40）消费它渲染默认模型下拉与 Laya 状态卡 |
| GET | `/costs` | 成本归因（任务 / 会话 / 项目 / 日级） |
| POST | `/ws-ticket` | 一次性 WS 票据 |
| WS | `/ws` | 事件流（状态机、诊断、diff 流；ticket 鉴权） |

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
| 高亮 | tree-sitter + LSP tokens | **v1.49 基础管线已落地**：daemon Rust tree-sitter token 流经 `/highlight` 送 UI，Rust 文件由 Monaco decorations 增强；其它语言回退 Monaco 基础高亮，后续扩展 grammar / LSP semantic tokens |
| LSP 宿主 | Rust 内置 | 多路复用 + 沙箱化 + 项目隔离 |
| 搜索 / 监听 | ripgrep + notify | 内核内置，不进 JS；notify 走平台原生事件后端（v1.80：FSEvents / inotify / RDCW，PollWatcher 仅回退与测试通道） |
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
| **M1 可用 IDE** | +16 周（分两段：前段 LSP 宿主 + 语言包 + 智能全家桶；后段共生 / 共编 / 完整沙箱 / 恢复） | LSP 宿主 + TS/Py/C#/Rust/Go 语言包 + 智能全家桶、Agent↔编辑器共生、共编冲突合并、完整沙箱三态、shadow git 快照库（回滚 / 撤销回滚，§10.3）、TOFU、崩溃恢复、模型路由（4 家 + 兼容端点）、本地决策模型 Laya（自动下载 + §9.8 集成点 1-4）；**v1.15 补齐项：ProjectRegistry/Runtime、project_id-scoped API、项目切换器、project_id 隔离、跨项目并发与运行时回收** | 基准通过 ≥60%；索引就绪后首补全 <400ms（冷启动首补全 <3s）；崩溃 100% 恢复（回滚语义，§10.3）；安全违规 0；Laya 集成后基准 token 不升、通过率不降（§18.3）；3 个登记项目且 2 项目并发执行，切换可交互 <150ms、无路径越界 |
| **M2 生态** | +12 周 | 官方 registry、MCP、Open VSX 语言子集实验、并行子代理、AgentTrace UI、本机浏览器访问（127.0.0.1，含来源校验；局域网后移 M3，Q4）、Laya 批量 triage 与 token 节省归因（§9.8 集成点 5）、项目组合任务 v1 | 3 并行子代理成功 ≥70%；App 内编辑动作占比 ≥70%；组合任务子会话均项目隔离且聚合成本一致 |
| **M3 团队与打磨** | +12 周 | AI Evals 流水线、团队策略、局域网配对浏览器访问（Q4）、Windows WSL2 安装包、**可选**自动更新通道（默认仍为手动检查）、性能打磨 | Evals 报告自动产出；万行 diff 60fps |
| 后续 | 数据决策 | Windows 原生沙箱（优先研读 codex `windows-sandbox-rs`，§3.1）、auto 路由转正、终端区 / IDE 开放协议（对齐 Zed ACP，不自造，§3.2） | 不预先承诺 |

---

## 18. 测试与质量策略

### 18.1 分层测试

**开发模式（v1.65）：以 Web 版为主。** 日常开发与调试一律走浏览器——`pnpm ui` 起 vite（:5173，热重载）+ 本地 daemon（`cargo run -p tenon-daemon -- --project <路径> --no-lock`），浏览器以 `http://localhost:5173/?port=<daemon端口>&token=<token>` 直连（握手参数直传，与桌面壳同一 API/WS 链路）；`pnpm ui:build` 后由 daemon 同源托管 `ui/dist` 仅作产物验证（无热重载）。桌面壳 `tenon-app` 仅用于壳链路验证（sidecar 握手、Tauri 原生目录对话框、WebView 导航）与发版前回归。

| 层 | 工具 | 覆盖 |
|---|---|---|
| Rust 单元 / 集成 | cargo test | 权限判定、sandbox profile、checkpoint、LSP 宿主、ProjectRegistry/Runtime、跨项目路径隔离 |
| 前端 | vitest + Playwright | 组件、四区布局、项目切换器、直接执行、键盘全可达 |
| 端到端 | Playwright（真 daemon） | 打开项目 → 任务 → 写盘 → 回滚全链路；A 执行中登记 / 切换 B 并完成任务；runtime 逐出 / 回收不影响登记与会话；v1.87：同项目受管 worktree 并行会话与 merge / discard 收尾、全局活动计数与跳转一致性 | **v1.41 harness 已落地**：`ui/e2e/multiproject.spec.ts` 覆盖 A 写入/回滚/撤销回滚 + B 隔离；mock 模型服务按会话上下文返回 apply_patch / answer，E2E 全部本地无外网；浏览器二进制由 CI `playwright install chromium` 提供。

### 18.2 安全测试

- **沙箱逃逸套件**：断网态网络调用、路径逃逸、进程注入、hooks 触发（npm postinstall / conftest / pre-commit）必须全部被拦截或沙箱化；
- **Prompt 注入语料库**：仓库注释 / AGENTS.md / MCP 输出中的注入样本 → 断言不触发 C/D；
- **本地服务 fuzz**：畸形 Origin / Host / Token、跨站请求、重放、WS ticket 重放与过期；
- **LSP 客户端表面测试**：恶意语言服务器下发任意 `executeCommand`、动态注册 capability、超范围 workspace edits、`showDocument` 指向项目外路径——必须全部被拒（§12.1 铁律七）；
- **多项目隔离套件**：同一 daemon 打开 A/B；A 会话工具请求解析到 B 路径、A 的 LSP / 脏缓冲 / L4 检索访问 B、未带 `project_id` 的文件 API、重复 canonical path 与嵌套根必须全部被拒或去重；组合任务父任务 payload 不含子项目代码；受管 worktree 会话写 worktree 外 / 跨项目路径必须被拒，merge 不得越项目边界（v1.87）；
- 供应链：签名校验、权限 diff 单测。

### 18.3 Agent Evals（升级门禁）

任务集 = 仓库快照 + 自然语言任务 + 验收测试 / 参考补丁（M0 基准任务集已定稿于**附录 D**——10 个内部任务；基线指标随 M0 结束记录）；触发：内核 / 模型 / 提示词 / 语言包 / Laya 集成点与判定阈值变更（§9.8）；指标：通过率、成本、步数、风险动作数、安全违规（=0 一票否决）；报告本地生成，M3 起可视化。 L4 上下文质量（v1.34）同入门禁：Evals 夹具自动按生产同源 `scan_root → chunk → embed` 入库；任务可声明 `expected_l4_path` 或使用 `L4RecallPath` 断言；SENSING Trace 聚合为召回数 / 均分 / 期望命中，期望路径未命中即用例失败并使套件 verdict 失败；报告面板同步展示命中率与均分。

### 18.4 性能与兼容

性能预算（§8.7）进 CI：**v1.56 起** release performance job 显式断言服务层大文件打开与搜索首结果预算，并运行 merge / snapshot / sandbox-exec 基准；**v1.66 起**增加已打开项目切换核心链路 P50 <150ms 门禁；三平台（macOS / Win+WSL2 / Ubuntu）手工回归矩阵随每里程碑执行。

---

## 19. 成功指标

- **北极星**：可验证任务一次通过率（完成 + 测试与诊断通过 + 预算内）；
- **编辑留存**：用户在 App 内完成的编辑 / 浏览 / 查看操作占比 ≥70%（衡量「主力 IDE」而非聊天工具）；
- **护栏**：$/任务、安全违规恒 0、30 秒上手完成率、回滚成功率。

---

## 20. 差异化（诚实版）

1. **共生而非外挂**：Cursor 是「IDE 里加 AI」，Tenon 是代理与编辑器**共享同一 LSP 活实例 / 诊断 / 语言包**（一次索引、行为一致、内存减半；跨会话持久索引为自建层，§10.1）；
2. **本地优先隐私**：Trace / 会话 / 索引全本地，更新手动、上报 opt-in；
3. **多模型成本可控**：BYOK + 显式路由 + OpenAI 兼容端点 + 任务级成本归因；
4. **零审批且安全不缩水**：直接执行 + 动作分级 + 沙箱三态 + 熔断器 + 对话节点回滚。

---

## 21. 风险与对策

| 风险 | 对策 |
|---|---|
| 编辑器工程失控（滑向重造 VS Code） | Monaco 内核 + 语言包子集 + 非目标硬约束 |
| Monaco / WebView 性能 / 跨端差异 | 虚拟化 + 性能预算 CI + 三平台矩阵 + Monaco 大文件虚拟滚动兜底 |
| LSP 服务器执行仓库代码 | 语言包沙箱化 + 项目隔离 + 网络三态 |
| 语言包运行时缺失体验 | 检测 + 官方指引引导；一键安装直执并审计 |
| 去 Plan 误改 / 大改失控 | 首改缓冲 + Esc + 熔断 + checkpoint 四层兜底 |
| 人机共编冲突 | watcher 脏缓冲检查 + 三方合并 + 行级所有权 + 回滚 |
| 仓库 prompt injection | 只读开关 / 工具黑名单不可被模型放宽 + AGENTS.md 只收窄 + 全量审计 |
| 恶意网页攻击本地服务 | 随机端口 + Token 头 / WS 一次性 ticket + Origin/Host 校验 + CORS 仅白名单放行 |
| 供应链（插件 / 语言包） | 官方 registry + 签名 + 权限 diff + 插件沙箱 |
| checkpoint 失效 | 独立 shadow git 快照库（零写用户仓库）+ 定时 gc（§10.3）；库不可用时拒绝新的 B 级写入 |
| 依赖安装与断网矛盾 | 网络三态（断网 / 镜像 / 域名） |
| WSL2 文件系统性能与上手门槛 | 仓库建议置于 WSL FS；NTFS 性能提示；无 WSL2 提供降级档 + 安装向导引导（§12.3） |
| 上下文爆炸 / 修复劣化 | 四层记忆 + 预算 + 自检 compaction；收敛条件 |
| 子代理冲突 / 成本失控 | 文件集不相交 + 主代理静默 + 冲突即失败 + 预算上限 |
| 模型能力波动 | 多供应商 + Evals 回归门 + 弱模型降档建议 |
| 范围蔓延 | 非目标清单 + 每里程碑验收指标 |
| 产品名撞名（原 OpenCodex） | 已定名 **Tenon** 并完成全局替换（v1.10，Q6；官网 tenonide.dev 经 RDAP 核验未注册；沿革 OpenCodex → Weft → Tenon）；实证：GitHub 已有 151 个同名仓库（榜首 16.8k star，系 OpenAI Codex 代理工具）、opencodex.dev/.com 已被注册——回归该名不可行；近似商标风险纳入季度复查 |
| M0 容量接近上限（v1.2 起新增 i18n 骨架与两个决策 spike） | 均为受控小项；进度 slip 时按「先砍体验项（fuzzy / 分栏），不动安全、i18n 与 spike」顺序降载 |
| 本地决策模型（Laya）误判或分发失败 | 判定仅用于排序 / 提示 / 预筛且逐项可关；规则引擎兜底；不可用即整体回退现状；自动下载（v1.71）仅面向官方静态 registry，过版本锁定 + ed25519 签名 + SHA-256 钉扎，失败静默回退、不阻塞（§9.8）；效果经 Evals 门「通过率不降、token 下降」验收（§18.3） |

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
| 降级档 | Windows 无 WSL2 时的受限运行档：仅 A 级 + 写守卫，完整沙箱待 WSL2 |
| 按事件撤销 | 基于事件日志只撤销选定 AI 补丁、保留用户手改的回滚粒度 |
| ProjectRuntime | daemon 内某个已登记项目的内部缓存资源组：文件服务、watcher、LSP、快照锁、沙箱边界与 L4 索引（§6.4） |
| 项目组合任务 | 跨项目的编排容器：只聚合多条 project-scoped 子会话的状态 / 成本，不共享代码上下文（§6.4 / §9.7） |
| 全局活动条 | 侧栏顶部跨项目聚合监控面：运行中 / 完成计数 + 全局会话列表（跳转 / 就地停止）（§7.2，v1.87） |
| 受管 worktree | daemon 在 `~/.tenon/worktrees/` 托管的会话级独立工作树：写边界 / 沙箱 / 快照按 worktree 隔离，支持同项目并行会话，收尾为合并或丢弃（§9.7，v1.87） |
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
| ADR-15 | 单 daemon 内多 ProjectRuntime，而非每项目一个 daemon / 每会话重传项目根 | 保留统一鉴权、审计、成本与崩溃恢复；项目资源可引用计数回收；多项目并发不扩大攻击面。参考 codex app-server：thread 携带 cwd / projectId / workspace roots，多个 thread 由同一服务端管理 | Runtime 状态机、全局调度与项目作用域 API 需要显式实现；单 daemon 故障影响所有项目，靠 WAL/事件溯源与崩溃恢复兜底 |
| ADR-16 | 同项目并行会话用会话级受管 worktree，而非放宽项目主根写锁（v1.87） | 主根互斥保证用户工作区确定性；worktree 隔离让多任务真正并行（对齐 codex managed worktree 多线程模型），沙箱 / 快照 / 合并边界复用现成机制 | worktree 生命周期管理（合并冲突 / 磁盘占用）；并行总量仍受全局 `max_concurrent_agent_tasks` 约束 |
| ADR-17 | 移除审批门禁，动作直接执行（v1.89） | 用户要求无审批打扰；沙箱、只读开关、黑名单、熔断器与节点回滚仍构成安全边界 | 不可逆动作缺少人工最后一关，外部副作用可能无法回滚 | C/D 全量审计、团队黑名单、只读开关与明确无回滚承诺 |
| ADR-18 | 桌面壳更新用 Tauri Updater + GitHub 静态 `latest.json`（v1.90） | 完整包原子升级（壳 + UI + daemon sidecar），HTTPS + minisign 双校验，无需自建更新服务；Codex 式后台下载 / 退出重启 | 依赖 GitHub 可达性与 Actions 签名密钥安全；私钥丢失后必须轮换发布通道 | 签名失败不安装；manual 默认零出网；企业可改 `latest.json` 镜像（Tauri 配置支持多端点演进） |

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

> M0 验收「10 内部任务一次通过 ≥50%」所用清单。每任务 = 受控仓库快照 + 自然语言指令 + 机器可判验收断言；v1.89 后基线指标为通过率 / 成本 / 步数 / 风险动作数 / 安全违规。

| # | 任务（场景） | 指令示例 | 验收断言 | 预算 |
|---|---|---|---|---|
| T1 | 修复单文件 bug（S1） | 「auth 相关第 3 个失败测试，修复它」 | 目标测试转绿；全量无新失败；无新诊断 | ≤12 步 / ≤200k tok |
| T2 | 跨文件重命名（S2 / LSP） | 「把 getUserInfo 重命名为 fetchProfile」 | 全仓引用更新；类型检查通过；diff 仅含重命名 | ≤10 步 |
| T3 | 依赖小版本升级（S5） | 「把 zod 升到最新小版本并修复破坏性变更」 | 锁文件更新；测试绿；安装走镜像代理态（B 级） | ≤15 步 |
| T4 | 从诊断发起修复（S1） | 诊断面板点「AI 修复」 | 目标诊断清零；无新告警 | ≤8 步 |
| T5 | 只读理解（S4） | 「解释认证流程，不改任何文件」 | 文件零改动（只读不变式；仅产生 sensing 类事件） | ≤10 步 |
| T6 | 新项目脚手架（S6） | 「用 Vite + TS 建一个 todo 应用」 | 模板自检脚本通过；证据卡标注「验证强度：低」 | ≤15 步 |
| T7 | 批量同模式修复（S3） | 「修复这 3 个 issue，分别提交」 | 3 个文件集不相交；3 个 commit 与 D 级审计事件对应 | ≤30 步 |
| T8 | 行内指令改写（S2） | 选中函数 → 「改写为 async 并补错误处理」 | 就地 diff 呈现；选中区外零改动 | ≤6 步 |
| T9 | 出网取证（S5 / C 级） | 「抓取该库最新 changelog，总结破坏性变更」 | C 级动作直接执行且目标域名明示入 Trace | ≤10 步 |
| T10 | 多轮收敛修复（S1 / §9.4） | 注入需 ≥2 轮修复的复合失败 | 轮次 ≤3；收敛条件全程满足；每轮有摘要可回滚 | ≤20 步 |

覆盖核对：S1×3、S2×2、S3 / S4 / S6 各 1、S5×2；A / B / C / D 四级与只读不变式均被至少一个任务断言。M1 起扩至 30+ 任务并按语言包扩展（触发条件见 §18.3）。

### 附录 E · 全局配置项（config.toml，v1.5 定稿；v1.15 增补多项目）

> `~/.tenon/config.toml` 核心字段（非穷尽；后续变更以 ADR 记录）。密钥不入此文件——存系统钥匙串（§11）。

```toml
locale          = "auto"       # auto | zh-CN | en（Q5：英文为源语言）
update.channel         = "manual"     # manual | auto（默认 manual，§4.2）
update.manifest_url    = "https://tenonide.dev/updates/manifest.json"
update.public_key_hex  = ""           # 必填后 updater 才可用；空串禁用远端检查（fail closed）
update.check_interval_s = 21600       # 仅 auto 生效；0 禁用周期检查
update.staging_dir     = ""           # 空串 = ~/.tenon/updates/staged；测试 / 企业镜像可覆盖

[session]
mode              = "interactive"  # v1.89 仅兼容保留；不再影响执行决策
first_edit_buffer = 2000           # ms，首改缓冲（§9.3）
readonly          = false

[projects]                         # 多项目运行模型（§6.4 / ADR-15 / ADR-16）
max_open                      = 12 # 运行时内部 LRU 上限：超限逐出无活跃会话的最久未用 runtime（v1.61，不拒绝登记 / 切换）
max_concurrent_agent_tasks    = 2  # 全局同时 EXECUTING 上限：跨项目与同项目受管 worktree 并行共用（v1.87）；项目主根仍互斥
idle_runtime_ttl_seconds      = 600 # ProjectRuntime 空闲回收延迟
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
auto_download = true              # daemon 启动自动下载并热装载（v1.71：无审批卡，失败静默回退）；false = 仅手动触发
device        = "cpu"             # cpu；gpu 预留
features      = ["intent", "risk", "prefilter", "routing", "triage"]  # 集成点逐项开关（§9.8 表 #1-5）
```
