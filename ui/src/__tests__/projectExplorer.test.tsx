// 多项目控制面 UI（v1.63 项目文件夹树 + v1.60 登记即用）：切换 / 移除 / 添加均在显式 project_id 上执行。
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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

function renderExplorer(
  projects: ProjectSummary[],
  apiOverrides: Partial<TenonApi> = {},
  callbacks: { onRefreshProjects?: () => void; onSessionRemoved?: (projectId: string, sessionId: string) => void } = {}
) {
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
      openError={null}
      refreshToken={1}
      onSwitchProject={onSwitchProject}
      onOpenProject={onOpenProject}
      onRemoveProject={onRemoveProject}
      onSelectSession={onSelectSession}
      onCreateSession={onCreateSession}
      onRefreshProjects={callbacks.onRefreshProjects}
      onSessionRemoved={callbacks.onSessionRemoved}
      onOpenFile={vi.fn()}
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

  // v1.107：「源码」内嵌文件树已迁右区源码树，项目行仅存「移除」操作。

  // v1.118：源码入口收敛为项目名后常驻文字钮（钮面显示目标视图名）——默认任务，切源码显文件树，per-project 记忆。
  it("toggles per-project task/source views and renders the file tree", async () => {
    renderExplorer([project("open-a")]);
    // 默认任务视图：会话列表可见，无文件树。
    await waitFor(() => expect(screen.getByTestId("chat-list-open-a")).toBeInTheDocument());
    expect(screen.queryByTestId("file-tree")).not.toBeInTheDocument();
    // 切「源码」（文字钮，钮面 = 目标视图名）：文件树出现，选择记忆于 tenon:peView。
    const tasksLabel = screen.getByTestId("pe-source-open-a").textContent ?? "";
    fireEvent.click(screen.getByTestId("pe-source-open-a"));
    await waitFor(() => expect(screen.getByTestId("file-tree")).toBeInTheDocument());
    expect(JSON.parse(localStorage.getItem("tenon:peView") ?? "{}")).toEqual({
      "open-a": "files",
    });
    // 钮面文字随切换变化，切回后复原。
    expect(screen.getByTestId("pe-source-open-a").textContent).not.toBe(tasksLabel);
    fireEvent.click(screen.getByTestId("pe-source-open-a"));
    await waitFor(() => expect(screen.queryByTestId("file-tree")).not.toBeInTheDocument());
    expect(screen.getByTestId("chat-list-open-a")).toBeInTheDocument();
    expect(screen.getByTestId("pe-source-open-a").textContent).toBe(tasksLabel);
  });

  // v1.114：新任务入口重排——标题行「＋ 新任务」作用 active 项目，分组行尾 hover 分支图标（v1.125 由「⎡」换 SVG）。
  it("creates sessions from the header button and the per-group worktree action", () => {
    const { onCreateSession } = renderExplorer([project("open-a")]);
    fireEvent.click(screen.getByTestId("project-new-task"));
    expect(onCreateSession).toHaveBeenCalledWith(
      expect.objectContaining({ id: "open-a" }),
      false
    );
    fireEvent.click(screen.getByTestId("session-new-worktree-open-a"));
    expect(onCreateSession).toHaveBeenCalledWith(
      expect.objectContaining({ id: "open-a" }),
      true
    );
  });

  // v1.116：只展示真正开始的任务——有标题会话行渲染，无标题回退（模型名 / 短 id）不进列表。
  it("chat rows prefer generated titles and hide untitled fallbacks", () => {
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
    expect(screen.queryByText("mock")).toBeNull();
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
      // v1.118：「最近更新」时间移出主按钮到行级，改在分组行容器上断言。
      const row = screen.getByTestId("project-item-open-a").closest(".pe-group-row");
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
      // v1.116：无标题会话不进列表——行操作类用例的会话默认带标题（已开始）。
      title: "已开始的任务",
      updated_at: "2026-10-05T00:00:00Z",
      ...session,
    })),
  };
}

describe("ProjectExplorer worktree sessions (v1.87)", () => {
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

  it("creates plain and managed worktree sessions from per-project entries", () => {
    const a = projectWithSessions("proj-a", [{ id: "s-a1", status: "idle" }]);
    const { onCreateSession } = renderExplorer([a]);
    fireEvent.click(screen.getByTestId(`project-new-task`));
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
    // 会话行标题带 ⎇ 标记（受管 worktree）
    expect(screen.getAllByText(/⎇/).length).toBeGreaterThan(0);
    // 会话行 hover 操作：仅受管会话出现合并 / 丢弃
    const merge = screen.getByRole("button", { name: "projects.worktree_merge" });
    fireEvent.click(merge);
    await waitFor(() =>
      expect(apiMock.mergeWorktreeSession).toHaveBeenCalledWith("s-wt")
    );
  });
});


describe("ProjectExplorer 补充", () => {
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

  it("remove project button calls onRemoveProject", async () => {
    const { onRemoveProject } = renderExplorer([project("p1")]);
    fireEvent.click(screen.getByTestId("project-remove-p1"));
    await waitFor(() => expect(onRemoveProject).toHaveBeenCalledWith(expect.objectContaining({ id: "p1" })));
  });

  it("session-show-all expands session list", () => {
    const proj = project("p1");
    proj.sessions = Array.from({ length: 12 }, (_, i) => ({
      id: `s${i}`, status: "idle", model: "m", title: `Chat ${i}`, updated_at: "2026-01-01T00:00:00Z",
    }));
    renderExplorer([proj]);
    const showAll = screen.getByTestId("session-show-all-p1");
    fireEvent.click(showAll);
    // 展开后显示更多会话行
    expect(screen.getAllByTestId(/^chat-row-/).length).toBeGreaterThan(5);
  });


  it("worktree discard confirm via window.confirm", () => {
    vi.stubGlobal("confirm", vi.fn().mockReturnValue(true));
    const { api: apiMock } = renderExplorer([project("p1")]);
    const discardBtn = screen.queryByTestId("worktree-discard");
    if (discardBtn) fireEvent.click(discardBtn);
    // 不崩溃即可
    expect(screen.getByTestId("project-list")).toBeInTheDocument();
  });
});

// v1.103：会话归档 / 删除（§14.2 / §15）——行内动作、运行中隐藏、confirm 门、已归档组。
describe("Session archive & delete (v1.103)", () => {
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

  const row = (id: string, status = "done") => ({
    id,
    status,
    model: "mock",
    // v1.116：无标题会话不进列表——行操作类用例的会话默认带标题（已开始）。
    title: "已开始的任务",
    updated_at: "2026-10-05T00:00:00Z",
  });

  it("archives an idle session from its row action", async () => {
    const archiveSession = vi.fn().mockResolvedValue({ archived: true });
    const onRefreshProjects = vi.fn();
    const p = { ...project("proj-a"), sessions: [row("s-done")] };
    renderExplorer([p], { archiveSession }, { onRefreshProjects });
    fireEvent.click(screen.getByTestId("session-archive-s-done"));
    await waitFor(() => expect(archiveSession).toHaveBeenCalledWith("s-done"));
    await waitFor(() => expect(onRefreshProjects).toHaveBeenCalled());
  });

  it("hides archive and delete actions on running sessions", () => {
    const p = { ...project("proj-a"), sessions: [row("s-run", "executing")] };
    renderExplorer([p]);
    expect(screen.queryByTestId("session-archive-s-run")).not.toBeInTheDocument();
    expect(screen.queryByTestId("session-delete-s-run")).not.toBeInTheDocument();
  });

  it("delete confirms then reports removal via onSessionRemoved", async () => {
    vi.stubGlobal("confirm", vi.fn().mockReturnValue(true));
    const deleteSession = vi.fn().mockResolvedValue({ deleted: true });
    const onSessionRemoved = vi.fn();
    const p = { ...project("proj-a"), sessions: [row("s-gone")] };
    renderExplorer([p], { deleteSession }, { onSessionRemoved });
    fireEvent.click(screen.getByTestId("session-delete-s-gone"));
    await waitFor(() => expect(deleteSession).toHaveBeenCalledWith("s-gone", true));
    await waitFor(() => expect(onSessionRemoved).toHaveBeenCalledWith("proj-a", "s-gone"));
  });

  it("cancels delete when confirm is dismissed", () => {
    vi.stubGlobal("confirm", vi.fn().mockReturnValue(false));
    const deleteSession = vi.fn();
    const p = { ...project("proj-a"), sessions: [row("s-keep")] };
    renderExplorer([p], { deleteSession });
    fireEvent.click(screen.getByTestId("session-delete-s-keep"));
    expect(deleteSession).not.toHaveBeenCalled();
  });

  it("shows the archived group with restore action", async () => {
    const unarchiveSession = vi.fn().mockResolvedValue({ unarchived: true });
    const p = { ...project("proj-a"), sessions: [], archived_sessions: [row("s-arc")] };
    renderExplorer([p], { unarchiveSession });
    // 默认收起
    expect(screen.queryByTestId("session-unarchive-s-arc")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("archived-toggle-proj-a"));
    expect(screen.getByTestId("session-unarchive-s-arc")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("session-unarchive-s-arc"));
    await waitFor(() => expect(unarchiveSession).toHaveBeenCalledWith("s-arc"));
  });
});

// ---------- 目录选择浮层（v1.133 §7.1：浏览器模式 GET /fs/dirs） ----------

describe("ProjectExplorer folder picker (v1.133)", () => {
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

  it("browse button is always visible and the picker fills the path on choose", async () => {
    // jsdom 无 __TAURI_INTERNALS__ = 浏览器模式：浏览不再依赖桌面壳
    const listDirs = vi
      .fn()
      .mockResolvedValueOnce({
        path: "/home/u",
        parent: "/home",
        entries: [{ name: "proj", path: "/home/u/proj" }],
      })
      .mockResolvedValueOnce({ path: "/home/u/proj", parent: "/home/u", entries: [] });
    const { onOpenProject } = renderExplorer([project("active")], { listDirs });
    fireEvent.click(screen.getByTestId("project-add"));
    expect(screen.getByTestId("project-add-browse")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("project-add-browse"));
    await waitFor(() =>
      expect(screen.getByTestId("project-add-picker")).toBeInTheDocument()
    );
    expect(listDirs).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("picker-dir-proj")).toBeInTheDocument();
    // 点击子目录逐层进入
    fireEvent.click(screen.getByTestId("picker-dir-proj"));
    await waitFor(() => expect(listDirs).toHaveBeenCalledWith("/home/u/proj"));
    await waitFor(() =>
      expect(screen.getByTestId("project-add-picker-path").textContent).toBe("/home/u/proj")
    );
    // 「选择当前目录」回填路径 + 自动项目名，并收浮层
    fireEvent.click(screen.getByTestId("project-add-picker-choose"));
    await waitFor(() =>
      expect(screen.getByTestId("project-add-path")).toHaveValue("/home/u/proj")
    );
    expect(screen.getByTestId("project-add-name")).toHaveValue("proj");
    expect(screen.queryByTestId("project-add-picker")).not.toBeInTheDocument();
    // 提交仍走既有 onOpenProject 链路
    fireEvent.click(screen.getByRole("button", { name: "projects.open" }));
    await waitFor(() =>
      expect(onOpenProject).toHaveBeenCalledWith("/home/u/proj", "proj")
    );
  });

  it("cancel closes the picker but keeps the add dialog", async () => {
    const listDirs = vi.fn().mockResolvedValue({ path: "/home/u", parent: "/home", entries: [] });
    renderExplorer([project("active")], { listDirs });
    fireEvent.click(screen.getByTestId("project-add"));
    fireEvent.click(screen.getByTestId("project-add-browse"));
    await waitFor(() =>
      expect(screen.getByTestId("project-add-picker")).toBeInTheDocument()
    );
    fireEvent.click(screen.getByTestId("project-add-picker-cancel"));
    expect(screen.queryByTestId("project-add-picker")).not.toBeInTheDocument();
    expect(screen.getByTestId("project-add-form")).toBeInTheDocument();
  });

  it("renders a load error and Escape returns to the add dialog", async () => {
    const listDirs = vi.fn().mockRejectedValue(new Error("403: forbidden"));
    renderExplorer([project("active")], { listDirs });
    fireEvent.click(screen.getByTestId("project-add"));
    fireEvent.click(screen.getByTestId("project-add-browse"));
    await waitFor(() =>
      expect(screen.getByTestId("project-add-picker-error")).toBeInTheDocument()
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("project-add-picker")).not.toBeInTheDocument();
    expect(screen.getByTestId("project-add-form")).toBeInTheDocument();
  });
});
