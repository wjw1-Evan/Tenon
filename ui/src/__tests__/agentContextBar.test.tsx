// v1.126 任务上下文条（§7.5）：输入区上方——草稿态项目 / 工作区选择器（多项目归属可见可改，
// 「分支选择」= 主根 / 受管 worktree 意图）、会话态只读归属标识、草稿文本 per-project 隔离、
// 注入类入口（诊断 AI 修复 / 行内指令）主根草稿态走草稿首发链路。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { AgentPanel } from "../components/AgentPanel";
import type { ProjectSummary, TenonApi } from "../lib/api";

interface Call {
  url: string;
  method: string;
  body: unknown;
}

const calls: Call[] = [];

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
}

function usage() {
  return { input_tokens: 0, output_tokens: 0, cost_usd: 0 };
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

/** 双项目：proj-a（boot 项目，仅未开始会话）与 proj-b（变体按用例注入）。 */
let projectA: ProjectSummary;
let projectB: ProjectSummary;

beforeEach(() => {
  calls.length = 0;
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });

  function project(id: string, name: string): ProjectSummary {
    return {
      id,
      path: `/tmp/${id}`,
      display_name: name,
      trusted: true,
      sessions: [sessionSummary({ id: `junk-${id}` })],
      session_runtimes: [],
      active_sessions: 0,
      dirty_buffers: 0,
      usage: usage(),
    };
  }
  projectA = project("proj-a", "project-a");
  projectB = project("proj-b", "project-b");

  vi.stubGlobal(
    "fetch",
    vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const method = (init?.method ?? "GET").toUpperCase();
      const body = init?.body ? JSON.parse(String(init.body)) : null;
      calls.push({ url, method, body });
      if (url.endsWith("/projects/open")) return jsonResponse(projectA);
      if (url.endsWith("/projects")) {
        return jsonResponse({ projects: [projectA, projectB] });
      }
      if (url.endsWith("/session") && method === "POST") {
        return jsonResponse({ session_id: "new-sess", project_id: body?.project_id ?? "?" });
      }
      if (url.includes("/session/new-sess/message")) return jsonResponse({ accepted: true });
      if (url.includes("/session/new-sess/trace")) return jsonResponse({ events: [] });
      if (url.endsWith("/session/new-sess")) {
        return jsonResponse({ session_id: "new-sess", status: "idle", latest_seq: 0, outcome: null });
      }
      if (url.includes("/session/s-a/trace")) return jsonResponse({ events: [] });
      if (url.endsWith("/session/s-a")) {
        return jsonResponse({ session_id: "s-a", status: "idle", latest_seq: 0, outcome: null });
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
      if (url.includes("/costs")) return jsonResponse(usage());
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

function createCalls() {
  return calls.filter((c) => c.method === "POST" && c.url.endsWith("/session"));
}

describe("agent context bar (v1.126) — AgentPanel contract", () => {
  const projects: ProjectSummary[] = [
    {
      id: "proj-a",
      path: "/tmp/proj-a",
      display_name: "project-a",
      trusted: true,
      sessions: [],
      active_sessions: 0,
      dirty_buffers: 0,
      usage: usage(),
    },
    {
      id: "proj-b",
      path: "/tmp/proj-b",
      display_name: "project-b",
      trusted: true,
      sessions: [],
      active_sessions: 0,
      dirty_buffers: 0,
      usage: usage(),
    },
  ];

  function mockApi() {
    return {
      models: () => Promise.resolve({ models: [], default: "", laya: null }),
      trace: () => Promise.resolve({ events: [] }),
      getSession: () => Promise.resolve({ session_id: "s", status: "idle", latest_seq: 0 }),
      sendMessage: () => Promise.resolve({ accepted: true }),
    } as unknown as TenonApi;
  }

  it("shows project + workspace selectors in draft state", () => {
    render(
      <AgentPanel
        api={mockApi()}
        t={(k) => k}
        sessionId={null}
        draft
        draftWorktree={false}
        projects={projects}
        projectId="proj-a"
      />
    );
    const projectSelect = screen.getByTestId("draft-project-select") as HTMLSelectElement;
    expect(projectSelect.value).toBe("proj-a");
    expect(projectSelect.options).toHaveLength(2);
    const workspaceSelect = screen.getByTestId("draft-workspace-select") as HTMLSelectElement;
    expect(workspaceSelect.value).toBe("main");
  });

  it("switching selectors reports intent callbacks", () => {
    const onSwitch = vi.fn();
    const onWorktree = vi.fn();
    render(
      <AgentPanel
        api={mockApi()}
        t={(k) => k}
        sessionId={null}
        draft
        draftWorktree={false}
        projects={projects}
        projectId="proj-a"
        onSwitchDraftProject={onSwitch}
        onChangeDraftWorktree={onWorktree}
      />
    );
    fireEvent.change(screen.getByTestId("draft-project-select"), { target: { value: "proj-b" } });
    expect(onSwitch).toHaveBeenCalledWith(projects[1]);
    fireEvent.change(screen.getByTestId("draft-workspace-select"), { target: { value: "managed" } });
    expect(onWorktree).toHaveBeenCalledWith(true);
  });

  it("draft input text is per-project isolated", () => {
    const { rerender } = render(
      <AgentPanel
        api={mockApi()}
        t={(k) => k}
        sessionId={null}
        draft
        projects={projects}
        projectId="proj-a"
      />
    );
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "task for a" } });
    rerender(
      <AgentPanel
        api={mockApi()}
        t={(k) => k}
        sessionId={null}
        draft
        projects={projects}
        projectId="proj-b"
      />
    );
    expect((screen.getByTestId("task-input") as HTMLTextAreaElement).value).toBe("");
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "task for b" } });
    rerender(
      <AgentPanel
        api={mockApi()}
        t={(k) => k}
        sessionId={null}
        draft
        projects={projects}
        projectId="proj-a"
      />
    );
    expect((screen.getByTestId("task-input") as HTMLTextAreaElement).value).toBe("task for a");
  });

  it("injected task routes through draft send on main-root draft, not on worktree draft", async () => {
    const onDraftSend = vi.fn().mockResolvedValue(undefined);
    const { rerender } = render(
      <AgentPanel
        api={mockApi()}
        t={(k) => k}
        sessionId={null}
        draft
        draftWorktree={false}
        projects={projects}
        projectId="proj-a"
        onDraftSend={onDraftSend}
        injectedTask={{ token: 1, text: "fix the diagnostic" }}
      />
    );
    await waitFor(() => expect(onDraftSend).toHaveBeenCalledWith("fix the diagnostic"));
    // 受管 worktree 草稿不接（主根上下文不注入副本）。
    const onDraftSend2 = vi.fn().mockResolvedValue(undefined);
    rerender(
      <AgentPanel
        api={mockApi()}
        t={(k) => k}
        sessionId={null}
        draft
        draftWorktree
        projects={projects}
        projectId="proj-a"
        onDraftSend={onDraftSend2}
        injectedTask={{ token: 2, text: "should not send" }}
      />
    );
    expect(onDraftSend2).not.toHaveBeenCalled();
  });

  it("session state shows read-only project label and worktree badge", () => {
    render(
      <AgentPanel
        api={mockApi()}
        t={(k) => k}
        sessionId="s1"
        projects={projects}
        projectId="proj-b"
        sessionWorktree
      />
    );
    expect(screen.getByTestId("session-project-label").textContent).toBe("project-b");
    expect(screen.getByTestId("session-worktree-badge")).toBeTruthy();
    expect(screen.queryByTestId("draft-project-select")).toBeNull();
    expect(screen.queryByTestId("draft-workspace-select")).toBeNull();
  });
});

describe("agent context bar (v1.126) — App wiring", () => {
  it("draft bar switches target project; first send creates the session on the switched project", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => {
      expect((screen.getByTestId("send") as HTMLButtonElement).disabled).toBe(false);
    });
    // 草稿态上下文条：项目选择器初值 = boot 项目。
    const projectSelect = screen.getByTestId("draft-project-select") as HTMLSelectElement;
    expect(projectSelect.value).toBe("proj-a");
    // 切到 proj-b（无会话 → 保持草稿），输入文本随项目切换隔离。
    fireEvent.change(projectSelect, { target: { value: "proj-b" } });
    await waitFor(() => expect((screen.getByTestId("draft-project-select") as HTMLSelectElement).value).toBe("proj-b"));
    expect((screen.getByTestId("task-input") as HTMLTextAreaElement).value).toBe("");
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "hello b" } });
    fireEvent.click(screen.getByTestId("send"));
    await waitFor(() => {
      expect(createCalls()).toHaveLength(1);
      expect(createCalls()[0].body).toEqual({ project_id: "proj-b", provider: "", worktree: null });
    });
    await waitFor(() => {
      const msg = calls.find((c) => c.url.includes("/session/new-sess/message"));
      expect(msg?.body).toEqual({ text: "hello b" });
    });
  });

  it("workspace selector flips the draft intent to managed worktree", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => {
      expect((screen.getByTestId("send") as HTMLButtonElement).disabled).toBe(false);
    });
    fireEvent.change(screen.getByTestId("draft-workspace-select"), { target: { value: "managed" } });
    fireEvent.change(screen.getByTestId("task-input"), { target: { value: "branch task" } });
    fireEvent.click(screen.getByTestId("send"));
    await waitFor(() => {
      expect(createCalls()).toHaveLength(1);
      expect(createCalls()[0].body).toEqual({ project_id: "proj-a", provider: "", worktree: "managed" });
    });
  });

  it("session state shows read-only project label with worktree badge", async () => {
    // proj-a 有已开始的受管 worktree 会话 s-a：boot 复用 → 会话态只读上下文。
    projectA = {
      ...projectA,
      sessions: [sessionSummary({ id: "s-a", title: "A task", worktree_path: "/tmp/wt-s-a" })],
      session_runtimes: ["s-a"],
    };
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/proj-a" />);
    await waitFor(() => expect(screen.getByTestId("session-project-label")).toBeTruthy());
    expect(screen.getByTestId("session-project-label").textContent).toBe("project-a");
    expect(screen.getByTestId("session-worktree-badge")).toBeTruthy();
    expect(screen.queryByTestId("draft-project-select")).toBeNull();
  });
});
