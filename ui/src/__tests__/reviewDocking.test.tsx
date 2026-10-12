// v2.4 常驻审查面 App 级接线（design-v2.md §6）：
// ① 会话运行态（executing）→ 审查面自动停靠；
// ② ✕ 手动关闭 → 本次会话不再自动弹（状态仍运行）；
// ③ 空闲（done）→ 不渲染。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import type { ProjectSummary } from "../lib/api";

function usage() {
  return { input_tokens: 0, output_tokens: 0, cost_usd: 0 };
}
function sessionSummary(status: string) {
  return {
    id: "s-a",
    status,
    model: "mock",
    updated_at: "2026-01-01T00:00:00Z",
  };
}
function project(status: string): ProjectSummary {
  return {
    id: "proj-a",
    path: "/tmp/proj-a",
    display_name: "project-a",
    trusted: true,
    sessions: [sessionSummary(status)],
    session_runtimes: ["s-a"],
    active_sessions: 1,
    dirty_buffers: 0,
    usage: usage(),
  };
}

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (k in Object.fromEntries(backing) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => void backing.clear(),
  });
  vi.stubGlobal(
    "fetch",
    vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const method = (init?.method ?? "GET").toUpperCase();
      const body = init?.body ? JSON.parse(String(init.body)) : null;
      if (url.endsWith("/projects/open")) return jsonResponse(project("done"));
      if (url.endsWith("/projects")) return jsonResponse({ projects: [project("done")] });
      if (url.endsWith("/session") && method === "POST")
        return jsonResponse({ session_id: "new-sess", project_id: "?" });
      if (url.includes("/session/s-a/trace")) return jsonResponse({ events: [] });
      if (url.endsWith("/session/s-a"))
        return jsonResponse({ session_id: "s-a", status: currentStatus, latest_seq: 0, outcome: null });
      if (url.endsWith("/session/new-sess"))
        return jsonResponse({ session_id: "new-sess", status: "idle", latest_seq: 0, outcome: null });
      if (url.includes("/ui-state")) return jsonResponse({});
      if (url.includes("/pairing")) return jsonResponse({ port: 9876, token: "dev" });
      if (url.includes("/ui-prefs")) return jsonResponse({});
      if (url.includes("/settings")) return jsonResponse({});
      if (url.includes("/updates")) return jsonResponse({ current_version: "0.1.0", staged: null });
      if (url.includes("/models"))
        return jsonResponse({ models: [{ name: "mock", path: "mock" }], default: "mock", laya: null });
      if (url.includes("/evals")) return jsonResponse({ runs: [] });
      if (url.includes("/costs")) return jsonResponse(usage());
      void body;
      return jsonResponse({});
    })
  );
  class FakeWS {
    onopen: (() => void) | null = null;
    onmessage: ((m: { data: string }) => void) | null = null;
    onclose: (() => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(_url: string) {
      queueMicrotask(() => this.onopen?.());
    }
    send(_data: string) {
      queueMicrotask(() => this.onmessage?.({ data: "auth ok" }));
    }
    close() {}
  }
  vi.stubGlobal("WebSocket", FakeWS);
});

let currentStatus = "done";

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
}

describe("常驻审查面 App 接线（design-v2.md §6）", () => {
  it("running session auto-docks the review pane; close keeps it closed while still running", async () => {
    currentStatus = "executing";
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    // boot 复用运行中会话 s-a → 审查面自动停靠
    await waitFor(() => expect(screen.getByTestId("review-pane")).toBeTruthy(), { timeout: 4000 });
    expect(screen.getByTestId("review-pane").getAttribute("data-running")).toBe("1");
    // 手动关闭：运行中也不再自动弹（reviewManual=false 直到会话切换）
    fireEvent.click(screen.getByTestId("review-close"));
    await waitFor(() => expect(screen.queryByTestId("review-pane")).toBeNull());
    await new Promise((r) => setTimeout(r, 700));
    expect(screen.queryByTestId("review-pane")).toBeNull();
  });

  it("idle session does not render the review pane", async () => {
    currentStatus = "done";
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect((screen.getByTestId("send") as HTMLButtonElement).disabled).toBe(false));
    await new Promise((r) => setTimeout(r, 600));
    expect(screen.queryByTestId("review-pane")).toBeNull();
  });

  it("v2.11 宽度拖拽记忆：左缘把手拖拽调宽并落 localStorage，双击回默认", async () => {
    localStorage.removeItem("tenon:reviewWidth");
    currentStatus = "executing";
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("review-pane")).toBeTruthy(), { timeout: 4000 });
    const handle = screen.getByTestId("resize-review");
    expect(handle).toBeTruthy();
    // 默认 340；拖拽：把手在审查面左缘，鼠标左移（delta -60）→ 增宽到 400
    const pane = () => screen.getByTestId("review-pane");
    expect(pane().style.width).toBe("340px");
    fireEvent.mouseDown(handle, { clientX: 700 });
    fireEvent.mouseMove(window, { clientX: 640 });
    fireEvent.mouseUp(window);
    expect(pane().style.width).toBe("400px");
    expect(localStorage.getItem("tenon:reviewWidth")).toBe("400");
    // 上限 clamp 520：一次大位移拉过界
    fireEvent.mouseDown(handle, { clientX: 640 });
    fireEvent.mouseMove(window, { clientX: 0 });
    fireEvent.mouseUp(window);
    expect(pane().style.width).toBe("520px");
    expect(localStorage.getItem("tenon:reviewWidth")).toBe("520");
    // 双击回默认 340
    fireEvent.dblClick(handle);
    expect(pane().style.width).toBe("340px");
    expect(localStorage.getItem("tenon:reviewWidth")).toBe("340");
  });
});
