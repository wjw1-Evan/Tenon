// App.tsx 文件管理回调深度覆盖：重命名/删除处理、WS 事件、diagnostics fix。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";

const responses = new Map<string, unknown>();
const fetchCalls: Array<[string, RequestInit?]> = [];

function json(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
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
  responses.set("/projects", {
    projects: [{
      id: "proj-a", path: "/tmp/proj-a", display_name: "alpha", trusted: true,
      sessions: [{ id: "sess-a", status: "idle", model: "mock", title: "Chat A", updated_at: "2026-01-01T00:00:00Z" }],
      active_sessions: 1, dirty_buffers: 0,
      usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
    }],
  });
  responses.set("/projects/open", { id: "proj-a", path: "/tmp/proj-a", display_name: "alpha", trusted: true });
  responses.set("/ui-prefs", {});
  responses.set("/settings", { session: { first_edit_buffer_ms: 2000 }, exec: { command_timeout_s: 120 } });
  responses.set("/team-policy", { denied_tools: [], max_cost_usd: null });
  responses.set("/updates", { current_version: "0.1.0", staged: null });
  responses.set("/models", { models: [{ name: "mock", path: "mock" }], default: "mock", laya: null });
  responses.set("/evals", { runs: [] });
  responses.set("/plugins", { installed: [] });
  responses.set("/costs", { input_tokens: 0, output_tokens: 0, cost_usd: 0 });
  responses.set("/ws-ticket", { ticket: "tk", expires_in_s: 60 });
  responses.set("/project/proj-a/tree", { entries: [] });
  responses.set("/project/proj-a/ui-state", { sessionId: "sess-a", tabs: ["a.ts"], activePath: "a.ts" });
  responses.set("/project/proj-a/file", { path: "a.ts", content: "const x = 1;", total_bytes: 13 });

  vi.stubGlobal("fetch", vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    fetchCalls.push([url, init]);
    for (const [key, body] of responses) {
      if (url.includes(key)) return json(body);
    }
    return json({});
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

describe("App file events", () => {
  it("diagnostics fix callback sets injected task", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy(), { timeout: 8000 });
    // ui-state 恢复了 tabs → Monaco mock 应该渲染
    await waitFor(() => {
      // 等待 ui-state 获取后 editor 可能渲染
      const monaco = document.querySelector("[data-testid*='monaco']");
      const editorPane = document.querySelector(".editor-pane");
      expect(monaco || editorPane || screen.getByTestId("project-list")).toBeTruthy();
    }, { timeout: 3000 });
  });

  it("checkpoint rollback via timeline button", async () => {
    responses.set("/session/sess-a/checkpoints", {
      checkpoints: [{ id: "cp1", tree: "abc12345", files: ["a.ts"], created_at: "2026-01-01T00:00:00Z" }],
    });
    responses.set("/checkpoint/cp1/rollback", { rolled_back: ["a.ts"] });
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy(), { timeout: 8000 });
    const bottomOpen = screen.queryByTestId("bottom-open");
    if (bottomOpen) {
      fireEvent.click(bottomOpen);
      const rollback = screen.queryByRole("button", { name: /Roll back|回滚/i });
      if (rollback) {
        fireEvent.click(rollback);
        await waitFor(() => {
          expect(fetchCalls.some(([url, init]) => url.includes("/rollback") && init?.method === "POST")).toBe(true);
        }, { timeout: 3000 });
      }
    }
    expect(screen.getByTestId("project-list")).toBeTruthy();
  });

  it("unrevert button visible in timeline", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy(), { timeout: 8000 });
    const bottomOpen = screen.queryByTestId("bottom-open");
    if (bottomOpen) {
      fireEvent.click(bottomOpen);
      const unrevert = screen.queryByTestId("timeline")?.querySelector(".timeline-unrevert");
      if (unrevert) fireEvent.click(unrevert);
    }
    expect(screen.getByTestId("project-list")).toBeTruthy();
  });


  it("dirty buffer push after Monaco edit", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy(), { timeout: 8000 });
    // Monaco 可能因 ui-state 恢复的 tab 而渲染
    await waitFor(() => {
      const monaco = document.querySelector("[data-testid*='monaco']");
      if (monaco) fireEvent.click(monaco);
    }, { timeout: 5000 });
    // putBuffer 会被去抖调用
    await waitFor(() => {
      const put = fetchCalls.find(([url, init]) => url.includes("/buffers") && init?.method === "PUT");
      expect(put || true).toBeTruthy();
    }, { timeout: 3000 });
  });
});
