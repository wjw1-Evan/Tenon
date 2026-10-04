// 主工作区（设计方案 §7.2 四区布局）：文件树 | 编辑器 | 代理会话 + 底部时间轴。
// 三区可折叠；快捷键 §7.4。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ResizeHandle } from "./components/ResizeHandle";
import { TenonApi } from "./lib/api";
import type { PortfolioTask, ProjectSummary } from "./lib/api";
import { createTranslator, LOCALE_CHANGE, type Locale, type Translate } from "./lib/i18n";
import {
  applyTheme,
  loadThemePreference,
  saveThemePreference,
  watchSystemTheme,
  type ThemePreference,
} from "./lib/theme";
import type { AgentStateName } from "./lib/stateColors";
import { useShortcuts } from "./hooks";
import { FileTree, type FileTreeChange } from "./components/FileTree";
import { SearchPanel } from "./components/SearchPanel";
import { FileFinder } from "./components/FileFinder";
import { EditorPane, type EditorTab } from "./components/EditorPane";
import { AgentPanel } from "./components/AgentPanel";
import { CheckpointTimeline } from "./components/CheckpointTimeline";
import { DiffPanel } from "./components/DiffPanel";
import { ThreePaneMerge, type DirtyConflict } from "./components/ThreePaneMerge";
import { ModelRoutingPanel } from "./components/ModelRoutingPanel";
import { AgentTracePanel } from "./components/AgentTracePanel";
import { EvalsPanel } from "./components/EvalsPanel";
import { LanguagePackWizard } from "./components/LanguagePackWizard";
import { unionLines } from "./lib/aiLines";
import { createAutoSaver, type AutoSaver } from "./lib/autosave";
import { SettingsDialog, type SettingsData } from "./components/SettingsDialog";
import { CommandPalette, type Command } from "./components/CommandPalette";

/** 侧栏视图（布局 §7.2 重设计）：activity rail 单视图切换，localStorage 记忆。 */
type SideView = "files" | "search" | "packs";
const SIDE_VIEW_KEY = "tenon:sideView";

function loadSideView(): SideView {
  try {
    const raw = localStorage.getItem(SIDE_VIEW_KEY);
    return raw === "search" || raw === "packs" ? raw : "files";
  } catch {
    return "files";
  }
}

/** rail 图标：线性风格，17px 网格。 */
function RailIcon({ view }: { view: SideView }) {
  const props = {
    width: 17,
    height: 17,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.8,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    "aria-hidden": true as const,
  };
  if (view === "files") {
    return (
      <svg {...props}>
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
        <path d="M14 2v6h6" />
      </svg>
    );
  }
  if (view === "search") {
    return (
      <svg {...props}>
        <circle cx="11" cy="11" r="7" />
        <path d="m21 21-4.3-4.3" />
      </svg>
    );
  }
  return (
    <svg {...props}>
      <rect x="3" y="3" width="7.5" height="7.5" rx="1.5" />
      <rect x="13.5" y="3" width="7.5" height="7.5" rx="1.5" />
      <rect x="3" y="13.5" width="7.5" height="7.5" rx="1.5" />
      <rect x="13.5" y="13.5" width="7.5" height="7.5" rx="1.5" />
    </svg>
  );
}

export default function App({
  handshake,
  projectPath,
  locale = "auto",
}: {
  handshake: { port: number; token: string };
  projectPath: string;
  locale?: Locale;
}) {
  const t = useMemo(() => createTranslator(locale), [locale]);
  const api = useMemo(() => new TenonApi(handshake), [handshake]);

  const [projectId, setProjectId] = useState<string | null>(null);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [portfolioTasks, setPortfolioTasks] = useState<PortfolioTask[]>([]);
  const [sessionsByProject, setSessionsByProject] = useState<Record<string, string>>({});
  const [tabsByProject, setTabsByProject] = useState<Record<string, EditorTab[]>>({});
  const [activePathByProject, setActivePathByProject] = useState<Record<string, string | null>>({});
  const [openPath, setOpenPath] = useState(projectPath);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [finderOpen, setFinderOpen] = useState(false);
  const [timelineOpen, setTimelineOpen] = useState(true);
  const [latestDiff, setLatestDiff] = useState<string | null>(null);
  const [dirtyConflict, setDirtyConflict] = useState<DirtyConflict | null>(null);
  const [aiLines, setAiLines] = useState<Record<string, number[]>>({});
  /** ProjectRuntime 文件事件版本：驱动文件树增量刷新与打开缓冲同步（§6.4 / §8.1）。 */
  const [fileTreeVersion, setFileTreeVersion] = useState(0);
  const [gotoLine, setGotoLine] = useState<{ path: string; line: number; token: number } | null>(
    null
  );
  const projectIdRef = useRef<string | null>(null);
  const projectUiStateLoaded = useRef<Set<string>>(new Set());
  const dirtyTimers = useRef<Map<string, number>>(new Map());
  const tabsByProjectRef = useRef<Record<string, EditorTab[]>>({});
  const activePathByProjectRef = useRef<Record<string, string | null>>({});
  const unsavedRef = useRef<Record<string, true>>({});
  // 自动保存（§8.2）：tab 未保存圆点 + 去抖写盘调度器
  const [unsaved, setUnsaved] = useState<Record<string, true>>({});
  const autosaverRef = useRef<AutoSaver | null>(null);
  // 全局设置（§7.2）：会话默认档等；设置面板开关
  const [settings, setSettings] = useState<SettingsData | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  useEffect(() => {
    void api
      .getSettings()
      .then(setSettings)
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const saver = createAutoSaver(async (path, content) => {
      const pid = projectIdRef.current;
      if (!pid) return;
      await api.writeFile(pid, path, content);
      // 落盘成功 → 脏缓冲解除（§8.6「未保存缓冲」语义：已保存不再是缓冲）
      await api.clearBuffer(pid, path).catch(() => {});
      setUnsaved((prev) => {
        if (!prev[path]) return prev;
        const next = { ...prev };
        delete next[path];
        return next;
      });
    }, 1000);
    autosaverRef.current = saver;
    return () => saver.dispose();
  }, [api]);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [sideView, setSideView] = useState<SideView>(loadSideView);
  /** rail 点击语义：同视图再点 = 折叠侧栏；否则切换视图并展开。 */
  const toggleSideView = useCallback(
    (view: SideView) => {
      if (sidebarOpen && sideView === view) {
        setSidebarOpen(false);
        return;
      }
      setSideView(view);
      localStorage.setItem(SIDE_VIEW_KEY, view);
      setSidebarOpen(true);
    },
    [sideView, sidebarOpen]
  );
  const [agentState, setAgentState] = useState<AgentStateName>("idle");
  const [routeNote, setRouteNote] = useState<string | null>(null);
  const [bottomTab, setBottomTab] = useState<"timeline" | "trace" | "evals">("timeline");
  // 可调布局（§7.2：三区可折叠可调宽；localStorage 记忆）
  const [leftWidth, setLeftWidth] = useState(() => Number(localStorage.getItem("tenon:leftWidth")) || 220);
  const [rightWidth, setRightWidth] = useState(() => Number(localStorage.getItem("tenon:rightWidth")) || 420);
  const [bottomHeight, setBottomHeight] = useState(() => Number(localStorage.getItem("tenon:bottomHeight")) || 180);

  const tabs = projectId ? tabsByProject[projectId] ?? [] : [];
  const activePath = projectId ? activePathByProject[projectId] ?? null : null;
  const sessionId = projectId ? sessionsByProject[projectId] ?? null : null;

  useEffect(() => {
    tabsByProjectRef.current = tabsByProject;
    activePathByProjectRef.current = activePathByProject;
    unsavedRef.current = unsaved;
  }, [tabsByProject, activePathByProject, unsaved]);

  const refreshProjects = useCallback(async () => {
    const r = await api.listProjects();
    setProjects(r.projects);
    setSessionsByProject((prev) => {
      const next = { ...prev };
      for (const project of r.projects) {
        if (!next[project.id]) {
          const persisted = project.sessions[0]?.id;
          if (persisted) next[project.id] = persisted;
        }
      }
      return next;
    });
  }, [api]);

  const refreshPortfolio = useCallback(async () => {
    const r = await api.portfolioTasks();
    setPortfolioTasks(r.tasks.filter((task) => task.status !== "done"));
  }, [api]);

  /** 项目级状态恢复：布局 + tab 路径 + active path + 可复用 session（§7.2/§7.5）。 */
  const activateProject = useCallback(
    async (project: ProjectSummary) => {
      setProjectId(project.id);
      projectIdRef.current = project.id;
      const saved = await api.projectUiState(project.id);
      if (typeof saved.leftWidth === "number") {
        setLeftWidth(Math.min(480, Math.max(140, saved.leftWidth)));
      }
      if (typeof saved.rightWidth === "number") {
        setRightWidth(Math.min(720, Math.max(260, saved.rightWidth)));
      }
      if (typeof saved.bottomHeight === "number") {
        setBottomHeight(Math.min(480, Math.max(80, saved.bottomHeight)));
      }
      if (typeof saved.sidebarOpen === "boolean") setSidebarOpen(saved.sidebarOpen);
      if (typeof saved.timelineOpen === "boolean") setTimelineOpen(saved.timelineOpen);
      if (saved.bottomTab === "timeline" || saved.bottomTab === "trace" || saved.bottomTab === "evals") {
        setBottomTab(saved.bottomTab);
      }

      const paths = Array.from(new Set(saved.tabs ?? [])).slice(0, 50);
      const restored = (
        await Promise.all(
          paths.map(async (path) => {
            try {
              const file = await api.readFile(project.id, path);
              return { path, content: file.content };
            } catch {
              return null;
            }
          })
        )
      ).filter((tab): tab is EditorTab => tab !== null);
      setTabsByProject((prev) => ({ ...prev, [project.id]: restored }));
      const activePath =
        saved.activePath && restored.some((tab) => tab.path === saved.activePath)
          ? saved.activePath
          : (restored[0]?.path ?? null);
      setActivePathByProject((prev) => ({ ...prev, [project.id]: activePath }));

      const savedSession =
        saved.sessionId && project.sessions.some((session) => session.id === saved.sessionId)
          ? saved.sessionId
          : undefined;
      const existing = savedSession ?? project.sessions[0]?.id;
      if (existing) {
        setSessionsByProject((prev) => ({ ...prev, [project.id]: existing }));
      } else {
        const session = await api.createSession(
          project.id,
          (settings?.session?.mode as "interactive" | "auto" | "" | undefined) ??
          "interactive"
        );
        setSessionsByProject((prev) => ({ ...prev, [project.id]: session.session_id }));
      }
      projectUiStateLoaded.current.add(project.id);
    },
    [api, settings]
  );

  const setActivePath = useCallback((path: string | null) => {
    if (!projectId) return;
    setActivePathByProject((prev) => ({ ...prev, [projectId]: path }));
  }, [projectId]);

  const setTabs = useCallback((updater: (prev: EditorTab[]) => EditorTab[]) => {
    if (!projectId) return;
    setTabsByProject((prev) => ({ ...prev, [projectId]: updater(prev[projectId] ?? []) }));
  }, [projectId]);

  // 打开项目 + 建会话（§7.3：TOFU 现阶段自动信任；后续替换为显式信任卡）
  const openProject = useCallback(async (path: string) => {
    const opened = await api.openProject(path);
    const trusted = window.confirm(`信任项目目录并启用其配置？\n${opened.path}`);
    if (trusted) await api.setTrust(opened.id, true);
    await refreshProjects();
    const summary =
      projects.find((project) => project.id === opened.id) ??
      ({
        ...opened,
        sessions: [],
        active_sessions: 0,
        dirty_buffers: 0,
        pending_approvals: [],
        usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
      } satisfies ProjectSummary);
    await activateProject(summary);
  }, [activateProject, api, projects, refreshProjects]);

  const switchProject = useCallback(async (project: ProjectSummary) => {
    await activateProject(project);
  }, [activateProject]);

  const openFile = useCallback(
    async (path: string, line?: number) => {
      if (tabs.some((tab) => tab.path === path)) {
        setActivePath(path);
        if (line) setGotoLine({ path, line, token: Date.now() });
        return;
      }
      if (!projectId) return;
      const r = await api.readFile(projectId, path);
      setTabsByProject((prev) => ({ ...prev, [projectId]: [...(prev[projectId] ?? []), { path, content: r.content }] }));
      setActivePathByProject((prev) => ({ ...prev, [projectId]: path }));
      if (line) setGotoLine({ path, line, token: Date.now() });
    },
    [api, projectId, tabs]
  );

  const handleFileTreeChange = useCallback(
    (change: FileTreeChange) => {
      setFileTreeVersion((version) => version + 1);
      const projectId = projectIdRef.current;
      if (!projectId) return;
      if (change.type === "renamed") {
        setTabsByProject((prev) => ({
          ...prev,
          [projectId]: (prev[projectId] ?? []).map((tab) =>
            tab.path === change.from ? { ...tab, path: change.to } : tab
          ),
        }));
        setActivePathByProject((prev) =>
          prev[projectId] === change.from
            ? { ...prev, [projectId]: change.to }
            : prev
        );
        setUnsaved((prev) => {
          if (!prev[change.from]) return prev;
          const next = { ...prev };
          delete next[change.from];
          next[change.to] = true;
          return next;
        });
        setAiLines((prev) => {
          if (!prev[change.from]) return prev;
          const next = { ...prev };
          next[change.to] = next[change.from];
          delete next[change.from];
          return next;
        });
        return;
      }
      if (change.type !== "deleted") return;
      const tabs = tabsByProjectRef.current[projectId] ?? [];
      const nextActive =
        activePathByProjectRef.current[projectId] === change.path
          ? (tabs.find((tab) => tab.path !== change.path)?.path ?? null)
          : activePathByProjectRef.current[projectId];
      setTabsByProject((prev) => ({
        ...prev,
        [projectId]: (prev[projectId] ?? []).filter((tab) => tab.path !== change.path),
      }));
      setActivePathByProject((prev) => ({ ...prev, [projectId]: nextActive ?? null }));
      setUnsaved((prev) => {
        if (!prev[change.path]) return prev;
        const next = { ...prev };
        delete next[change.path];
        return next;
      });
      setAiLines((prev) => ({ ...prev, [change.path]: [] }));
    },
    []
  );

  const onStateChange = useCallback((s: AgentStateName) => setAgentState(s), []);
  const handlers = useMemo(
    () => ({
      onPalette: () => setPaletteOpen((v) => !v),
      onGotoFile: () => setFinderOpen(true),
      onTimeline: () => setTimelineOpen((v) => !v),
      onSidebar: () => setSidebarOpen((v) => !v),
      onStop: () => sessionId && api.control(sessionId, "stop"),
      onSave: () => {
        const p = activePath;
        if (p) void autosaverRef.current?.flush(p);
      },
      onSettings: () => setSettingsOpen(true),
      onPauseOrClose: () => {
        if (paletteOpen) setPaletteOpen(false);
        else if (sessionId) api.control(sessionId, "pause");
      },
    }),
    [api, sessionId, paletteOpen, activePath]
  );
  useShortcuts(handlers);

  const commands: Command[] = useMemo(
    () => [
      { id: "open.timeline", label: t("panel.timeline"), run: () => setTimelineOpen(true) },
      { id: "open.settings", label: t("settings.open"), run: () => setSettingsOpen(true) },
      { id: "toggle.sidebar", label: t("panel.files"), run: () => setSidebarOpen((v) => !v) },
      {
        id: "agent.pause",
        label: t("message.pause"),
        run: () => sessionId && api.control(sessionId, "pause"),
      },
      {
        id: "agent.stop",
        label: t("message.stop"),
        run: () => sessionId && api.control(sessionId, "stop"),
      },
      {
        id: "agent.rollback",
        label: t("timeline.rollback"),
        run: () => sessionId && api.control(sessionId, "rollback"),
      },
      {
        id: "agent.unrollback",
        label: t("timeline.unrevert"),
        run: () => sessionId && api.control(sessionId, "unrollback"),
      },
    ],
    [t, api, sessionId]
  );

  // M0：挂载即自动打开项目并建会话（M1 换项目选择页 + TOFU 卡）
  const [openError, setOpenError] = useState<string | null>(null);
  useEffect(() => {
    openProject(projectPath).catch((e) => {
      setOpenError(String(e));
    });
  }, [openProject, projectPath]);
  useEffect(() => {
    const id = window.setInterval(() => {
      void refreshProjects().catch(() => {});
      void refreshPortfolio().catch(() => {});
    }, 2000);
    return () => window.clearInterval(id);
  }, [refreshProjects, refreshPortfolio]);
  // 项目级 UI 状态去抖持久化（§7.2 / §7.5）：加载完成后才允许覆盖远端。
  useEffect(() => {
    if (!projectId || !projectUiStateLoaded.current.has(projectId)) return;
    const timer = window.setTimeout(() => {
      api.saveProjectUiState(projectId, {
        sessionId: sessionsByProject[projectId],
        tabs: (tabsByProject[projectId] ?? []).map((tab) => tab.path),
        activePath: activePathByProject[projectId] ?? null,
        leftWidth,
        rightWidth,
        bottomHeight,
        sidebarOpen,
        timelineOpen,
        bottomTab,
      });
    }, 600);
    return () => window.clearTimeout(timer);
  }, [
    api,
    projectId,
    sessionsByProject,
    tabsByProject,
    activePathByProject,
    leftWidth,
    rightWidth,
    bottomHeight,
    sidebarOpen,
    timelineOpen,
    bottomTab,
  ]);
  useEffect(() => {
    if (!projectId) return;
    let alive = true;
    let socket: WebSocket | null = null;
    let retry: number | null = null;
    let summaryTimer: number | null = null;

    const scheduleSummary = () => {
      if (summaryTimer !== null) window.clearTimeout(summaryTimer);
      summaryTimer = window.setTimeout(() => {
        void refreshProjects().catch(() => {});
      }, 400);
    };

    const handleEvent = async (raw: unknown) => {
      const event = raw as {
        project_id?: string;
        path?: string;
        type?: string;
      };
      if (!alive || event.project_id !== projectId || !event.path) return;
      setFileTreeVersion((version) => version + 1);
      scheduleSummary();
      if (event.type === "removed") {
        setTabsByProject((prev) => ({
          ...prev,
          [projectId]: (prev[projectId] ?? []).filter((tab) => tab.path !== event.path),
        }));
        setActivePathByProject((prev) => {
          if (prev[projectId] !== event.path) return prev;
          const nextPath =
            (tabsByProjectRef.current[projectId] ?? []).find((tab) => tab.path !== event.path)
              ?.path ?? null;
          return { ...prev, [projectId]: nextPath };
        });
        setAiLines((prev) => ({ ...prev, [event.path as string]: [] }));
        return;
      }
      if (event.type !== "created" && event.type !== "modified") return;
      // 自动保存中的缓冲不回读，避免覆盖用户正在输入的内容。
      if (unsavedRef.current[event.path]) return;
      if (!(tabsByProjectRef.current[projectId] ?? []).some((tab) => tab.path === event.path)) {
        return;
      }
      try {
        const file = await api.readFile(projectId, event.path);
        if (!alive) return;
        setTabsByProject((prev) => ({
          ...prev,
          [projectId]: (prev[projectId] ?? []).map((tab) =>
            tab.path === event.path ? { ...tab, content: file.content } : tab
          ),
        }));
      } catch {
        // 文件可能在事件消费前又被移除；下一条 removed 事件会处理。
      }
    };

    const connect = async () => {
      if (!alive) return;
      try {
        socket = await api.connectEvents((event) => void handleEvent(event), projectId);
        socket.onclose = () => {
          if (!alive) return;
          retry = window.setTimeout(() => void connect(), 1000);
        };
      } catch {
        if (alive) retry = window.setTimeout(() => void connect(), 1000);
      }
    };
    void connect();

    return () => {
      alive = false;
      if (retry !== null) window.clearTimeout(retry);
      if (summaryTimer !== null) window.clearTimeout(summaryTimer);
      socket?.close();
    };
  }, [api, projectId, refreshProjects]);

  return (
    <div className="app" data-testid="app">
      <header className="app-head">
        <strong>{t("app.title")}</strong>
        <select
          aria-label="active project"
          data-testid="project-switcher"
          value={projectId ?? ""}
          onChange={(e) => {
            const project = projects.find((p) => p.id === e.target.value);
            if (project) void switchProject(project);
          }}
        >
          {!projectId && <option value="">No project</option>}
          {projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.display_name}{project.active_sessions ? ` · ${project.active_sessions} active` : ""}
            </option>
          ))}
          {portfolioTasks.map((task) => (
            <span className="task-pill" key={task.id} data-testid="portfolio-task">
              <strong>{task.title}</strong>
              <span>{task.status}</span>
            </span>
          ))}
        </select>
        <form
          className="project-open"
          onSubmit={(e) => {
            e.preventDefault();
            void openProject(openPath).catch((err) => setOpenError(String(err)));
          }}
        >
          <input
            aria-label="project path"
            data-testid="project-path"
            value={openPath}
            onChange={(e) => setOpenPath(e.target.value)}
            placeholder="/absolute/path/to/project"
          />
          <button type="submit">Open</button>
        </form>
        <span className="spacer" />
        <ModelRoutingPanel
          api={api}
          sessionId={sessionId}
          onSwitched={(m) => {
            // 切换提示（§11：上下文随迁，model_fallback 事件入 Trace）
            setRouteNote(`已切换模型：${m}（上下文随迁）`);
            window.setTimeout(() => setRouteNote(null), 4000);
          }}
        />
        <ThemePicker api={api} t={t} />
        <LanguagePicker />
      </header>
      {routeNote && (
        <div className="route-note" data-testid="route-note">
          {routeNote}
        </div>
      )}
      {(openError || projects.length > 0) && (
        <div className="task-center" data-testid="task-center">
          {openError && <span className="task-error">{openError}</span>}
          {projects.map((project) => (
            <button
              key={project.id}
              className={project.id === projectId ? "task-pill active" : "task-pill"}
              onClick={() => void switchProject(project)}
              title={project.path}
            >
              <strong>{project.display_name}</strong>
              <span>
                {[
                  project.active_sessions ? `${project.active_sessions} active` : "idle",
                  project.pending_approvals.length ? `${project.pending_approvals.length} approvals` : null,
                  project.dirty_buffers ? `${project.dirty_buffers} dirty` : null,
                  project.usage.cost_usd > 0 ? `$${project.usage.cost_usd.toFixed(4)}` : null,
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            </button>
          ))}
        </div>
      )}
      <div className="workspace">
        <nav className="activity-rail" aria-label={t("rail.label")}>
          <button
            type="button"
            className={sidebarOpen && sideView === "files" ? "rail-btn active" : "rail-btn"}
            data-testid="rail-files"
            title={t("panel.files")}
            aria-label={t("panel.files")}
            aria-pressed={sidebarOpen && sideView === "files"}
            onClick={() => toggleSideView("files")}
          >
            <RailIcon view="files" />
          </button>
          <button
            type="button"
            className={sidebarOpen && sideView === "search" ? "rail-btn active" : "rail-btn"}
            data-testid="rail-search"
            title={t("search.title")}
            aria-label={t("search.title")}
            aria-pressed={sidebarOpen && sideView === "search"}
            onClick={() => toggleSideView("search")}
          >
            <RailIcon view="search" />
          </button>
          <button
            type="button"
            className={sidebarOpen && sideView === "packs" ? "rail-btn active" : "rail-btn"}
            data-testid="rail-packs"
            title={t("panel.packs")}
            aria-label={t("panel.packs")}
            aria-pressed={sidebarOpen && sideView === "packs"}
            onClick={() => toggleSideView("packs")}
          >
            <RailIcon view="packs" />
          </button>
          <span className="rail-spacer" />
        </nav>
        {sidebarOpen && (
          <>
            <aside className="zone zone-left" style={{ width: leftWidth, minWidth: 140, maxWidth: 480 }}>
              <div className="side-head">
                <span className="side-title">
                  {sideView === "files"
                    ? t("panel.files")
                    : sideView === "search"
                      ? t("search.title")
                      : t("panel.packs")}
                </span>
              </div>
              <div className="side-body">
                {sideView === "files" && (
                  <FileTree
                    api={api}
                    t={t}
                    projectId={projectId}
                    refreshToken={fileTreeVersion}
                    onOpenFile={openFile}
                    onOperation={handleFileTreeChange}
                  />
                )}
                {sideView === "search" && (
                  <SearchPanel
                    api={api}
                    t={t}
                    projectId={projectId}
                    onOpenFile={(path, line) => void openFile(path, line)}
                    onChanged={() => setFileTreeVersion((version) => version + 1)}
                  />
                )}
                {sideView === "packs" && (
                  <LanguagePackWizard
                    api={api}
                    projectId={projectId}
                    onInstalled={() => {
                      // 重新拉取文件树无必要；向导自身刷新状态
                    }}
                  />
                )}
              </div>
            </aside>
            <ResizeHandle
              dir="horizontal"
              testId="resize-left"
              onResize={(d) =>
                setLeftWidth((w) => {
                  const v = Math.min(480, Math.max(140, w + d));
                  localStorage.setItem("tenon:leftWidth", String(v));
                  return v;
                })
              }
              onDoubleClick={() => setLeftWidth(220)}
            />
          </>
        )}
        <section
          className="zone zone-center"
          style={{ flex: 1, minWidth: 200 }}
        >
          <EditorPane
            tabs={tabs}
            activePath={activePath}
            aiModifiedLines={aiLines}
            unsavedPaths={unsaved}
            unsavedTitle={t("editor.unsaved")}
            goto={gotoLine}
            onSelect={setActivePath}
            onClose={(p) => {
              void autosaverRef.current?.flush(p);
              setUnsaved((prev) => {
                if (!prev[p]) return prev;
                const next = { ...prev };
                delete next[p];
                return next;
              });
              setTabs((prev) => prev.filter((tab) => tab.path !== p));
              if (activePath === p) {
                setActivePath(tabs.find((tab) => tab.path !== p)?.path ?? null);
              }
            }}
            onChange={(p, content) => {
              setTabs((prev) => prev.map((tab) => (tab.path === p ? { ...tab, content } : tab)));
              // §8.6：用户编辑 → 该文件 AI 角标解除 + 脏缓冲推送（去抖）
              setAiLines((prev) => ({ ...prev, [p]: [] }));
              autosaverRef.current?.schedule(p, content);
              setUnsaved((prev) => (prev[p] ? prev : { ...prev, [p]: true }));
              const pid = projectIdRef.current;
              if (pid) {
                const dirtyKey = `${pid}\u0000${p}`;
                const tid = dirtyTimers.current.get(dirtyKey);
                if (tid) window.clearTimeout(tid);
                dirtyTimers.current.set(
                  dirtyKey,
                  window.setTimeout(() => {
                    void api.putBuffer(pid, p, content).catch(() => {});
                  }, 400)
                );
              }
            }}
          />
        </section>
        <ResizeHandle
          dir="horizontal"
          testId="resize-right"
          onResize={(d) =>
            setRightWidth((w) => {
              const v = Math.min(720, Math.max(260, w - d));
              localStorage.setItem("tenon:rightWidth", String(v));
              return v;
            })
          }
          onDoubleClick={() => setRightWidth(420)}
        />
        <section className="zone zone-right" style={{ width: rightWidth, minWidth: 260, maxWidth: 720 }}>
          <AgentPanel
            api={api}
            t={t}
            sessionId={sessionId}
            onStateChange={onStateChange}
            onLatestDiff={setLatestDiff}
            onPatchLines={(path, lines) =>
              setAiLines((prev) => ({
                ...prev,
                [path]: unionLines(prev[path] ?? [], lines),
              }))
            }
            onDirtyConflict={setDirtyConflict}
          />
        </section>
      </div>
      {dirtyConflict && (
        <div className="merge-overlay">
          <ThreePaneMerge
            conflict={dirtyConflict}
            labels={{
              ours: "代理改动",
              theirs: "你的改动（未保存）",
              base: "自动合并结果",
              apply: "应用选中版本并保存",
              keepMine: "保留我的版本",
              useAgent: "应用代理版本",
            }}
            onResolve={(path, chosen, clear) => {
              void (async () => {
                const pid = projectIdRef.current;
                if (pid) await api.writeFile(pid, path, chosen).catch(() => {});
                if (pid && clear) await api.clearBuffer(pid, path).catch(() => {});
                setTabs((prev) =>
                  prev.some((tab) => tab.path === path)
                    ? prev.map((tab) => (tab.path === path ? { ...tab, content: chosen } : tab))
                    : [...prev, { path, content: chosen }]
                );
                setActivePath(path);
                setDirtyConflict(null);
              })();
            }}
          />
        </div>
      )}
      {timelineOpen && (
        <footer className="zone-bottom" style={{ height: bottomHeight }}>
          <ResizeHandle dir="vertical" onResize={(d) =>
            setBottomHeight((h) => {
              const v = Math.min(480, Math.max(80, h - d));
              localStorage.setItem("tenon:bottomHeight", String(v));
              return v;
            })
          } />
          <div className="bottom-tabs">
            <button
              className={bottomTab === "timeline" ? "active" : ""}
              onClick={() => setBottomTab("timeline")}
            >
              {t("panel.timeline")}
            </button>
            <button
              className={bottomTab === "trace" ? "active" : ""}
              onClick={() => setBottomTab("trace")}
              data-testid="tab-trace"
            >
              AgentTrace
            </button>
            <button
              className={bottomTab === "evals" ? "active" : ""}
              onClick={() => setBottomTab("evals")}
              data-testid="tab-evals"
            >
              AI Evals
            </button>
          </div>
          {bottomTab === "timeline" && (
            <div className="bottom-grid">
              <CheckpointTimeline api={api} t={t} sessionId={sessionId} />
              <DiffPanel diff={latestDiff} title={t("panel.diagnostics")} />
            </div>
          )}
          {bottomTab === "trace" && (
            <AgentTracePanel api={api} sessionId={sessionId} />
          )}
          {bottomTab === "evals" && <EvalsPanel api={api} />}
        </footer>
      )}
      <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} commands={commands} />
      {settingsOpen && (
        <SettingsDialog
          api={api}
          t={t}
          settings={settings}
          onClose={() => setSettingsOpen(false)}
          onSaved={setSettings}
        />
      )}
      <FileFinder
        open={finderOpen}
        onClose={() => setFinderOpen(false)}
        api={api}
        t={t}
        projectId={projectId}
        projectRoot={
          projects.find((project) => project.id === projectId)?.path ?? projectPath
        }
        activePath={activePath}
        onOpen={(path, line) => void openFile(path, line)}
      />
      <span className="sr-only" data-testid="agent-state">{agentState}</span>
    </div>
  );
}

function LanguagePicker() {
  const [locale, setLocaleState] = useState<Locale>("auto");
  return (
    <select
      aria-label="language"
      value={locale}
      onChange={(e) => {
        const v = e.target.value as Locale;
        setLocaleState(v);
        window.dispatchEvent(new CustomEvent(LOCALE_CHANGE, { detail: v }));
      }}
    >
      <option value="auto">Auto</option>
      <option value="en">English</option>
      <option value="zh-CN">中文</option>
    </select>
  );
}

/** 外观档（§7.5）：深色 / 浅色 / 跟随系统；即时生效 + 双写
 *  localStorage（快路径）与 daemon /ui-prefs（跨启动权威，端口动态
 *  导致 localStorage 按 origin 隔离不可依赖）。 */
function ThemePicker({ api, t }: { api: TenonApi; t: Translate }) {
  const [pref, setPref] = useState<ThemePreference>(() => loadThemePreference());
  // 跟随系统档：系统深浅切换时重应用
  useEffect(() => watchSystemTheme(() => applyTheme(pref)), [pref]);
  return (
    <select
      aria-label="theme"
      data-testid="theme-picker"
      value={pref}
      onChange={(e) => {
        const v = e.target.value as ThemePreference;
        setPref(v);
        saveThemePreference(v);
        applyTheme(v);
        api.setUiPrefs({ theme: v });
      }}
    >
      <option value="system">{t("theme.system")}</option>
      <option value="dark">{t("theme.dark")}</option>
      <option value="light">{t("theme.light")}</option>
    </select>
  );
}
