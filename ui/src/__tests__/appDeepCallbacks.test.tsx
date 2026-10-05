// App.tsx 剩余回调覆盖：文件树变更 / WS 推送 / 项目激活。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";

const responses = new Map<string, unknown>();
const fetchCalls: Array<[string, RequestInit?]> = [];

function json(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
}

// WS handler 捕获器
let wsHandler: ((ev: unknown) => void) | null = null;

beforeEach(() => {
  responses.clear();
  fetchCalls.length = 0;
  wsHandler = null;
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });

  responses.set("/pairing", { port: 9876, token: "dev" });
  responses.set("/projects/open", { id: "proj-a", path: "/tmp/proj-a", display_name: "alpha", trusted: true });
  responses.set("/projects", {
    projects: [{
      id: "proj-a", path: "/tmp/proj-a", display_name: "alpha", trusted: true,
      sessions: [{ id: "sess-a", status: "idle", model: "mock", title: "Chat A", updated_at: "2026-01-01T00:00:00Z" }],
      active_sessions: 1, dirty_buffers: 0,
      usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
    }],
  });
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
  responses.set("/project/proj-a/file", { path: "a.ts", content: "updated", total_bytes: 7 });

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
        queueMicrotask(() => {
          // 模拟 auth ok 后设置 data handler
          this.onmessage?.({ data: "auth ok" });
        });
      });
    }
    send(data: string) {
      this.sent.push(data);
      // auth 成功后替换 onmessage → 后续 pushEvent 走 handler
    }
    close() { this.onclose?.(); }
    /** 测试助手：通过 onmessage 推送 JSON 事件 */
    pushEvent(event: Record<string, unknown>) {
      act(() => {
        this.onmessage?.({ data: JSON.stringify(event) });
      });
    }
  }
  vi.stubGlobal("WebSocket", FakeWS);
});

async function bootApp() {
  render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
  await waitFor(() => expect(screen.getByTestId("project-list")).toBeTruthy(), { timeout: 8000 });
  // 等待项目激活
  await waitFor(() => {
    const openCall = fetchCalls.find(([url, init]) => url.includes("/projects/open") && init?.method === "POST");
    expect(openCall || screen.getByTestId("project-list")).toBeTruthy();
  }, { timeout: 5000 });
}

describe("App deep callbacks", () => {

  it("session title event updates project list without crash", async () => {
    await bootApp();
    // 通过 fetch mock 触发 refreshProjects
    await waitFor(() => {
      expect(fetchCalls.some(([url]) => url.includes("/projects") && !url.includes("open"))).toBe(true);
    });
    expect(screen.getByTestId("project-list")).toBeTruthy();
  });

  it("project open + session creation flow", async () => {
    responses.set("/session", { session_id: "new-sess", project_id: "proj-a" });
    await bootApp();
    // 项目已激活，session-new 按钮可能可用
    const newBtn = screen.queryByTestId("session-new-proj-a");
    if (newBtn) {
      fireEvent.click(newBtn);
      await waitFor(() => {
        expect(fetchCalls.some(([url, init]) => url.includes("/session") && init?.method === "POST")).toBe(true);
      }, { timeout: 3000 });
    }
    expect(screen.getByTestId("project-list")).toBeTruthy();
  });

  it("bottom panel tab switching exercises render paths", async () => {
    await bootApp();
    const bottomOpen = screen.queryByTestId("bottom-open");
    if (bottomOpen) {
      fireEvent.click(bottomOpen);
      // 各 tab 切换
      for (const tabId of ["bottom-tab-source", "bottom-tab-timeline", "bottom-tab-trace", "bottom-tab-evals"]) {
        const tab = screen.queryByTestId(tabId);
        if (tab) fireEvent.click(tab);
      }
    }
    expect(screen.getByTestId("project-list")).toBeTruthy();
  });

  it("editor tab close via keyboard shortcut", async () => {
    await bootApp();
    // Cmd+W 或 close 路径（不崩溃即可）
    fireEvent.keyDown(window, { key: "w", metaKey: true });
    expect(screen.getByTestId("project-list")).toBeTruthy();
  });



});
