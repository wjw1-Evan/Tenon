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
import { FileTree } from "./components/FileTree";
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
import { CommandPalette, type Command } from "./components/CommandPalette";

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
  const [timelineOpen, setTimelineOpen] = useState(true);
  const [latestDiff, setLatestDiff] = useState<string | null>(null);
  const [dirtyConflict, setDirtyConflict] = useState<DirtyConflict | null>(null);
  const [aiLines, setAiLines] = useState<Record<string, number[]>>({});
  const projectIdRef = useRef<string | null>(null);
  const dirtyTimers = useRef<Map<string, number>>(new Map());
  const [sidebarOpen, setSidebarOpen] = useState(true);
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
    setProjectId(opened.id);
    projectIdRef.current = opened.id;
    const session = await api.createSession(opened.id, "interactive");
    setSessionsByProject((prev) => ({ ...prev, [opened.id]: session.session_id }));
  }, [api, refreshProjects]);

  const switchProject = useCallback(async (project: ProjectSummary) => {
    setProjectId(project.id);
    projectIdRef.current = project.id;
    const existing = sessionsByProject[project.id] ?? project.sessions[0]?.id;
    if (!existing) {
      const session = await api.createSession(project.id, "interactive");
      setSessionsByProject((prev) => ({ ...prev, [project.id]: session.session_id }));
    } else if (!sessionsByProject[project.id]) {
      setSessionsByProject((prev) => ({ ...prev, [project.id]: existing }));
    }
  }, [api, sessionsByProject]);

  const openFile = useCallback(
    async (path: string) => {
      if (tabs.some((tab) => tab.path === path)) {
        setActivePath(path);
        return;
      }
      if (!projectId) return;
      const r = await api.readFile(projectId, path);
      setTabsByProject((prev) => ({ ...prev, [projectId]: [...(prev[projectId] ?? []), { path, content: r.content }] }));
      setActivePathByProject((prev) => ({ ...prev, [projectId]: path }));
    },
    [api, projectId, tabs]
  );

  const onStateChange = useCallback((s: AgentStateName) => setAgentState(s), []);
  const handlers = useMemo(
    () => ({
      onPalette: () => setPaletteOpen((v) => !v),
      onGotoFile: () => setPaletteOpen(true),
      onTimeline: () => setTimelineOpen((v) => !v),
      onSidebar: () => setSidebarOpen((v) => !v),
      onStop: () => sessionId && api.control(sessionId, "stop"),
      onPauseOrClose: () => {
        if (paletteOpen) setPaletteOpen(false);
        else if (sessionId) api.control(sessionId, "pause");
      },
    }),
    [api, sessionId, paletteOpen]
  );
  useShortcuts(handlers);

  const commands: Command[] = useMemo(
    () => [
      { id: "open.timeline", label: t("panel.timeline"), run: () => setTimelineOpen(true) },
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

  return (
    <div className="app" data-testid="app">
      <header className="app-head">
        <strong>{t("app.title")}</strong>
        <span className="muted">{t("app.subtitle")}</span>
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
              <span>{project.active_sessions ? `${project.active_sessions} active` : "idle"}</span>
            </button>
          ))}
        </div>
      )}
      <div className="workspace">
        {sidebarOpen && (
          <>
            <aside className="zone zone-left" style={{ width: leftWidth, minWidth: 140, maxWidth: 480 }}>
              <LanguagePackWizard
                api={api}
                projectId={projectId}
                onInstalled={() => {
                  // 重新拉取文件树无必要；向导自身刷新状态
                }}
              />
              <FileTree api={api} t={t} projectId={projectId} onOpenFile={openFile} />
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
            onSelect={setActivePath}
            onClose={(p) => {
              setTabs((prev) => prev.filter((tab) => tab.path !== p));
              if (activePath === p) {
                setActivePath(tabs.find((tab) => tab.path !== p)?.path ?? null);
              }
            }}
            onChange={(p, content) => {
              setTabs((prev) => prev.map((tab) => (tab.path === p ? { ...tab, content } : tab)));
              // §8.6：用户编辑 → 该文件 AI 角标解除 + 脏缓冲推送（去抖）
              setAiLines((prev) => ({ ...prev, [p]: [] }));
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
