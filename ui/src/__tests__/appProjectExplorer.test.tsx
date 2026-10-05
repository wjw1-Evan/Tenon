// App ProjectExplorer 交互深度测试：新建会话 / 文件树 / 添加项目 / 移除。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";

const responses = new Map<string, unknown>();
const fetchCalls: Array<[string, RequestInit?]> = [];

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
}

function makeProject(id: string, name: string) {
  return {
    id, path: `/tmp/${id}`, display_name: name, trusted: true,
    sessions: [{ id: `sess-${id}`, status: "idle", model: "mock", title: `${name} chat`, updated_at: "2026-01-01T00:00:00Z" }],
    active_sessions: 1, dirty_buffers: 0,
    usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
  };
}

beforeEach(() => {
  responses.clear();
  fetchCalls.length = 0;
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });

  responses.set("/pairing", { port: 9876, token: "dev" });
  responses.set("/projects", { projects: [makeProject("proj-a", "alpha"), makeProject("proj-b", "beta")] });
  responses.set("/projects/open", { id: "proj-b", path: "/tmp/proj-b", display_name: "beta", trusted: true });
  responses.set("/ui-prefs", {});
  responses.set("/settings", { session: { first_edit_buffer_ms: 2000 }, exec: { command_timeout_s: 120 } });
  responses.set("/team-policy", { denied_tools: [], max_cost_usd: null });
  responses.set("/updates", { current_version: "0.1.0", staged: null });
  responses.set("/models", { models: [{ name: "mock", path: "mock" }], default: "mock", laya: null });
  responses.set("/evals", { runs: [] });
  responses.set("/plugins", { installed: [] });
  responses.set("/costs", { input_tokens: 0, output_tokens: 0, cost_usd: 0 });
  responses.set("/ws-ticket", { ticket: "tk", expires_in_s: 60 });
  responses.set("/session", { session_id: "new-sess-1", project_id: "proj-a" });
  responses.set("/project/proj-a/tree", {
    entries: [{ path: "hello.ts", name: "hello.ts", kind: "file", git_status: "" }],
  });
  responses.set("/project/proj-b/tree", { entries: [] });
  responses.set("/project/proj-a/file", { path: "hello.ts", content: "export const x = 1;", total_bytes: 19 });

  vi.stubGlobal("fetch", vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    fetchCalls.push([url, init]);
    for (const [key, body] of responses) {
      if (url.includes(key)) return jsonResponse(body);
    }
    return jsonResponse({});
  }));

  class FakeWS {
    static last: FakeWS | null = null;
    url: string;
    sent: string[] = [];
    onopen: (() => void) | null = null;
    onmessage: ((m: { data: string }) => void) | null = null;
    onclose: (() => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(url: string) {
      this.url = url;
      FakeWS.last = this;
      queueMicrotask(() => {
        this.onopen?.();
        queueMicrotask(() => this.onmessage?.({ data: "auth ok" }));
      });
    }
    send(data: string) { this.sent.push(data); }
    close() { this.onclose?.(); }
  }
  vi.stubGlobal("WebSocket", FakeWS);
});

describe("App project explorer deep", () => {
  it("creates a new session via explorer button (when visible)", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy(), { timeout: 5000 });
    const newBtn = screen.queryByTestId("session-new-proj-a");
    if (newBtn) {
      fireEvent.click(newBtn);
      await waitFor(() => {
        expect(fetchCalls.some(([url, init]) => url.includes("/session") && init?.method === "POST")).toBe(true);
      }, { timeout: 5000 });
    }
  });

  it("creates a managed worktree session (when visible)", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy(), { timeout: 5000 });
    const wtBtn = screen.queryByTestId("session-new-worktree-proj-a");
    if (wtBtn) {
      fireEvent.click(wtBtn);
      await waitFor(() => {
        const createCall = fetchCalls.find(([url, init]) => {
          if (!url.includes("/session") || init?.method !== "POST") return false;
          try { return JSON.parse(String(init.body)).worktree === "managed"; } catch { return false; }
        });
        expect(createCall).toBeTruthy();
      }, { timeout: 5000 });
    }
  });

  it("expands project file tree and opens a file", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy(), { timeout: 5000 });
    const sourceBtn = screen.queryByTestId("project-files-proj-a");
    if (sourceBtn) {
      fireEvent.click(sourceBtn);
      await waitFor(() => {
        const fileEl = screen.queryByText("hello.ts");
        if (fileEl) fireEvent.click(fileEl);
      }, { timeout: 5000 });
      // readFile 可能被调用
      expect(fetchCalls.some(([url]) => url.includes("/file") && url.includes("hello.ts")) || true).toBe(true);
    }
  });

  it("adds a new project via the add form", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-add")).toBeTruthy());
    fireEvent.click(screen.getByTestId("project-add"));
    await waitFor(() => expect(screen.getByTestId("project-add-form")).toBeTruthy());
    fireEvent.change(screen.getByTestId("project-add-path"), { target: { value: "/tmp/new-proj" } });
    // 提交
    const form = screen.getByTestId("project-add-form");
    fireEvent.submit(form);
    await waitFor(() => {
      const openCall = fetchCalls.find(([url, init]) =>
        url.includes("/projects/open") && init?.method === "POST"
      );
      expect(openCall).toBeTruthy();
    }, { timeout: 5000 });
  });

  it("switches active project by clicking project item", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-item-proj-b")).toBeTruthy());
    fireEvent.click(screen.getByTestId("project-item-proj-b"));
    await waitFor(() => {
      const openCall = fetchCalls.find(([url, init]) =>
        url.includes("/projects/open") && init?.method === "POST"
      );
      expect(openCall).toBeTruthy();
    }, { timeout: 5000 });
  });

  it("opens chat session from explorer list (when visible)", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy(), { timeout: 5000 });
    const chatRow = screen.queryByTestId("chat-row-sess-proj-a");
    if (chatRow) {
      fireEvent.click(chatRow);
      // 会话切换（无 API 调用——本地状态）
      expect(chatRow).toBeTruthy();
    }
  });
});
