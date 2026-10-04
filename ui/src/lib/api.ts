// daemon 本地 API 客户端（设计方案 §15）。
// 认证：HTTP 用 X-Tenon-Token；WS 用一次性 ticket（首帧携带，ADR-10）。

export interface Handshake {
  port: number;
  token: string;
}

export interface ProjectSummary {
  id: string;
  path: string;
  display_name: string;
  trusted: boolean;
  open: boolean;
  sessions: Array<{ id: string; status: string; model: string; updated_at: string }>;
  active_sessions: number;
  dirty_buffers: number;
  pending_approvals: Array<{
    id: string;
    session_id: string;
    action: string;
    level: string;
    created_at: string;
  }>;
  usage: { input_tokens: number; output_tokens: number; cost_usd: number };
}

/** 项目级 UI 状态（§7.2 / §7.5：布局按项目记忆）。 */
export interface ProjectUiState {
  sessionId?: string;
  tabs?: string[];
  activePath?: string | null;
  leftWidth?: number;
  rightWidth?: number;
  bottomHeight?: number;
  sidebarOpen?: boolean;
  timelineOpen?: boolean;
  bottomTab?: "timeline" | "trace" | "evals";
}

export interface OpenedProject {
  id: string;
  path: string;
  display_name: string;
  trusted: boolean;
  open: boolean;
}

export interface PortfolioTask {
  id: string;
  title: string;
  status: string;
  created_at: string;
  updated_at: string;
  children: Array<{
    id: string;
    project_id: string;
    session_id: string;
    text: string;
    status: string;
  }>;
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

/** 全局设置（§15 /settings 合并视图）。 */
export interface SettingsData {
  session: {
    mode?: string;
    first_edit_buffer_ms?: number;
    approval_timeout_s?: number;
    [k: string]: unknown;
  };
  exec?: { command_timeout_s?: number; [k: string]: unknown };
  [k: string]: unknown;
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
      throw new Error(`API ${resp.status}: ${text}`);
    }
    return (await resp.json()) as T;
  }

  health(): Promise<string> {
    return fetch(`${this.base}/health`).then((r) => r.text());
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

  registerProject(path: string) {
    return this.request<{ id: string; trusted: boolean }>("/project", {
      method: "PUT",
      json: { path },
    });
  }

  listProjects() {
    return this.request<{ projects: ProjectSummary[]; config: Record<string, unknown> }>(
      "/projects"
    );
  }

  openProject(path: string) {
    return this.request<OpenedProject>("/projects/open", {
      method: "POST",
      json: { path },
    });
  }

  closeProject(projectId: string, mode: "drain" | "pause" | "force" = "drain") {
    return this.request<{ closed: boolean }>(`/projects/${projectId}/close`, {
      method: "POST",
      json: { mode },
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

  /** mode 空串 = 由 daemon 按全局设置默认档决定（§7.2 / §15）。 */
  createSession(projectId: string, mode: "interactive" | "auto" | "", provider = "") {
    return this.request<{ session_id: string; project_id: string }>("/session", {
      method: "POST",
      json: { project_id: projectId, mode, provider },
    });
  }

  sendMessage(sessionId: string, text: string) {
    return this.request<{ accepted: boolean }>(`/session/${sessionId}/message`, {
      method: "POST",
      json: { text },
    });
  }

  getSession(sessionId: string) {
    return this.request<{
      session_id: string;
      status: string;
      latest_seq: number;
      outcome: Record<string, unknown> | null;
    }>(`/session/${sessionId}`);
  }

  control(sessionId: string, action: string, value?: boolean) {
    return this.request<{ ok: boolean }>(`/session/${sessionId}/control`, {
      method: "POST",
      json: { action, value },
    });
  }

  decideApproval(approvalId: string, decision: "once" | "session" | "deny") {
    return this.request<{ ok: boolean }>(`/approval/${approvalId}`, {
      method: "POST",
      json: { decision },
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
      }>;
    }>(`/session/${sessionId}/checkpoints`);
  }

  rollbackCheckpoint(checkpointId: string) {
    return this.request<{ rolled_back: string[] }>(
      `/checkpoint/${checkpointId}/rollback`,
      { method: "POST", json: { granularity: "revert" } }
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
    return this.request<{ path: string; content: string }>(
      `/project/${projectId}/file?path=${encodeURIComponent(path)}`
    );
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
      | "format";
    line?: number;
    character?: number;
    extra?: string;
  }) {
    return this.request<{ result: unknown }>("/lsp", {
      method: "POST",
      json: {
        project_id: body.project_id,
        path: body.path,
        action: body.action,
        line: body.line ?? 0,
        character: body.character ?? 0,
        extra: body.extra,
      },
    });
  }

  portfolioTasks() {
    return this.request<{ tasks: PortfolioTask[] }>("/portfolio-tasks");
  }

  createPortfolioTask(
    title: string,
    children: Array<{
      project_id: string;
      text: string;
      mode?: "interactive" | "auto";
      working_dir?: string;
    }>,
    provider = ""
  ) {
    return this.request<PortfolioTask>("/portfolio-tasks", {
      method: "POST",
      json: { title, children, provider },
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

  /** 成本归因：月度聚合。 */
  monthlyCosts() {
    return this.request<{ monthly: Array<Record<string, unknown>> }>("/costs");
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

  /** 语言包一键安装（两阶段 D 级审批）。 */
  installLanguagePack(projectId: string, pack: string, approvalId?: string) {
    return this.request<{
      approval_id?: string;
      level?: string;
      command?: string;
      installed?: boolean;
    }>(`/project/${projectId}/language-packs/install`, {
      method: "POST",
      json: { pack, approval_id: approvalId },
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
      laya?: Record<string, unknown>;
    }>("/models");
  }

  costs(sessionId?: string) {
    const q = sessionId ? `?session=${sessionId}` : "";
    return this.request<Record<string, unknown>>(`/costs${q}`);
  }

  /** 换取一次性 WS 票据（§12.6）。 */
  async wsTicket(): Promise<string> {
    const r = await this.request<{ ticket: string; expires_in_s: number }>(
      "/ws-ticket",
      { method: "POST" }
    );
    return r.ticket;
  }

  /** 连接事件流：先换票，首帧携带（ADR-10）。 */
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
      ws.onopen = () => resolve();
      ws.onerror = () => reject(new Error("ws connect failed"));
    });
    ws.send(ticket);
    ws.onmessage = (msg) => {
      if (msg.data === "auth ok" || msg.data === "auth failed") return;
      try {
        onEvent(JSON.parse(msg.data));
      } catch {
        // 忽略非 JSON 帧
      }
    };
    return ws;
  }
}
