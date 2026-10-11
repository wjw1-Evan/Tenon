// v2.12 全局活动视图（design-v2.md §6「命令面板 + 侧栏入口的跨项目任务总览」）契约：
// ① 组件：项目分组（含运行会话项目置顶）、任务行（状态点/标题/相对时间/子任务
//    n/m）、空态、行点击回调、Esc / 遮罩关闭；
// ② App 接线：rail activity 钮与命令面板双入口打开浮层、跨项目行点击触发
//    activateProject（POST /projects/open）+ selectSession + 浮层关闭。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { GlobalActivityDialog } from "../components/GlobalActivityDialog";
import type { ProjectSummary } from "../lib/api";

function usage() {
  return { input_tokens: 0, output_tokens: 0, cost_usd: 0 };
}

function session(id: string, status: string, title: string, extra: Record<string, unknown> = {}) {
  return { id, status, model: "mock", updated_at: "2026-01-01T00:00:00Z", title, ...extra };
}

function project(id: string, name: string, sessions: unknown[], running: boolean): ProjectSummary {
  return {
    id,
    path: `/tmp/${id}`,
    display_name: name,
    trusted: true,
    sessions: sessions as ProjectSummary["sessions"],
    session_runtimes: [],
    active_sessions: running ? 1 : 0,
    dirty_buffers: 0,
    usage: usage(),
  } as ProjectSummary;
}

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (k in Object.fromEntries(backing) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => void backing.clear(),
  });
});

const t = (k: string, vars?: Record<string, string | number>) => {
  let s = k;
  if (vars) for (const [kk, vv] of Object.entries(vars)) s = s.replaceAll(`{${kk}}`, String(vv));
  return s;
};

describe("GlobalActivityDialog 组件（v2.12）", () => {
  const projects = [
    project("p-idle", "idle-project", [session("s-done", "done", "整理文档")], false),
    project("p-busy", "busy-project", [
      session("s-run", "executing", "重构管道", { subtasks: { done: 1, total: 3 } }),
      session("s-old", "done", "修 lint"),
    ], true),
  ];

  it("分组渲染：运行项目置顶、任务行含标题/子任务徽标/状态点、行点击回调", () => {
    const onOpenSession = vi.fn();
    const onClose = vi.fn();
    render(
      <GlobalActivityDialog
        open
        onClose={onClose}
        projects={projects}
        t={t}
        onOpenSession={onOpenSession}
      />
    );
    const groups = screen.getAllByTestId("activity-group");
    expect(groups[0].textContent).toContain("busy-project");
    expect(groups[1].textContent).toContain("idle-project");
    expect(groups[0].textContent).toContain("重构管道");
    expect(groups[0].textContent).toContain("1/3");
    expect(screen.getByTestId("activity-running").textContent).toContain("activity.running_count");
    const rows = screen.getAllByTestId("activity-row");
    expect(rows).toHaveLength(3);
    fireEvent.click(rows[0]);
    expect(onOpenSession).toHaveBeenCalledWith("p-busy", "s-run");
  });

  it("空 projects 渲染空态；Esc 与遮罩点击关闭", () => {
    const onClose = vi.fn();
    const { container } = render(
      <GlobalActivityDialog open onClose={onClose} projects={[]} t={t} onOpenSession={() => {}} />
    );
    expect(screen.getByTestId("global-activity").textContent).toContain("activity.empty");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    fireEvent.click(container.querySelector(".palette-overlay")!);
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("open=false 不渲染", () => {
    render(
      <GlobalActivityDialog open={false} onClose={() => {}} projects={projects} t={t} onOpenSession={() => {}} />
    );
    expect(screen.queryByTestId("global-activity")).toBeNull();
  });
});

describe("全局活动 App 接线（v2.12）", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const method = (init?.method ?? "GET").toUpperCase();
      if (url.endsWith("/projects/open") && method === "POST") {
        return jsonResponse(project("p-busy", "busy-project", [], true));
      }
      if (url.endsWith("/projects")) {
        return jsonResponse({
          projects: [
            project("p-a", "alpha", [session("s-a1", "done", "alpha 任务")], false),
            project("p-busy", "busy-project", [
              session("s-b1", "executing", "busy 运行中", { subtasks: { done: 0, total: 2 } }),
            ], true),
          ],
        });
      }
      if (url.endsWith("/session") && method === "POST")
        return jsonResponse({ session_id: "new-sess", project_id: "?" });
      if (url.includes("/session/s-a1/trace")) return jsonResponse({ events: [] });
      if (url.includes("/session/s-b1/trace")) return jsonResponse({ events: [] });
      if (/\/session\/s-a1$/.test(url))
        return jsonResponse({ session_id: "s-a1", status: "done", latest_seq: 0, outcome: null });
      if (/\/session\/s-b1$/.test(url))
        return jsonResponse({ session_id: "s-b1", status: "executing", latest_seq: 0, outcome: null });
      if (url.includes("/ui-state")) return jsonResponse({});
      if (url.includes("/pairing")) return jsonResponse({ port: 9876, token: "dev" });
      if (url.includes("/ui-prefs")) return jsonResponse({});
      if (url.includes("/settings")) return jsonResponse({});
      if (url.includes("/team-policy")) return jsonResponse({ denied_tools: [], max_cost_usd: null });
      if (url.includes("/updates")) return jsonResponse({ current_version: "0.1.0", staged: null });
      if (url.includes("/models"))
        return jsonResponse({ models: [{ name: "mock", path: "mock" }], default: "mock", laya: null });
      if (url.includes("/evals")) return jsonResponse({ runs: [] });
      if (url.includes("/costs")) return jsonResponse(usage());
      return jsonResponse({});
    });
    vi.stubGlobal("fetch", fetchMock);
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

  function jsonResponse(body: unknown, status = 200) {
    return Promise.resolve(new Response(JSON.stringify(body), { status }));
  }

  it("rail 钮打开浮层；跨项目行点击触发激活（POST /projects/open）并关闭", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/p-a" />);
    await waitFor(() => expect(screen.getByTestId("rail-activity")).toBeTruthy(), { timeout: 5000 });
    fireEvent.click(screen.getByTestId("rail-activity"));
    await waitFor(() => expect(screen.getByTestId("global-activity")).toBeTruthy());
    // 当前项目（p-a）在列 + 跨项目 busy 行可见
    const rows = screen.getAllByTestId("activity-row");
    expect(rows.length).toBeGreaterThanOrEqual(2);
    // 点跨项目行 → 激活 busy（POST /projects/open）+ 浮层关闭
    const cross = rows.find((r) => r.textContent?.includes("busy 运行中"))!;
    fireEvent.click(cross);
    await waitFor(() => expect(screen.queryByTestId("global-activity")).toBeNull());
    await waitFor(() => {
      const open = fetchMock.mock.calls.filter(
        ([u, i]) =>
          String(u).endsWith("/projects/open") &&
          (i as RequestInit | undefined)?.method === "POST"
      );
      expect(open.length).toBeGreaterThan(0);
    });
  });

  it("命令面板含「全局活动」命令且可执行打开", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/p-a" />);
    await waitFor(() => expect(screen.getByTestId("rail-activity")).toBeTruthy(), { timeout: 5000 });
    fireEvent.keyDown(window, { key: "p", metaKey: true, shiftKey: true, ctrlKey: false });
    await waitFor(() => expect(screen.getByTestId("command-palette")).toBeTruthy());
    const input = screen.getByTestId("command-palette").querySelector("input")!;
    fireEvent.change(input, { target: { value: "Global activity" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(screen.getByTestId("global-activity")).toBeTruthy());
  });
});
