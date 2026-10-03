// daemon 本地 API 客户端（设计方案 §15）。
// 认证：HTTP 用 X-Tenon-Token；WS 用一次性 ticket（首帧携带，ADR-10）。

export interface Handshake {
  port: number;
  token: string;
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

  registerProject(path: string) {
    return this.request<{ id: string; trusted: boolean }>("/project", {
      method: "PUT",
      json: { path },
    });
  }

  setTrust(projectId: string, trusted: boolean) {
    return this.request<{ ok: boolean }>("/project/trust", {
      method: "PUT",
      json: { project_id: projectId, trusted },
    });
  }

  createSession(projectPath: string, mode: "interactive" | "auto", provider = "") {
    return this.request<{ session_id: string; project_id: string }>("/session", {
      method: "POST",
      json: { project_path: projectPath, mode, provider },
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

  tree(projectId: string) {
    return this.request<{
      entries: Array<{
        path: string;
        name: string;
        kind: "dir" | "file";
        git_status: string;
      }>;
    }>(`/project/${projectId}/tree`);
  }

  readFile(path: string) {
    return this.request<{ path: string; content: string }>(
      `/file?path=${encodeURIComponent(path)}`
    );
  }

  writeFile(path: string, content: string) {
    return this.request<{ ok: boolean; created: boolean }>("/file", {
      method: "PUT",
      json: { path, content },
    });
  }

  search(q: string) {
    return this.request<{
      hits: Array<{ path: string; line: number; column: number; text: string }>;
    }>(`/search?q=${encodeURIComponent(q)}`);
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
  async connectEvents(onEvent: (ev: unknown) => void): Promise<WebSocket> {
    const ticket = await this.wsTicket();
    const ws = new WebSocket(`ws://127.0.0.1:${new URL(this.base).port}/ws`);
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
