// 多项目控制面 UI（v1.63 项目文件夹树 + v1.60 登记即用）：切换 / 移除 / 添加均在显式 project_id 上执行。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProjectExplorer } from "../components/ProjectExplorer";
import type { ProjectSummary, TenonApi } from "../lib/api";

const treeMock = vi.fn();

function project(id: string): ProjectSummary {
  return {
    id,
    path: `/tmp/${id}`,
    display_name: id,
    trusted: true,
    sessions: [],
    active_sessions: 0,
    dirty_buffers: 0,
    usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
  };
}

function api(overrides: Partial<TenonApi> = {}) {
  return { tree: treeMock, control: vi.fn().mockResolvedValue({ ok: true }), mergeWorktreeSession: vi.fn().mockResolvedValue({ merged: [], skipped: [], conflicts: [] }), discardWorktreeSession: vi.fn().mockResolvedValue({ discarded: true }), ...overrides } as unknown as TenonApi;
}

function renderExplorer(projects: ProjectSummary[], apiOverrides: Partial<TenonApi> = {}) {
  const onSwitchProject = vi.fn();
  const onOpenProject = vi.fn().mockResolvedValue(undefined);
  const onRemoveProject = vi.fn().mockResolvedValue(undefined);
  const onSelectSession = vi.fn();
  const onCreateSession = vi.fn();
  const apiMock = api(apiOverrides);
  render(
    <ProjectExplorer
      api={apiMock}
      t={(key) => key}
      projects={projects}
      projectId={projects[0]?.id ?? null}
      sessionsByProject={projects[0] ? { [projects[0].id]: "session-1" } : {}}
      portfolioTasks={[]}
      openError={null}
      refreshToken={1}
      onSwitchProject={onSwitchProject}
      onOpenProject={onOpenProject}
      onRemoveProject={onRemoveProject}
      onSelectSession={onSelectSession}
      onCreateSession={onCreateSession}
      onOpenFile={() => {}}
      onFileTreeChange={() => {}}
    />
  );
  return { onSwitchProject, onOpenProject, onRemoveProject, onSelectSession, onCreateSession, api: apiMock };
}

describe("ProjectExplorer multi-project control surface", () => {
  beforeEach(() => {
    // jsdom 环境不提供 localStorage，按仓库约定 stub（tenon:peFiles / tenon:peExpanded 记忆）
    const backing = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
      setItem: (k: string, v: string) => void backing.set(k, v),
      removeItem: (k: string) => void backing.delete(k),
      clear: () => backing.clear(),
    });
    treeMock.mockReset();
    treeMock.mockResolvedValue({ entries: [] });
  });

  afterEach(() => vi.unstubAllGlobals());

  // v1.63：登记项目以文件夹树常驻，无需打开下拉即可见全貌
  it("lists every registered project as persistent folder rows", () => {
    renderExplorer([project("open-a"), project("open-b")]);
    expect(screen.getByTestId("project-list")).toBeInTheDocument();
    expect(screen.getByTestId("project-item-open-a")).toBeInTheDocument();
    expect(screen.getByTestId("project-item-open-b")).toBeInTheDocument();
    // 无下拉面板
    expect(screen.queryByTestId("project-dropdown")).not.toBeInTheDocument();
    expect(screen.queryByTestId("project-switcher")).not.toBeInTheDocument();
  });

  // v1.63：active 项目默认展开，点击其他项目文件夹行即切换并展开
  it("expands the active project and switches by clicking another folder row", () => {
    const { onSwitchProject } = renderExplorer([project("open-a"), project("open-b")]);
    // active 项目默认展开：会话列表可见
    expect(screen.getByTestId("chat-list-open-a")).toBeInTheDocument();
    expect(screen.queryByTestId("chat-list-open-b")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("project-item-open-b"));
    expect(onSwitchProject).toHaveBeenCalledWith(
      expect.objectContaining({ id: "open-b" })
    );
    // 目标文件夹展开且原文件夹保持展开（多项目可同时展开）
    expect(screen.getByTestId("chat-list-open-b")).toBeInTheDocument();
    expect(screen.getByTestId("chat-list-open-a")).toBeInTheDocument();
  });

  // v1.63：已展开的 active 项目再点仅收起，不改激活
  it("collapses an expanded active folder without deactivating it", () => {
    const { onSwitchProject } = renderExplorer([project("open-a")]);
    expect(screen.getByTestId("chat-list-open-a")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("project-item-open-a"));
    expect(screen.queryByTestId("chat-list-open-a")).not.toBeInTheDocument();
    expect(onSwitchProject).not.toHaveBeenCalled();
    // 展开状态记忆到 localStorage
    expect(JSON.parse(localStorage.getItem("tenon:peExpanded") ?? "[]")).toEqual([]);
  });

  it("cannot remove a project that still owns persisted sessions", () => {
    const historical = {
      ...project("historical"),
      sessions: [{ id: "s1", status: "done", model: "mock", updated_at: "now" }],
    };
    renderExplorer([historical]);
    expect(screen.getByTestId(`project-remove-${historical.id}`)).toBeDisabled();
  });

  it("adds another concurrently open project by absolute path", async () => {
    const { onOpenProject } = renderExplorer([project("active")]);
    fireEvent.click(screen.getByTestId("project-add"));
    // 模态对话框出现（§6.4：路径 + 项目名）
    expect(screen.getByRole("dialog", { name: "projects.add_title" })).toBeInTheDocument();
    fireEvent.change(screen.getByTestId("project-add-path"), {
      target: { value: "/tmp/second" },
    });
    // 未手动改名 → 名称自动取路径末段
    await waitFor(() =>
      expect(screen.getByTestId("project-add-name")).toHaveValue("second")
    );
    fireEvent.click(screen.getByRole("button", { name: "projects.open" }));
    await waitFor(() =>
      expect(onOpenProject).toHaveBeenCalledWith("/tmp/second", "second")
    );
    // 成功后模态关闭
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    );
  });

  it("opens the add-project modal and submits a custom display name", async () => {
    const { onOpenProject } = renderExplorer([project("active")]);
    fireEvent.click(screen.getByTestId("project-add"));
    fireEvent.change(screen.getByTestId("project-add-path"), {
      target: { value: "/tmp/second" },
    });
    // 手动命名后不被路径变更覆盖，提交携带显式名称
    fireEvent.change(screen.getByTestId("project-add-name"), {
      target: { value: "自定义名" },
    });
    fireEvent.change(screen.getByTestId("project-add-path"), {
      target: { value: "/tmp/third" },
    });
    expect(screen.getByTestId("project-add-name")).toHaveValue("自定义名");
    fireEvent.click(screen.getByRole("button", { name: "projects.open" }));
    await waitFor(() =>
      expect(onOpenProject).toHaveBeenCalledWith("/tmp/third", "自定义名")
    );
  });

  it("closes the add-project modal on cancel without opening", async () => {
    const { onOpenProject } = renderExplorer([project("active")]);
    fireEvent.click(screen.getByTestId("project-add"));
    expect(screen.getByTestId("project-add-form")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("project-add-cancel"));
    expect(screen.queryByTestId("project-add-form")).not.toBeInTheDocument();
    expect(onOpenProject).not.toHaveBeenCalled();
  });

  it("esc closes the add-project modal", () => {
    renderExplorer([project("active")]);
    fireEvent.click(screen.getByTestId("project-add"));
    expect(screen.getByTestId("project-add-form")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("project-add-form")).not.toBeInTheDocument();
  });

  // v1.70：「源码」按钮切换该行下内嵌的该项目文件树，各项目独立展开并记忆
  it("toggles a per-project inline file tree from the source button", async () => {
    renderExplorer([project("open-a"), project("open-b")]);
    expect(screen.queryByTestId("file-tree")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("project-files-open-a"));
    await waitFor(() => expect(screen.getByTestId("file-tree")).toBeInTheDocument());
    expect(JSON.parse(localStorage.getItem("tenon:peFiles") ?? "[]")).toContain("open-a");
    // 各项目独立展开，互不影响
    fireEvent.click(screen.getByTestId("project-files-open-b"));
    await waitFor(() => expect(screen.getAllByTestId("file-tree")).toHaveLength(2));
    fireEvent.click(screen.getByTestId("project-files-open-a"));
    await waitFor(() => expect(screen.getAllByTestId("file-tree")).toHaveLength(1));
    expect(JSON.parse(localStorage.getItem("tenon:peFiles") ?? "[]")).not.toContain(
      "open-a"
    );
  });

  // v1.58 对话标题：对话行标题优先，无标题回退模型名。
  it("chat rows prefer generated titles and fall back to model names", () => {
    const titled = {
      ...project("open-a"),
      sessions: [
        {
          id: "session-1",
          status: "idle",
          model: "mock",
          title: "修复登录超时",
          updated_at: "now",
        },
        { id: "session-2", status: "done", model: "mock", updated_at: "now" },
      ],
    };
    renderExplorer([titled]);
    expect(screen.getByTestId("chat-list-open-a")).toBeInTheDocument();
    expect(screen.getByText("修复登录超时")).toBeInTheDocument();
    expect(screen.getByText("mock")).toBeInTheDocument();
  });

  // v1.88 Codex projects sidebar：项目名 / 路径 / 会话标题即时索引过滤。
  it("filters the project index by project and session text", () => {
    const matching = {
      ...project("open-a"),
      sessions: [
        { id: "s-a", status: "idle", model: "mock", title: "payment retry", updated_at: "now" },
      ],
    };
    renderExplorer([matching, project("open-b")]);
    fireEvent.change(screen.getByTestId("project-search"), {
      target: { value: "payment" },
    });
    expect(screen.getByTestId("project-item-open-a")).toBeInTheDocument();
    expect(screen.queryByTestId("project-item-open-b")).not.toBeInTheDocument();
    expect(screen.queryByText("projects.no_matches")).not.toBeInTheDocument();
    fireEvent.change(screen.getByTestId("project-search"), {
      target: { value: "does-not-exist" },
    });
    expect(screen.getByText("projects.no_matches")).toBeInTheDocument();
  });

  // v1.88 Codex 项目行：Updated 列取最近会话，支持侧栏快速扫读。
  it("shows the latest session timestamp in the Updated column", () => {
    const dated = {
      ...project("open-a"),
      sessions: [
        { id: "s-old", status: "done", model: "mock", updated_at: "2026-10-04T00:00:00Z" },
        { id: "s-new", status: "idle", model: "mock", updated_at: "2026-10-05T00:00:00Z" },
      ],
    };
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-05T01:00:00Z"));
    try {
      renderExplorer([dated]);
      const row = screen.getByTestId("project-item-open-a");
      expect(row).toHaveTextContent("relative.hours_ago");
    } finally {
      vi.useRealTimers();
    }
  });

  // v1.88 Codex 展开语义：默认最近 10 条，显式开关后才物化完整列表。
  it("materializes the ten most recent sessions first and expands all explicitly", () => {
    const sessions = Array.from({ length: 13 }, (_, index) => ({
      id: `s-${String(index).padStart(2, "0")}`,
      status: "idle",
      model: "mock",
      title: `Session ${index}`,
      updated_at: `2026-10-05T00:${String(12 - index).padStart(2, "0")}:00Z`,
    }));
    renderExplorer([{ ...project("open-a"), sessions }]);
    expect(screen.getAllByTestId(/^chat-row-s-/)).toHaveLength(10);
    expect(screen.queryByText("Session 12")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("session-show-all-open-a"));
    expect(screen.getAllByTestId(/^chat-row-s-/)).toHaveLength(13);
    expect(screen.getByText("Session 12")).toBeInTheDocument();
  });
});

// ---------- 全局活动条（v1.87 §7.2，参考 Codex 侧栏线程流） ----------

function projectWithSessions(
  id: string,
  sessions: Array<Partial<ProjectSummary["sessions"][number]> & { id: string }>
): ProjectSummary {
  return {
    ...project(id),
    sessions: sessions.map((session) => ({
      status: "idle",
      model: "mock",
      updated_at: "2026-10-05T00:00:00Z",
      ...session,
    })),
  };
}

describe("Global activity strip (v1.87 multi-project monitoring)", () => {
  beforeEach(() => {
    const backing = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
      setItem: (k: string, v: string) => void backing.set(k, v),
      removeItem: (k: string) => void backing.delete(k),
      clear: () => backing.clear(),
    });
    treeMock.mockReset();
    treeMock.mockResolvedValue({ entries: [] });
  });

  afterEach(() => vi.unstubAllGlobals());

  it("aggregates cross-project counts and flattens sessions on expand", () => {
    const a = projectWithSessions("proj-a", [
      { id: "s-a1", status: "executing", updated_at: "2026-10-05T01:00:00Z" },
      { id: "s-a2", status: "done" },
    ]);
    const b = projectWithSessions("proj-b", [
      { id: "s-b1", status: "idle" },
    ]);
    renderExplorer([a, b]);
    const bar = screen.getByTestId("global-activity-bar");
    expect(bar).toHaveTextContent("activity.running 1");
    expect(bar).toHaveTextContent("activity.done 1");
    fireEvent.click(bar);
    const list = screen.getByTestId("global-activity-list");
    // 跨项目平铺：三个会话全部可见，无需展开项目文件夹
    expect(list).toHaveTextContent("proj-a");
    expect(list).toHaveTextContent("proj-b");
  });

  it("jumps to the target project and session from a global row", () => {
    const a = projectWithSessions("proj-a", [{ id: "s-a1", status: "idle" }]);
    const b = projectWithSessions("proj-b", [
      { id: "s-b1", status: "idle", updated_at: "2026-10-05T02:00:00Z" },
    ]);
    const { onSwitchProject, onSelectSession } = renderExplorer([a, b]);
    fireEvent.click(screen.getByTestId("global-activity-bar"));
    fireEvent.click(screen.getAllByRole("button", { name: /proj-b/ })[0]);
    expect(onSwitchProject).toHaveBeenCalledWith(expect.objectContaining({ id: "proj-b" }));
    expect(onSelectSession).toHaveBeenCalledWith("proj-b", "s-b1");
  });

  it("filters running rows and exposes in-place stop", async () => {
    const a = projectWithSessions("proj-a", [
      { id: "s-run", status: "executing" },
      { id: "s-idle", status: "idle" },
    ]);
    const { api: apiMock } = renderExplorer([a]);
    fireEvent.click(screen.getByTestId("global-activity-bar"));
    fireEvent.click(screen.getByRole("button", { name: "activity.filter.running" }));
    const stop = screen.getAllByRole("button", { name: "activity.stop" })[0];
    fireEvent.click(stop);
    await waitFor(() => expect(apiMock.control).toHaveBeenCalledWith("s-run", "stop"));
  });

  it("creates plain and managed worktree sessions from per-project entries", () => {
    const a = projectWithSessions("proj-a", [{ id: "s-a1", status: "idle" }]);
    const { onCreateSession } = renderExplorer([a]);
    fireEvent.click(screen.getByTestId(`session-new-proj-a`));
    expect(onCreateSession).toHaveBeenCalledWith(
      expect.objectContaining({ id: "proj-a" }),
      false
    );
    fireEvent.click(screen.getByTestId(`session-new-worktree-proj-a`));
    expect(onCreateSession).toHaveBeenCalledWith(
      expect.objectContaining({ id: "proj-a" }),
      true
    );
  });

  it("offers merge / discard actions on managed worktree sessions only", async () => {
    const a = projectWithSessions("proj-a", [
      { id: "s-wt", status: "done", worktree_path: "/tmp/wt/s-wt" },
      { id: "s-root", status: "done" },
    ]);
    const { api: apiMock } = renderExplorer([a]);
    // 全局列表标题带 ⎇ 标记（受管 worktree）
    fireEvent.click(screen.getByTestId("global-activity-bar"));
    expect(screen.getAllByText(/⎇/).length).toBeGreaterThan(0);
    // 会话行 hover 操作：仅受管会话出现合并 / 丢弃
    const merge = screen.getByRole("button", { name: "projects.worktree_merge" });
    fireEvent.click(merge);
    await waitFor(() =>
      expect(apiMock.mergeWorktreeSession).toHaveBeenCalledWith("s-wt")
    );
  });
});
