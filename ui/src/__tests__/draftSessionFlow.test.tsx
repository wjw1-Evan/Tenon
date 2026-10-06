// v1.116 新任务草稿态（§7.2）：点「＋ 新任务」不建会话，线程入草稿态；
// 首条消息发出才落库建会话（按意图附受管 worktree）并发送；
// 无标题（未开始）会话不进任务列表（含已归档组），标题生成后才显示。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { ProjectExplorer } from "../components/ProjectExplorer";
import type { ProjectSummary, TenonApi } from "../lib/api";

interface Call {
  url: string;
  method: string;
  body: unknown;
}

const calls: Call[] = [];
let projectPayload: ProjectSummary;

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
}

function sessionSummary(overrides: Partial<ProjectSummary["sessions"][number]> = {}) {
  return {
    id: "sess-x",
    status: "idle",
    model: "mock",
    updated_at: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

beforeEach(() => {
  calls.length = 0;
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });

  // 无标题会话 = 从未发送过首条消息（v1.116 起不进列表、不参与复用）。
  projectPayload = {
    id: "proj-a",
    path: "/tmp/proj-a",
    display_name: "project-a",
    trusted: true,
    sessions: [sessionSummary({ id: "junk-1" })],
    session_runtimes: [],
    active_sessions: 0,
    dirty_buffers: 0,
    usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
  };

  vi.stubGlobal(
    "fetch",
    vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const method = (init?.method ?? "GET").toUpperCase();
      const body = init?.body ? JSON.parse(String(init.body)) : null;
      calls.push({ url, method, body });
      if (url.endsWith("/projects/open")) return jsonResponse(projectPayload);
      if (url.endsWith("/projects")) return jsonResponse({ projects: [projectPayload] });
      if (url.endsWith("/session") && method === "POST") {
        return jsonResponse({ session_id: "new-sess", project_id: "proj-a" });
      }
      if (url.includes("/session/new-sess/message")) return jsonResponse({ accepted: true });
      if (url.includes("/session/new-sess/trace")) return jsonResponse({ events: [] });
      if (url.endsWith("/session/new-sess")) {
        return jsonResponse({ session_id: "new-sess", status: "idle", latest_seq: 0, outcome: null });
      }
      if (url.includes("/ui-state")) return jsonResponse({});
      if (url.includes("/pairing")) return jsonResponse({ port: 9876, token: "dev" });
      if (url.includes("/ui-prefs")) return jsonResponse({});
      if (url.includes("/settings")) {
        return jsonResponse({ session: {}, exec: { command_timeout_s: 120 } });
      }
      if (url.includes("/team-policy")) return jsonResponse({ denied_tools: [], max_cost_usd: null });
      if (url.includes("/updates")) return jsonResponse({ current_version: "0.1.0", staged: null });
      if (url.includes("/models")) {
        return jsonResponse({ models: [{ name: "mock", path: "mock" }], default: "mock", laya: null });
      }
      if (url.includes("/evals")) return jsonResponse({ runs: [] });
      if (url.includes("/plugins")) return jsonResponse({ installed: [] });
      if (url.includes("/portfolio-tasks")) return jsonResponse({ tasks: [] });
      if (url.includes("/costs")) return jsonResponse({ input_tokens: 0, output_tokens: 0, cost_usd: 0 });
      return jsonResponse({});
    })
  );

  class FakeWS {
    static last: FakeWS | null = null;
    onopen: (() => void) | null = null;
    onmessage: ((m: { data: string }) => void) | null = null;
    onclose: (() => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(_url: string) {
      FakeWS.last = this;
      queueMicrotask(() => this.onopen?.());
    }
    send(_data: string) {
      queueMicrotask(() => this.onmessage?.({ data: "auth ok" }));
    }
    close() {}
  }
  vi.stubGlobal("WebSocket", FakeWS);
});

function createCalls() {
  return calls.filter((c) => c.method === "POST" && c.url.endsWith("/session"));
}

describe("draft new-task flow (v1.116)", () => {
  it("boots a never-started project into draft without creating a session", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("task-input")).toBeTruthy());
    // 草稿态：输入与发送可用，但不落库建会话。
    await waitFor(() => {
      expect((screen.getByTestId("send") as HTMLButtonElement).disabled).toBe(false);
    });
    expect(createCalls()).toHaveLength(0);
    // 无标题（未开始）会话不进任务列表。
    expect(screen.queryByTestId("chat-row-junk-1")).toBeNull();
  });

  it("creates the session on first send and routes the message to it", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => {
      expect((screen.getByTestId("send") as HTMLButtonElement).disabled).toBe(false);
    });
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "hello draft" } });
    fireEvent.click(screen.getByTestId("send"));
    await waitFor(() => {
      expect(createCalls()).toHaveLength(1);
      expect(createCalls()[0].body).toEqual({ project_id: "proj-a", provider: "", worktree: null });
    });
    await waitFor(() => {
      const msg = calls.find((c) => c.url.includes("/session/new-sess/message"));
      expect(msg?.body).toEqual({ text: "hello draft" });
    });
  });

  it("keeps worktree intent in the draft and creates a managed worktree session on first send", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => {
      expect((screen.getByTestId("send") as HTMLButtonElement).disabled).toBe(false);
    });
    // 分组行尾分支图标（受管 worktree 新任务，v1.125 由「⎡」换 SVG）→ 草稿记录意图，仍不建会话。
    fireEvent.click(screen.getByTestId("session-new-worktree-proj-a"));
    expect(createCalls()).toHaveLength(0);
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "branch task" } });
    fireEvent.click(screen.getByTestId("send"));
    await waitFor(() => {
      expect(createCalls()).toHaveLength(1);
      expect(createCalls()[0].body).toEqual({ project_id: "proj-a", provider: "", worktree: "managed" });
    });
  });
});

describe("untitled sessions stay out of the task list (v1.116)", () => {
  function renderExplorer(project: ProjectSummary) {
    render(
      <ProjectExplorer
        api={{} as TenonApi}
        t={(key) => key}
        projects={[project]}
        projectId={project.id}
        sessionsByProject={{}}
        openError={null}
        sourceOpen={false}
        onOpenSource={vi.fn()}
        onCloseSource={vi.fn()}
        refreshToken={1}
        onOpenFile={vi.fn()}
        onFileTreeChange={() => {}}
        onSwitchProject={vi.fn()}
        onOpenProject={vi.fn()}
        onRemoveProject={vi.fn()}
        onSelectSession={vi.fn()}
        onCreateSession={vi.fn()}
      />
    );
  }

  it("hides untitled rows and counts only started sessions in the archived group", () => {
    renderExplorer({
      id: "proj-a",
      path: "/tmp/proj-a",
      display_name: "project-a",
      trusted: true,
      sessions: [
        sessionSummary({ id: "t1", title: "修复登录超时" }),
        sessionSummary({ id: "junk-1" }),
      ],
      archived_sessions: [
        sessionSummary({ id: "t2", title: "历史任务", status: "done" }),
        sessionSummary({ id: "junk-2", status: "done" }),
      ],
      active_sessions: 0,
      dirty_buffers: 0,
      usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
    });
    expect(screen.getByTestId("chat-row-t1")).toBeTruthy();
    expect(screen.queryByTestId("chat-row-junk-1")).toBeNull();
    // 已归档组：还原 / 删除操作只含已开始会话（归档行本身无 chat-row testid）。
    fireEvent.click(screen.getByTestId("archived-toggle-proj-a"));
    expect(screen.getByTestId("session-unarchive-t2")).toBeTruthy();
    expect(screen.queryByTestId("session-unarchive-junk-2")).toBeNull();
  });
});
