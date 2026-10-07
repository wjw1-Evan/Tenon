# AGENTS.md

## 项目

**Tenon**（tenonide.dev）：开源 Agent 优先桌面开发环境——Rust daemon 内核 + Tauri 2 桌面壳 + React/Monaco UI。**平台优先级 macOS 为主**（Windows 走 WSL2 路径）。

## 铁律：设计驱动开发

**`docs/design.md` 是开发的唯一依据**，当前版本以文件头部为准（工作树常领先于 README/HEAD）；全量版本演进表在 **`docs/design-changelog.md`**（v1.132 自 design.md 头部外移，只追加不改写）：

1. 动手前先读 design.md 相关章节（实施：§6-18；安全：§12 全篇）；
2. 发现设计缺陷 / 遗漏 / 与实现冲突：**先改 design.md 再写代码**，不允许代码与设计静默偏离；
3. 每次设计变更在 `docs/design-changelog.md` 表尾加一行（递增版本号 + 一行要点），并同步 design.md 头部「版本 / 日期」元数据。

版本号与协作注意：

- **并行 agent 会话共用此工作树**：design.md 版本号会被并行会话占用，加版本前必须先查工作树里的最新版本；禁用 `git stash`；提交时按 hunk 只提自己的改动；功能勿依赖他人未提交的设施；禁用脚本盲改共享在途文件；
- README 不维护版本更新记录（版本明细由 git 历史与 `docs/design-changelog.md` 演进表承载），勿向 README 追加版本条目；
- 方向性 UI/UX 改动先给双参考方案（如 Codex 形态 vs ZCode 形态）供用户选择，再实现。

## 代码结构（monorepo，ADR-13）

```text
crates/
├── tenon-config      # config.toml schema
├── tenon-store       # SQLite：事件溯源 / 审批审计 / 成本归因
├── tenon-core        # 动作 A/B/C/D 分级 / 熔断器 / 三方合并 / 上下文与提示
├── tenon-snapshot    # shadow git 独立快照库：snapshot / revert / restore / unrevert
├── tenon-sandbox     # 写守卫 / 网络三态 / Seatbelt+Landlock+seccomp / 超时执行
├── tenon-models      # provider trait：OpenAI 兼容 / Anthropic / mock；路由与成本
├── tenon-lsp         # LSP 宿主：多路复用 + 守卫（编辑器与 Agent 共享同一实例）
├── tenon-fs          # 文件树 / 读写 / rg 搜索 / fuzzy / watcher / 脏缓冲
├── tenon-laya        # 本地决策模型：choice/score/bool 三原语
├── tenon-agent       # Agent 循环：感知→判断→执行→验证 + 审批 + Evals 运行器
├── tenon-registry / tenon-mcp   # 静态 registry 客户端 / MCP 外部插件
└── tenon-daemon      # 本地 HTTP+WS API：token / WS 票据 / 配对 / 团队策略
ui/                   # React + TS + Monaco 四区工作区 + i18n（中英）+ vitest + Playwright
app/src-tauri/        # Tauri 桌面壳：daemon sidecar + 握手注入
scripts/              # dev.sh / dev-daemon.sh（热重载）；install-windows.ps1
evals/                # GLM 基线（baseline-glm.json）
```

## 常用命令

开发主通道是**浏览器 Web 版**（vite 热重载）；桌面壳仅用于壳链路验证（sidecar 握手 / 原生目录对话框）：

```bash
pnpm dev            # 一键：dev daemon + vite HMR。dev daemon 固定 127.0.0.1:9876 + token "dev"，
                    # HOME 隔离到 .tenon-dev/，crates/**.rs 改动自动重建重启；浏览器开 http://localhost:5173
pnpm dev:daemon     # 仅 dev daemon（配合 pnpm ui）
```

dev 会话退出（INT/TERM/HUP/正常退出）自动清理 `target/debug/incremental` 纯缓存（deps/ 不动，防 target 无 GC 膨胀）；`TENON_DEV_NOCLEAN=1` 跳过，检测到并行 cargo/rustc 进程自动让路。

检查与测试（CI 同款，提交前跑齐）：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings   # 0 警告是硬门槛
cargo test --workspace        # 无 config.local.toml 时 GLM 真实端点联调测试自动跳过
pnpm ui:test                  # vitest（ui/ 内）
pnpm ui:build                 # tsc -b && vite build——UI typecheck 在这里
pnpm ui:test:e2e              # Playwright 真 daemon E2E（CI 先 playwright install chromium）
cargo test -p tenon-fs --release --test performance_budgets -- --ignored --nocapture  # 性能门禁
cargo run -p tenon-evals -- --provider glm   # 附录 D 10 任务基准
```

## 架构边界

- **daemon 是唯一服务端**：UI 只经 HTTP+WS API（token 鉴权 + WS 票据）访问文件 / 搜索 / LSP / 会话，不直接碰 fs；
- **一切按 `project_id` 作用域**：文件 / 搜索 / LSP / 脏缓冲 / 会话 / WS 事件全部 project-scoped；跨项目只做全局任务聚合，不做隐式上下文共享；
- **编辑器与 Agent 共享同一 LSP 宿主**（tenon-lsp 多路复用）：一次索引行为一致，Agent `lsp_query` 与编辑器诊断同实例；
- **回滚走独立 shadow git 快照库**（tenon-snapshot）：零写用户仓库 `.git`；
- **沙箱三态**：macOS Seatbelt / Linux Landlock+seccomp / Windows WSL2（tenon-sandbox）；
- **密钥存储双轨**（v1.165 用户裁定简化）：`api_key` 可直存配置文件（settings.json / config.local.toml，0600；GET /settings 永不回显），或经 `api_key_env` 环境变量 / OS 凭据库引用（解析优先级见 design.md §11）；
- **模型接入**：默认 `~/.tenon/config.toml`（schema 见 design.md 附录 E）；仓库根 `config.local.toml`（gitignored）仅供本地联调，dev 脚本会显式 `--config` 传入。

## 约定

- 每次修改完成后记得提交源码（commit），不要把已完成的改动留在工作树；
- 注释、文档、commit message 用**中文**；UI 文案**中英双语**，改文案须同步 `ui/src/locales/` 两个语言文件；
- UI 有 vitest 门禁含性能预算（如 20k 行 diff DOM 物化预算），改 DiffPanel 等组件注意别破预算；
- daemon 数据落 `~/.tenon/`（settings.json 0600）；dev 模式隔离在 `.tenon-dev/`，勿混用。
