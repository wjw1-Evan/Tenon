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

**M0 已验收**：附录 D 基准 10 任务接 GLM 真实模型 **8/10 通过、安全违规 0**（基线见 `evals/baseline-glm.json`）；**M1 / M2 / M3 全部实现**（多项目、语言包、沙箱三态、快照回滚、模型路由、Laya 决策模型、registry / 并行子代理、Evals、团队策略、局域网配对、自动更新执行器、Windows WSL2 安装脚本）。各版本变更明细见设计方案「版本演进」表（当前 v1.121：设置重排为 Codex 式模态框——左侧 General / Models / Permissions / Plugins / Updates 分类导航，右侧只渲染当前分类；General 承载保存方式与代理参数，设置 API、全局保存与密钥不变式不变；v1.120：删除 rail 顶部「收起 / 展开侧栏」切换钮（用户令「删除 该按钮」）——rail-sidebar-toggle testid 退役，侧栏开合收敛 rail 同视图再点 / 命令面板 toggle.sidebar，palette.collapse_sidebar / expand_sidebar 死键五语言清除；v1.119：顶栏外观档下拉改单按钮三态循环——跟随系统 → 浅色 → 深色（🖥 / ☀ / ☾ 图标随档位切换，点击推进下一档、末端回绕），三档语义 / data-theme 换色 / 偏好双写不变，theme-picker testid 保留、aria-label 合成当前档名、零新增语言键；v1.118：「任务 / 源码」视图切换收敛为项目名后常驻文字钮——撤销 v1.114 分组行尾 hover 文件夹图标开关，钮面显示目标视图名（任务视图显「源码」、源码视图显「任务」），tenon:peView 记忆与 pe-source-* testid 不变，「最近更新」时间移出主按钮；v1.117：其余视图「收侧栏」钮全数移除——search / packs 视图 side-head 收起钮删除（sidebar-collapse testid 退役），侧栏开合全线收敛 rail 顶部切换钮 / rail 同视图再点 / 命令面板；v1.116：新任务草稿态，列表只展示真正开始的任务——「＋ 新任务」不再急切建会话（per-project 草稿记录主根 / worktree 意图，线程切空任务输入），首条消息发出才落库建会话并发送；打开无会话项目同样入草稿态、不再产生从未开始的空会话；无标题（未开始）会话不进任务列表（含已归档组），daemon 生成标题（session_title）后即现，v1.101「无标题灰显」由「不显示」取代；轮询兜底跳过草稿态项目、默认只选已开始会话；v1.115：左栏「项目」标题行去收起钮 + 默认宽收窄——pe-head 移除 v1.106 收侧栏钮（侧栏开合收敛 rail 切换钮 / rail 同视图再点 / 命令面板），左栏默认宽 220→180px（仅无记忆尺寸的新档案默认）；v1.114：侧栏「项目」视图重构为 Codex 任务流形态（双参考方案选定）——项目从文件夹行降为小字分组标题（点击隐式激活 + 折叠组，v1.60/63 语义不变），任务行平铺组下（状态图标化：运行 spinner / done=✓ / error=✗ / 其余状态色点，行尾相对时间，hover 浮现归档/删除）；「任务 | 源码」tab 撤销、源码树收为分组行尾文件夹图标开关（tenon:peView 记忆不变）；新任务入口重排：标题行「＋ 新任务」主按钮（active 项目）+ 分组行尾 ⎡（worktree），列表底部回归「＋ 添加项目」常驻行，「+ Session」双按钮行移除；v1.113：跟随模式不再抢占弹浮层——E2E 抓出的 v1.110 缺陷（任务执行中代理写入即弹全屏浮层并滞留、遮挡线程与底部面板）：§8.5 跟随模式修订为数据就绪不抢屏，代理写入仍打开 tab / 标 AI 行 / 记定位，浮层已开才定位改动行、未开由用户主动打开；v1.112：会话流降噪——A 级只读工具步骤按回合聚合为一行 muted 摘要（失败 / 被拒仍单独红显），memory_saved / compaction / model_fallback 不再渲染，验证证据空壳不渲染；v1.111：消息级撤销——用户消息气泡下「撤销」钮回滚该回合全部 AI 改动（恢复到回合首步写前 checkpoint，unrevert 可逆），仅最后含改动回合提供、运行态禁用；checkpoint_rollback 端点修复为真按 id 恢复（rollback_to_checkpoint 提取，rollback_last 重构为特例）；v1.110：源码查看回侧栏 + 编辑器转应用内浮层——侧栏「项目」展开区顶部「任务 | 源码」行内切换（per-project 记忆 tenon:peView，默认任务，源码=该项目文件树）；右区整体退场（源码树 / 源码控制组 / 整体开关 / 编辑器停靠窗格与 rightWidth 全链删除，线程恒满宽）；单击文件弹全屏编辑器浮层（EditorPane 整体迁入，✕ 关闭返回线程、最后 tab 关闭浮层随收、activePath 保留供底部诊断）；文件拖入任务输入框插入 `@path` 引用（dragover 高亮，agent 自主读取）；v1.109：会话过程流重构为 Codex 形态——事件流按 user_input 切分回合（用户任务块 + 步骤卡 / 风险卡 + 助手 markdown 正文），工具步骤卡折叠（状态图标 + 动词短语 + 目标摘要，展开看 args 与输出，失败红显默认展开），daemon patch_applied / command_run 事件补 args，零依赖受限 markdown 渲染器，信息事件降级 muted 单行；v1.108：源码区整体开合——右区（源码树 + 编辑器）一区一开关：开启即停靠（无 tab 显编辑器空态），关闭整体隐藏（tab 状态保留、重开恢复），右缘细条为关闭态常驻恢复入口，任意打开文件入口自动唤出，命令面板 toggle.source；v1.107：源码显示收敛到右区「源码树」——审查窗格左缘单列合并树：文件树（git 状态装饰）+「全部 / 仅变更」过滤 + 列尾「源码控制」折叠组（分支 / 最近提交 / 活动 blame），作用域收敛 active 项目；移除左区项目行「源码」内嵌文件树与底部「源码」tab，底部保留时间轴 / 轨迹 / 评估；v1.106：左栏显式展开 / 收起开关——rail 顶部常驻切换钮（收起后经此钮或任意视图图标恢复）+ 视图标题行右端「收侧栏」钮；v1.105：对话 token 消耗优化（§10.2 历史压缩落地）——任务内输入超 24k token 预算即压缩：保留最近 4 条工具输出原文、更早的存根化（tool_call_id 配对不变、存根幂等，任务文本与 L1 工作集永不省略），压缩事件 compaction 入 Trace 可查；L2 目标有界化（最近 8 条），系统提示不随会话长度线性膨胀；v1.104：AI 对话记忆系统（L5 跨会话记忆）——§10.1 四层扩五层，新 memories 表（schema v9）双作用域 scope（project / global）× kind（preference / fact / decision / workflow），global 仅用户偏好、永不承载仓库内容；任务完成后单轮提取调用（无工具、严格 JSON、失败静默回退）+ 手动 API 写入，本地确定性 embedding 余弦 ≥0.90 去重合并；任务启动按 importance × 新鲜度注入系统提示「跨会话记忆（L5）」节（top16 / 1.5k token 预算，标注参考数据非指令）；每项目上限 200 条治理；`[memories] enabled` 开关；`GET / POST /project/:id/memories` + `DELETE /memories/:id`；UI 管理面板另行评审；v1.103：会话手动归档与删除——行内「归档 / 删除 / 还原」（删除 confirm 门），schema 新增 sessions.archived_at（默认隐藏、可还原、数据不出库），删除为事务级联清事件 / 工具调用 / checkpoint / 用量（快照交由 gc 老化）；`POST /session/:id/archive | unarchive`、`DELETE /session/:id`（运行中 / 未收尾 worktree 409），项目摘要新增 archived_sessions，「已归档 N」折叠组；v1.102：Laya 分发多镜像 + 内置 starter 兜底——registry 清单多镜像链（官方域名 → GitHub Pages → jsDelivr 顺序尝试）、清单新增模型镜像列表 urls（SHA-256 钉扎），全部失败且未装载时安装内置 starter 模型保证离线首启可用，registry 可达后按版本自动升级；v1.101：项目视图排版紧凑化——「+」并入视图标题行右端、移除「Name / Updated」列头；会话行状态改 §7.5 状态色圆点（文字转 hover）、行高压缩、已回滚 / 无标题回退会话灰显降噪；显示层清洗截断 prompt 前缀标题（如 The user's message is: "），空状态摘要不渲染；v1.100：多语言支持——新增繁體中文 / 日本語 / 한국어 三个社区语言包（288 键全量、与 en 键集对齐测试门禁，新增语言 = locales/&lt;tag&gt;.json + LOCALES 注册表一行），en + zh-CN 静态随包、其余懒加载独立分块（主包零增量），resolveLocale 升级 BCP-47 前缀匹配（繁体区→zh-TW），语言偏好 localStorage + ui-prefs 双写、启动屏预载语言包首绘即目标语言、`&lt;html lang&gt;` 随语言切换；v1.99：i18n 硬编码清扫三批——main.tsx 启动屏 5 处、Evals 列头 target/tokens/steps 与 Trace seq、FileTree 空目录与操作失败文案、顶栏语言 Auto 项与选择器 aria-label、证据 error fallback 接入双语（16 键，中英双向扫描），语言名母语显示与协议串豁免；v1.98：移除「All activity」全局活动组——v1.87 引入、v1.88 降为列表组的跨项目聚合监控面整链删除（项目行徽标与行内会话列表已承载活动信息），「不做系统级推送、attention 以徽标为准」不变式保留；v1.97：i18n 落地收尾二批——剩余硬编码文案清零（模型切换 toast、diff 空态、Monaco 加载占位与 AI 行 hover、修复诊断与行内指令任务模板）+ 日期时间本地化（新增 useResolvedLocale，ProjectExplorer / CheckpointTimeline / L4StatusPanel / GitSourcePanel 时间格式随应用内语言切换）；v1.96：移除项目视图顶部搜索——撤销 v1.88「搜索 + 添加入口」中的搜索半边（登记项目数量级小、即时过滤无实用价值），工具栏仅存添加入口，过滤链路 / 空态 / 双语文案 / 样式整链删除；v1.95：术语统一——§4.2「国际化」更名「i18n」；v1.94：i18n 合规——Evals / AgentTrace / 语言包 / 冲突合并四面板接入双语文案（此前整组件绕过 t()），snapshot / store 死函数收尾；v1.93：断链接线实装——暂停改真挂起等待恢复（此前 resume 为空操作）并堵任务重入竞态、`set_readonly` 实装 + 命令面板只读开关、provider 可配单价接入成本归因与 token / 预算熔断、shadow 库按 `keep_days` 周期 gc、超期会话每日归档、审批事件 / 月日聚合 / Mode 等死符号与死配置字段清扫，已落地；v1.92：功能面收敛——移除 legacy 隐式项目端点 / 组合任务自循环 / Open VSX 实验 / 会话档位 / 遥测开关等无用功能，修复 `/project/:id/lsp` 假作用域，接线 Laya 会话注入与 AGENTS.md 项目规则，§9.8 收敛为三集成点）。

测试：**Rust 367 + performance gates 3 + Vitest 144 全绿**；clippy 0 警告；Playwright 真 daemon E2E 覆盖多项目隔离与冷启动预算。

## License

Apache-2.0
