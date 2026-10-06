// daemon 本地 API 客户端（设计方案 §15）。
// 认证：HTTP 用 X-Tenon-Token；WS 用一次性 ticket（首帧携带，ADR-10）。

/** daemon 错误体统一为 {"error": "..."}；解析失败回退原文，避免把原始 JSON 泄漏进 UI。 */
function daemonErrorMessage(raw: string): string {
  try {
    const parsed = JSON.parse(raw) as { error?: unknown };
    if (typeof parsed.error === "string" && parsed.error.trim()) return parsed.error;
  } catch {
    // 非 JSON 原样返回
  }
  return raw;
}

export interface Handshake {
  port: number;
  token: string;
}

export interface SessionSummary {
  id: string;
  status: string;
  model: string;
  /** 自动生成对话标题（v1.58）；空 / 缺省回退模型名 / 短 id。 */
  title?: string;
  /** 会话级受管 worktree 路径（v1.87）；空 = 项目主根会话。 */
  worktree_path?: string;
  updated_at: string;
  /** v1.148 §15：最新一次 subtasks 快照完成计数；无清单 / 旧 daemon 缺省。 */
  subtasks?: { done: number; total: number } | null;
}

/** v1.147（§9.1）：发送消息队列条目——运行态入队的待发消息（GET /session/:id `queue`）。 */
export interface QueuedMessage {
  id: string;
  text: string;
}

export interface ProjectSummary {
  id: string;
  path: string;
  display_name: string;
  trusted: boolean;
  sessions: SessionSummary[];
  /** 手动归档会话（v1.103 §15）：侧栏默认隐藏，「已归档」折叠组数据源。 */
  archived_sessions?: SessionSummary[];
  /** 有活跃 runtime 的会话 id（§6.2：runtime 不跨 daemon 重启）。 */
  session_runtimes?: string[];
  active_sessions: number;
  dirty_buffers: number;
  /** v1.129：cached_tokens / duration_ms 缺省兼容旧 daemon（命中率与均速派生自二者）。 */
  usage: {
    input_tokens: number;
    output_tokens: number;
    cached_tokens?: number;
    duration_ms?: number;
    cost_usd: number;
  };
}

/** 项目级 UI 状态（§7.2 / §7.5：布局按项目记忆）。 */
export interface ProjectUiState {
  sessionId?: string;
  tabs?: string[];
  activePath?: string | null;
  splitPath?: string | null;
  leftWidth?: number;
  rightWidth?: number;
  bottomHeight?: number;
  sidebarOpen?: boolean;
  timelineOpen?: boolean;
  bottomTab?: "source" | "timeline" | "trace" | "evals";
}

export interface OpenedProject {
  id: string;
  path: string;
  display_name: string;
  trusted: boolean;
}

export interface SearchHit {
  path: string;
  line: number;
  column: number;
  text: string;
}

export interface ReplacePreview {
  path: string;
  diff: string;
}

export interface SearchReplaceResult {
  path: string;
  replacements: number;
  diff: string;
}

export interface SearchReplaceSkipped {
  path: string;
  reason: string;
}

export interface LspSymbol {
  name: string;
  path: string;
  line: number;
  column: number;
  detail?: string;
}

export type FileOperation =
  | { op: "rename"; from: string; to: string }
  | { op: "move"; from: string; to: string }
  | { op: "delete"; path: string };

/** UI 偏好（§7.5 外观档等）：daemon 权威存储（跨启动 / 跨端）。 */
export type UiPrefs = Record<string, string>;

/** provider 设置条目（§15 /settings models 合并视图；api_key 明文永不回显）。 */
export interface ProviderSettings {
  kind?: string;
  base_url?: string;
  wire_api?: string;
  model?: string;
  api_key_env?: string;
  /** 该 provider 在设置覆盖表中（可从设置删除；纯配置文件条目只能编辑）。 */
  overridden?: boolean;
  [k: string]: unknown;
}

/** 权限高级策略（v1.89）：工具黑名单与成本上限收窄；无审批。 */
export interface TeamPolicySettings {
  denied_tools?: string[];
  max_cost_usd?: number | null;
  [k: string]: unknown;
}

/** 全局设置（§15 /settings 合并视图）。 */
export interface SettingsData {
  session: {
    first_edit_buffer_ms?: number;
    [k: string]: unknown;
  };
  exec?: { command_timeout_s?: number; [k: string]: unknown };
  models?: {
    default?: string;
    providers?: Record<string, ProviderSettings>;
    laya?: Record<string, unknown>;
    [k: string]: unknown;
  };
  /** 代理技能（§13.4 v1.130）：停用名单（新会话生效）。 */
  skills?: {
    disabled?: string[];
    [k: string]: unknown;
  };
  /** 技能与插件市场（§13.5 v1.145）：市场源。 */
  market?: {
    sources?: string[];
    [k: string]: unknown;
  };
  /** MCP 插件（§13.5 v1.145）：服务器表（新会话生效）。 */
  mcp?: {
    servers?: Record<string, McpServerData>;
    [k: string]: unknown;
  };
  team_policy?: TeamPolicySettings;
  [k: string]: unknown;
}

/** MCP 服务器条目（settings mcp.servers；command 经启动器白名单校验）。 */
export interface McpServerData {
  command: string;
  args?: string[];
  env?: Record<string, string>;
  enabled?: boolean;
  permissions?: string[];
  /** 市场溯源 owner/repo；手动添加为空。 */
  source?: string | null;
  /** 市场条目版本（更新比对；手动添加为空）。 */
  version?: string | null;
  [k: string]: unknown;
}

/** 市场清单条目（marketplace.json；§13.5）。 */
export interface MarketEntryData {
  kind: "skill" | "mcp" | string;
  name: string;
  description?: string;
  source?: string;
  path?: string;
  ref?: string;
  version?: string;
  command?: string;
  args?: string[];
  permissions?: string[];
  [k: string]: unknown;
}

/** 市场清单（marketplace.json 解析视图）。 */
export interface MarketManifestData {
  name?: string;
  entries?: MarketEntryData[];
  [k: string]: unknown;
}

/** 技能行（GET /skills；market 为市场溯源 sidecar，§13.5 v1.145）。 */
export interface SkillRowData {
  name: string;
  display_name: string;
  description: string;
  scope: "global" | "project" | string;
  dir: string;
  enabled: boolean;
  market?: {
    market_source?: string;
    source?: string;
    path?: string;
    ref?: string;
    version?: string | null;
  } | null;
}

export class TenonApi {
  private base: string;
  private token: string;

  constructor(handshake: Handshake) {
    this.base = `http://127.0.0.1:${handshake.port}`;
    this.token = handshake.token;
  }

  private async request<T>(
    path: string,
    init?: RequestInit & { json?: unknown }
  ): Promise<T> {
    const headers: Record<string, string> = {
      "X-Tenon-Token": this.token,
    };
    let body = init?.body;
    if (init?.json !== undefined) {
      headers["Content-Type"] = "application/json";
      body = JSON.stringify(init.json);
    }
    const resp = await fetch(`${this.base}${path}`, { ...init, headers, body });
    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(`API ${resp.status}: ${daemonErrorMessage(text)}`);
    }
    return (await resp.json()) as T;
  }

  /** 读 UI 偏好（§7.5）：失败回退空对象（外观走本地缓存 / 系统档）。 */
  async getUiPrefs(): Promise<UiPrefs> {
    try {
      return await this.request<UiPrefs>("/ui-prefs");
    } catch {
      return {};
    }
  }

  /** 写 UI 偏好（§7.5）：fire-and-forget，失败不影响本地即时生效。 */
  setUiPrefs(prefs: UiPrefs): void {
    void this.request("/ui-prefs", { method: "PUT", json: prefs }).catch(() => {});
  }

  /** 读全局设置（§15 /settings 合并视图）。 */
  getSettings(): Promise<SettingsData> {
    return this.request<SettingsData>("/settings");
  }

  /** 写全局设置（§15）：校验 + 持久化 + 新会话生效；非法值 400。 */
  putSettings(body: Record<string, unknown>): Promise<SettingsData> {
    return this.request<SettingsData>("/settings", { method: "PUT", json: body });
  }

  /** 写权限高级策略（v1.85）：全量原子替换，仅对新会话生效。 */
  putTeamPolicy(body: TeamPolicySettings): Promise<TeamPolicySettings> {
    return this.request<TeamPolicySettings>("/team-policy", { method: "PUT", json: body });
  }

  listProjects() {
    return this.request<{ projects: ProjectSummary[]; config: Record<string, unknown> }>(
      "/projects"
    );
  }

  openProject(path: string, displayName?: string) {
    return this.request<OpenedProject>("/projects/open", {
      method: "POST",
      json: displayName === undefined ? { path } : { path, display_name: displayName },
    });
  }

  /** 目录浏览（v1.133 §15）：列绝对路径直接子目录（只含目录），path 缺省 = 主目录。 */
  listDirs(path?: string) {
    return this.request<{
      path: string;
      parent: string | null;
      entries: Array<{ name: string; path: string }>;
    }>(path ? `/fs/dirs?path=${encodeURIComponent(path)}` : "/fs/dirs");
  }

  removeProject(projectId: string) {
    return this.request<{ removed: boolean; disk_contents_deleted: boolean }>(`/projects/${projectId}`, {
      method: "DELETE",
    });
  }

  /** 重命名已登记项目（v1.153 §15）：空串清除自定义名回退路径末段。 */
  renameProject(projectId: string, displayName: string) {
    return this.request<{ id: string; display_name: string }>(`/projects/${projectId}/rename`, {
      method: "POST",
      json: { display_name: displayName },
    });
  }

  setTrust(projectId: string, trusted: boolean) {
    return this.request<{ ok: boolean }>("/project/trust", {
      method: "PUT",
      json: { project_id: projectId, trusted },
    });
  }

  async projectUiState(projectId: string): Promise<ProjectUiState> {
    try {
      return await this.request<ProjectUiState>(`/project/${projectId}/ui-state`);
    } catch {
      return {};
    }
  }

  saveProjectUiState(projectId: string, state: ProjectUiState): void {
    void this.request(`/project/${projectId}/ui-state`, {
      method: "PUT",
      json: state,
    }).catch(() => {});
  }

  /** v1.92：mode 档位随审批移除已删；v1.87 "managed" = 会话级受管 worktree。 */
  createSession(
    projectId: string,
    provider = "",
    /** v1.87 §9.7："managed" = 会话级受管 worktree，可与主根会话并行执行。 */
    worktree?: "managed"
  ) {
    return this.request<{
      session_id: string;
      project_id: string;
      worktree_path?: string | null;
    }>("/session", {
      method: "POST",
      json: { project_id: projectId, provider, worktree: worktree ?? null },
    });
  }

  /** v1.147（§9.1）：运行态发送转排队——queued=true 时消息已入会话级 FIFO，回合自然完成后自动出队。 */
  sendMessage(sessionId: string, text: string) {
    return this.request<{ accepted: boolean; queued?: boolean; position?: number }>(
      `/session/${sessionId}/message`,
      {
        method: "POST",
        json: { text },
      },
    );
  }

  getSession(sessionId: string) {
    return this.request<{
      session_id: string;
      status: string;
      latest_seq: number;
      outcome: Record<string, unknown> | null;
      /** v1.147 发送消息队列快照（运行态入队的待发消息，多窗口一致）。 */
      queue?: QueuedMessage[];
    }>(`/session/${sessionId}`);
  }

  /** v1.147（§9.1）：移除一条排队消息（编辑 = 移除后重新发送）。 */
  deleteQueuedMessage(sessionId: string, msgId: string) {
    return this.request<{ removed: boolean }>(`/session/${sessionId}/queue/${msgId}`, {
      method: "DELETE",
    });
  }

  control(sessionId: string, action: string, value?: boolean) {
    return this.request<{ ok: boolean }>(`/session/${sessionId}/control`, {
      method: "POST",
      json: { action, value },
    });
  }

  /** 受管 worktree 会话收尾·合并（v1.87）：三方合入项目根；409 = 冲突预览。 */
  mergeWorktreeSession(sessionId: string) {
    return this.request<{
      merged: string[];
      skipped: Array<{ path: string; reason: string }>;
      conflicts: Array<{ path: string }>;
      snapshot_tree?: string;
    }>(`/session/${sessionId}/worktree/merge`, { method: "POST" });
  }

  /** 受管 worktree 会话收尾·丢弃（v1.87）：confirm 必填，不动用户根。 */
  discardWorktreeSession(sessionId: string, confirm: boolean) {
    return this.request<{ discarded: boolean }>(`/session/${sessionId}/worktree/discard`, {
      method: "POST",
      json: { confirm },
    });
  }

  /** 归档会话（v1.103 §15）：侧栏默认隐藏、可还原；运行中 / 未收尾 worktree 409。 */
  archiveSession(sessionId: string) {
    return this.request<{ archived: boolean }>(`/session/${sessionId}/archive`, {
      method: "POST",
    });
  }

  /** 取消归档（v1.103）：恢复侧栏列表。 */
  unarchiveSession(sessionId: string) {
    return this.request<{ unarchived: boolean }>(`/session/${sessionId}/unarchive`, {
      method: "POST",
    });
  }

  /** 删除会话（v1.103 §15）：confirm 必填，事务级联清事件 / 工具调用 / 用量；快照不随删。 */
  deleteSession(sessionId: string, confirm: boolean) {
    return this.request<{ deleted: boolean }>(`/session/${sessionId}`, {
      method: "DELETE",
      json: { confirm },
    });
  }

  trace(sessionId: string, after = 0) {
    return this.request<{
      events: Array<{
        id: number;
        seq: number;
        type: string;
        payload: Record<string, unknown>;
      }>;
      latest_seq: number;
    }>(`/session/${sessionId}/trace?after=${after}`);
  }

  checkpoints(sessionId: string) {
    return this.request<{
      checkpoints: Array<{
        id: string;
        tree: string;
        files: string[];
        created_at: string;
        // v1.111：关联事件 seq（null = 任务级快照）——消息级撤销按 seq 匹配回合首步。
        event_seq: number | null;
      }>;
    }>(`/session/${sessionId}/checkpoints`);
  }

  rollbackCheckpoint(checkpointId: string, truncate = false) {
    return this.request<{ rolled_back: string[] }>(
      `/checkpoint/${checkpointId}/rollback`,
      { method: "POST", json: { granularity: "revert", truncate } }
    );
  }

  tree(projectId: string, path = "") {
    return this.request<{
      entries: Array<{
        path: string;
        name: string;
        kind: "dir" | "file";
        git_status: string;
      }>;
    }>(
      `/project/${projectId}/tree?path=${encodeURIComponent(path)}`
    );
  }

  fuzzyFiles(projectId: string, q: string, limit = 100) {
    return this.request<{
      hits: Array<{
        path: string;
        name: string;
        kind: "dir" | "file";
        git_status: string;
        score: number;
      }>;
      query: string;
    }>(
      `/project/${projectId}/files/fuzzy?q=${encodeURIComponent(q)}&limit=${limit}`
    );
  }

  readFile(projectId: string, path: string) {
    const query = new URLSearchParams({ path });
    return this.request<{
      path: string;
      content: string;
      total_bytes: number;
    }>(`/project/${projectId}/file?${query}`);
  }

  writeFile(projectId: string, path: string, content: string) {
    return this.request<{ ok: boolean; created: boolean }>(`/project/${projectId}/file`, {
      method: "PUT",
      json: { path, content },
    });
  }

  fileOps(projectId: string, ops: FileOperation[]) {
    return this.request<{
      results: Array<{ ok: boolean; outcome?: string; error?: string }>;
    }>(`/project/${projectId}/file/ops`, {
      method: "POST",
      json: { ops },
    });
  }

  l4Stats(projectId: string) {
    return this.request<{
      project_id: string;
      chunks: number;
      status?: { state: string; updated_at: string; error?: string | null };
    }>(`/project/${projectId}/l4/stats`);
  }

  l4Rebuild(projectId: string) {
    return this.request<{ queued: boolean; project_id: string }>(
      `/project/${projectId}/l4/rebuild`,
      { method: "POST" }
    );
  }

  search(projectId: string, q: string) {
    return this.request<{ hits: SearchHit[] }>(
      `/project/${projectId}/search?q=${encodeURIComponent(q)}`
    );
  }

  searchPreview(projectId: string, q: string, replace: string) {
    return this.request<{ previews: ReplacePreview[] }>(
      `/project/${projectId}/search?q=${encodeURIComponent(q)}&replace=${encodeURIComponent(replace)}`
    );
  }

  applySearchReplace(
    projectId: string,
    q: string,
    replacement: string,
    paths: string[]
  ) {
    return this.request<{
      applied: SearchReplaceResult[];
      skipped: SearchReplaceSkipped[];
    }>(`/project/${projectId}/search/replace`, {
      method: "POST",
      json: { q, replacement, paths },
    });
  }

  lsp(body: {
    project_id: string;
    path: string;
    action:
      | "completion"
      | "hover"
      | "definition"
      | "references"
      | "diagnostics"
      | "workspace_symbol"
      | "codeaction"
      | "signature_help"
      | "rename"
      | "format";
    line?: number;
    character?: number;
    extra?: string;
  }) {
    return this.request<{ result: unknown }>(`/project/${body.project_id}/lsp`, {
      method: "POST",
      json: {
        path: body.path,
        action: body.action,
        line: body.line ?? 0,
        character: body.character ?? 0,
        extra: body.extra,
      },
    });
  }

  /** AI ghost text（P2 实验，默认关闭）：仅发送光标前后窗口。 */
  inlineComplete(body: {
    project_id: string;
    session_id: string;
    path: string;
    language?: string;
    prefix: string;
    suffix: string;
  }) {
    return this.request<{ completion: string; provider: string }>(
      `/project/${body.project_id}/inline-complete`,
      {
        method: "POST",
        json: {
          session_id: body.session_id,
          path: body.path,
          language: body.language,
          prefix: body.prefix,
          suffix: body.suffix,
        },
      }
    );
  }

  /** daemon 侧 tree-sitter 基础高亮 token 流（§8.2）。 */
  getHighlights(projectId: string, path: string) {
    return this.request<{
      tokens: Array<{
        kind: string;
        start_line: number;
        start_column: number;
        end_line: number;
        end_column: number;
      }>;
      language: string | null;
      fallback?: boolean;
    }>(`/project/${projectId}/highlight?path=${encodeURIComponent(path)}`);
  }

  /** Git source view：branch / changes / commits / inline blame（§8.1）。 */
  getGitView(projectId: string, path?: string | null) {
    const query = path ? `?path=${encodeURIComponent(path)}` : "";
    return this.request<{
      repository: boolean;
      branch: string | null;
      branches: Array<{ name: string; current: boolean; commit: string; upstream?: string }>;
      changes: Array<{
        path: string;
        old_path?: string;
        index_status: string;
        worktree_status: string;
      }>;
      commits: Array<{
        id: string;
        short_id: string;
        summary: string;
        author: string;
        email: string;
        timestamp: number;
      }>;
      blame?: {
        path: string;
        lines: Array<{
          line: number;
          commit: string;
          author: string;
          email: string;
          timestamp: number;
          summary: string;
          content: string;
        }>;
      } | null;
    }>(`/project/${projectId}/git/view${query}`);
  }

  /** LSP WorkspaceEdit 原子应用：shadow checkpoint + 失败整体回滚（§8.5）。 */
  applyLspEdit(
    projectId: string,
    workspaceEdit: unknown,
    sessionId: string
  ) {
    return this.request<{
      applied: boolean;
      checkpoint_id: string;
      before_tree: string;
      after_tree: string;
      files: Array<{ path: string; diff: string }>;
    }>(`/project/${projectId}/lsp/apply`, {
      method: "POST",
      json: { workspace_edit: workspaceEdit, session_id: sessionId },
    });
  }

  /** AI Evals 报告列表（§18.3 / M3 可视化）。 */
  evalRuns() {
    return this.request<{
      runs: Array<{
        id: string;
        target: string;
        metrics_json: Record<string, unknown>;
        verdict: string;
        created_at: string;
      }>;
    }>("/evals");
  }

  /** 路由建议（§11 轻量启发式 + Laya 集成点 #4）。 */
  modelSuggest(text: string) {
    return this.request<{
      pure_read: boolean;
      source: Record<string, unknown>;
      default: string;
    }>("/model-suggest", { method: "POST", json: { text } });
  }

  /** 会话切换模型（§11：上下文随迁）。 */
  switchSessionModel(sessionId: string, provider: string, model = "") {
    return this.request<{ session_id: string; model: string }>(
      `/session/${sessionId}/model`,
      { method: "POST", json: { provider, model } }
    );
  }

  /** 语言包探测（§8.4 项目感知推荐）。 */
  detectLanguagePacks(projectId: string) {
    return this.request<{
      packs: Array<{
        language: string;
        command: string;
        detected: boolean;
        server_installed: boolean;
        runtime_hint: string | null;
      }>;
    }>(`/project/${projectId}/language-packs`);
  }

  /** 语言包一键安装（v1.89 直执；失败返回错误）。 */
  installLanguagePack(projectId: string, pack: string) {
    return this.request<{
      command?: string;
      installed?: boolean;
    }>(`/project/${projectId}/language-packs/install`, {
      method: "POST",
      json: { pack },
    });
  }

  /** 推送未保存缓冲（§8.6 人机共编：代理写盘前三方合并）。 */
  putBuffer(projectId: string, path: string, dirty: string) {
    return this.request<{ ok: boolean }>(`/project/${projectId}/buffers`, {
      method: "PUT",
      json: { path, dirty },
    });
  }

  /** 保存 / 关闭后解除脏缓冲。 */
  clearBuffer(projectId: string, path: string) {
    return this.request<{ ok: boolean }>(
      `/project/${projectId}/buffers?path=${encodeURIComponent(path)}`,
      { method: "DELETE" }
    );
  }

  models() {
    return this.request<{
      models: Array<{
        name: string;
        default_model: string;
        local: boolean;
        is_default: boolean;
      }>;
      default: string;
      /** Laya 状态（§15：版本 / 已下载 / 设备）。 */
      laya?: {
        enabled?: boolean;
        downloaded?: boolean;
        version?: string | null;
        [k: string]: unknown;
      };
    }>("/models");
  }

  costs(sessionId?: string) {
    const q = sessionId ? `?session=${sessionId}` : "";
    return this.request<Record<string, unknown>>(`/costs${q}`);
  }

  // ---------- 技能与插件市场（§13.5 v1.145：GitHub 市场清单通道） ----------

  /** 市场源清单（settings market.sources）。 */
  listMarketSources() {
    return this.request<{ sources: string[] }>("/market/sources");
  }

  /** 整体替换市场源（daemon 校验 owner/repo 形态）。 */
  putMarketSources(sources: string[]) {
    return this.request<{ sources: string[] }>("/market/sources", {
      method: "PUT",
      json: { sources },
    });
  }

  /** 市场清单（镜像链拉取，daemon 5 分钟缓存）。 */
  getMarketManifest(owner: string, repo: string) {
    return this.request<MarketManifestData>(
      `/market/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}`
    );
  }

  /** 安装市场条目（skill 落全局技能目录；mcp 写 settings mcp.servers）。 */
  marketInstall(source: string, kind: "skill" | "mcp", name: string) {
    return this.request<{
      installed?: boolean;
      updated?: boolean;
      kind?: string;
      name?: string;
      files?: number;
      note?: string;
      error?: string;
    }>("/market/install", { method: "POST", json: { source, kind, name } });
  }

  /** 卸载市场条目（skill 仅市场 sidecar 条目可卸）。 */
  marketUninstall(kind: "skill" | "mcp", name: string) {
    return this.request<{
      uninstalled?: boolean;
      note?: string;
      error?: string;
    }>("/market/uninstall", { method: "POST", json: { kind, name } });
  }

  /** 换取一次性 WS 票据（§12.6）。 */
  async wsTicket(): Promise<string> {
    const r = await this.request<{ ticket: string; expires_in_s: number }>(
      "/ws-ticket",
      { method: "POST" }
    );
    return r.ticket;
  }

  /** 代理技能合并清单（§13.4 / §15 v1.130）：项目同名覆盖全局后的生效集。 */
  listSkills(projectId?: string) {
    const q = projectId ? `?project=${encodeURIComponent(projectId)}` : "";
    return this.request<{
      skills: Array<{
        name: string;
        display_name: string;
        description: string;
        scope: "global" | "project";
        dir: string;
        enabled: boolean;
      }>;
    }>(`/skills${q}`);
  }

  /** 读 SKILL.md 原文（§13.4：设置面板编辑器数据源）。 */
  getSkill(name: string, projectId?: string) {
    const q = projectId ? `?project=${encodeURIComponent(projectId)}` : "";
    return this.request<{
      name: string;
      scope: "global" | "project";
      path: string;
      content: string;
    }>(`/skills/${encodeURIComponent(name)}${q}`);
  }

  /** 新建全局技能（§13.4：目录名即 id；项目技能经项目文件 API 创建）。 */
  createSkill(name: string, content: string) {
    return this.request<{ ok: boolean; name: string }>("/skills", {
      method: "POST",
      json: { name, content },
    });
  }

  /** 写全局技能原文（§13.4）。 */
  updateSkill(name: string, content: string) {
    return this.request<{ ok: boolean; name: string }>(
      `/skills/${encodeURIComponent(name)}`,
      { method: "PUT", json: { content } }
    );
  }

  /** 删除全局技能（§13.4：项目技能删除走项目文件操作）。 */
  deleteSkill(name: string) {
    return this.request<{ ok: boolean; name: string }>(
      `/skills/${encodeURIComponent(name)}`,
      { method: "DELETE" }
    );
  }

  /** 连接事件流：先换票；等待 auth ok 后才交给调用方（ADR-10）。 */
  async connectEvents(
    onEvent: (ev: unknown) => void,
    projectId?: string
  ): Promise<WebSocket> {
    const ticket = await this.wsTicket();
    const filter = projectId ? `?project_id=${encodeURIComponent(projectId)}` : "";
    const ws = new WebSocket(
      `ws://127.0.0.1:${new URL(this.base).port}/ws${filter}`
    );
    await new Promise<void>((resolve, reject) => {
      ws.onopen = () => ws.send(ticket);
      ws.onerror = () => reject(new Error("ws connect failed"));
      ws.onmessage = (msg) => {
        if (msg.data === "auth ok") {
          ws.onmessage = (event) => {
            try {
              onEvent(JSON.parse(event.data));
            } catch {
              // 忽略非 JSON 帧
            }
          };
          resolve();
        } else if (msg.data === "auth failed") {
          reject(new Error("ws auth failed"));
        }
      };
    });
    return ws;
  }
}
