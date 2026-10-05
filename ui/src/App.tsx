// 主工作区（设计方案 §7.2 v1.110）：项目侧栏 | 代理线程主区（满宽）；
// 编辑器为应用内浮层（单击文件弹出，✕ 返回线程）；底部 时间轴/轨迹/评估。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ResizeHandle } from "./components/ResizeHandle";
import { TenonApi } from "./lib/api";
import type { ProjectSummary } from "./lib/api";
import {
  LOCALES,
  LOCALE_CHANGE,
  readLocalePreference,
  resolveLocale,
  saveLocalePreference,
  useLocaleTranslator,
  type Locale,
  type Translate,
} from "./lib/i18n";
import {
  applyTheme,
  loadThemePreference,
  saveThemePreference,
  watchSystemTheme,
  type ThemePreference,
} from "./lib/theme";
import type { AgentStateName } from "./lib/stateColors";
import { playDoneChime, shouldChimeOnTransition } from "./lib/notifySound";
import { useShortcuts } from "./hooks";
import type { FileTreeChange } from "./components/FileTree";
import { ProjectExplorer } from "./components/ProjectExplorer";
import { SearchPanel } from "./components/SearchPanel";
import { FileFinder } from "./components/FileFinder";
import { EditorPane, type EditorSelection, type EditorTab } from "./components/EditorPane";
import { InlineInstruction, buildInlineTask, type InlineTarget } from "./components/InlineInstruction";
import { AgentPanel } from "./components/AgentPanel";
import { CheckpointTimeline } from "./components/CheckpointTimeline";
import { DiffPanel } from "./components/DiffPanel";
import { L4StatusPanel } from "./components/L4StatusPanel";
import {
  DiagnosticsPanel,
  type EditorDiagnostic,
} from "./components/DiagnosticsPanel";
import { ThreePaneMerge, type DirtyConflict } from "./components/ThreePaneMerge";
import { AgentTracePanel } from "./components/AgentTracePanel";
import { EvalsPanel } from "./components/EvalsPanel";
import { LanguagePackWizard } from "./components/LanguagePackWizard";
import { unionLines } from "./lib/aiLines";
import { createAutoSaver, type AutoSaver } from "./lib/autosave";
import { saveNow as saveNowBuffered } from "./lib/save";
import {
  effectiveBottom,
  effectiveFloatWidth,
  effectiveLeft,
  useViewport,
} from "./lib/viewport";
import { SettingsDialog, type SettingsData } from "./components/SettingsDialog";
import { CommandPalette, type Command } from "./components/CommandPalette";

/** 侧栏视图（布局 §7.2 重设计）：activity rail 单视图切换，localStorage 记忆。 */
type SideView = "projects" | "search" | "packs";
const SIDE_VIEW_KEY = "tenon:sideView";

function loadSideView(): SideView {
  try {
    const raw = localStorage.getItem(SIDE_VIEW_KEY);
    // 兼容旧值：「文件」视图已升级为「项目」。
    if (raw === "search" || raw === "packs") return raw;
    return "projects";
  } catch {
    return "projects";
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
  if (view === "projects") {
    return (
      <svg {...props}>
        <path d="M3 7V5a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
        <path d="M3 7h18" />
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
  /** 语言偏好（v1.100 多语言）：prop 为初值；订阅 LOCALE_CHANGE（顶栏派发）即时切换，
   *  偏好双写——localStorage 快路径 + daemon ui-prefs 跨启动权威（§7.5，与外观档同法）。 */
  const [localePref, setLocalePref] = useState<Locale>(locale);
  const api = useMemo(() => new TenonApi(handshake), [handshake]);
  useEffect(() => {
    const stored = readLocalePreference();
    if (stored !== locale) setLocalePref(stored);
    const onLocaleChange = (event: Event) => {
      const next = (event as CustomEvent).detail as Locale;
      setLocalePref(next);
      saveLocalePreference(next);
      api.setUiPrefs({ locale: next });
    };
    window.addEventListener(LOCALE_CHANGE, onLocaleChange);
    return () => window.removeEventListener(LOCALE_CHANGE, onLocaleChange);
  }, [api, locale]);
  /** 翻译器随偏好重建；懒加载语言（zh-TW / ja / ko）资源就绪后自动重渲染。 */
  const t = useLocaleTranslator(localePref);
  useEffect(() => {
    // <html lang> 随应用语言（无障碍 / 屏幕阅读器发音）
    document.documentElement.lang = resolveLocale(localePref);
  }, [localePref]);

  const [projectId, setProjectId] = useState<string | null>(null);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [sessionsByProject, setSessionsByProject] = useState<Record<string, string>>({});
  /** 新任务草稿态（v1.116 §7.2）：点「＋ 新任务」不再急切建会话，记录待启动意图
   *  （false=主根 / true=受管 worktree）；首条消息发出才落库建会话并进列表。 */
  const [draftByProject, setDraftByProject] = useState<Record<string, boolean>>({});
  const draftByProjectRef = useRef<Record<string, boolean>>({});
  const [tabsByProject, setTabsByProject] = useState<Record<string, EditorTab[]>>({});
  const [activePathByProject, setActivePathByProject] = useState<Record<string, string | null>>({});
  const [splitPathByProject, setSplitPathByProject] = useState<Record<string, string | null>>({});
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [finderOpen, setFinderOpen] = useState(false);
  // 底部面板默认收起（v1.78 复刻 Codex 形态：无常驻底栏）；
  // 项目 ui-state 记忆（saved.timelineOpen）优先于新默认。
  const [timelineOpen, setTimelineOpen] = useState(false);
  const [latestDiff, setLatestDiff] = useState<string | null>(null);
  const [dirtyConflict, setDirtyConflict] = useState<DirtyConflict | null>(null);
  const [aiLines, setAiLines] = useState<Record<string, number[]>>({});
  /** ProjectRuntime 文件事件版本：驱动文件树增量刷新与打开缓冲同步（§6.4 / §8.1）。 */
  const [fileTreeVersion, setFileTreeVersion] = useState(0);
  const [gotoLine, setGotoLine] = useState<{ path: string; line: number; token: number } | null>(
    null
  );
  const [injectedTask, setInjectedTask] = useState<{ token: number; text: string } | null>(null);
  /** 行内指令（§7.4 Cmd+I / §8.5）：编辑器选区上下文与弹卡开关。 */
  const [selection, setSelection] = useState<EditorSelection | null>(null);
  const [inlineOpen, setInlineOpen] = useState(false);
  /** 跟随模式（§8.5）：代理写入文件时自动打开并滚动到首个改动行；默认开、可关。 */
  const [followMode, setFollowMode] = useState<boolean>(() => {
    try {
      return localStorage.getItem("tenon:followMode") !== "off";
    } catch {
      return true;
    }
  });
  /** AI ghost text（§8.3 / §4.1）：P2 实验，设计要求默认关闭。 */
  const [inlineCompletionEnabled, setInlineCompletionEnabled] = useState<boolean>(() => {
    try {
      return localStorage.getItem("tenon:inlineCompletion") === "on";
    } catch {
      return false;
    }
  });
  const followModeRef = useRef(followMode);
  useEffect(() => {
    followModeRef.current = followMode;
  }, [followMode]);
  useEffect(() => {
    draftByProjectRef.current = draftByProject;
  }, [draftByProject]);
  const toggleFollow = useCallback(() => {
    setFollowMode((v) => {
      const next = !v;
      try {
        localStorage.setItem("tenon:followMode", next ? "on" : "off");
      } catch {
        // 仅当前会话生效
      }
      return next;
    });
  }, []);
  const toggleInlineCompletion = useCallback(() => {
    setInlineCompletionEnabled((value) => {
      const next = !value;
      try {
        localStorage.setItem("tenon:inlineCompletion", next ? "on" : "off");
      } catch {
        // 仅当前会话生效
      }
      setRouteNote(next ? t("inline.enabled_note") : t("inline.disabled_note"));
      window.setTimeout(() => setRouteNote(null), 3000);
      return next;
    });
  }, []);
  const projectIdRef = useRef<string | null>(null);
  const projectUiStateLoaded = useRef<Set<string>>(new Set());
  const dirtyTimers = useRef<Map<string, number>>(new Map());
  const tabsByProjectRef = useRef<Record<string, EditorTab[]>>({});
  const activePathByProjectRef = useRef<Record<string, string | null>>({});
  const splitPathByProjectRef = useRef<Record<string, string | null>>({});
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
  // 保存模式（§8.2 v1.75）：auto（默认，去抖自动写盘）| manual（仅显式保存写盘）。
  // 偏好存 daemon ui_prefs（§7.5 权威，键 editor.saveMode），设置面板即时切换。
  const [saveMode, setSaveMode] = useState<"auto" | "manual">("auto");
  useEffect(() => {
    void api.getUiPrefs().then((prefs) => {
      if (prefs["editor.saveMode"] === "manual") setSaveMode("manual");
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const changeSaveMode = useCallback(
    (mode: "auto" | "manual") => {
      setSaveMode(mode);
      api.setUiPrefs({ "editor.saveMode": mode });
    },
    [api]
  );
  // 任务完成提示音（§7.5 v1.122）：偏好存 daemon ui_prefs（§7.5 权威，键 sound.done，
  // 默认开），即时切换；非视觉偏好不入 localStorage（无闪烁问题，与保存方式同法）。
  const [soundDone, setSoundDone] = useState(true);
  useEffect(() => {
    void api.getUiPrefs().then((prefs) => {
      if (prefs["sound.done"] === "off") setSoundDone(false);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const changeSoundDone = useCallback(
    (on: boolean) => {
      setSoundDone(on);
      api.setUiPrefs({ "sound.done": on ? "on" : "off" });
    },
    [api]
  );
  // 命令面板撤销/重做入口（§8.2 v1.75）：EditorPane 挂载时绑定 active editor。
  const editorApiRef = useRef<{ undo: () => void; redo: () => void } | null>(null);

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
  // 编辑器应用内浮层（v1.110）：单击文件 / 模糊打开 / 搜索 / 诊断 / 跟随模式
  // 打开文件即弹出；✕ 关闭返回线程；会话内状态不持久化（重启后不自动弹出）。
  const [editorOpen, setEditorOpen] = useState(false);
  const [sideView, setSideView] = useState<SideView>(loadSideView);
  // 视口自适应（§7.2 v1.74）：三档布局；narrow 下侧栏 / 代理面板转互斥浮层。
  const viewport = useViewport();
  const band = viewport.band;
  const narrow = band === "narrow";
  /** narrow 侧栏浮层（v1.78；v1.110 编辑器改应用内浮层，仅剩侧栏浮层）：
   * 仅窄屏有意义，不写持久化状态（sidebarOpen / ui-state 不被窄屏污染）。 */
  const [sideFloat, setSideFloat] = useState(false);
  // 跨档位沿：离开 narrow 清空浮层；进入 narrow 线程在流可见，无默认浮层。
  const bandRef = useRef(band);
  useEffect(() => {
    if (bandRef.current === band) return;
    bandRef.current = band;
    setSideFloat(false);
  }, [band]);
  /** rail 点击语义：同视图再点 = 折叠侧栏；否则切换视图并展开。 */
  const toggleSideView = useCallback(
    (view: SideView) => {
      if (narrow) {
        if (sideFloat && sideView === view) {
          setSideFloat(false);
          return;
        }
        setSideView(view);
        localStorage.setItem(SIDE_VIEW_KEY, view);
        setSideFloat(true);
        return;
      }
      if (sidebarOpen && sideView === view) {
        setSidebarOpen(false);
        return;
      }
      setSideView(view);
      localStorage.setItem(SIDE_VIEW_KEY, view);
      setSidebarOpen(true);
    },
    [narrow, sideFloat, sideView, sidebarOpen]
  );
  const [agentState, setAgentState] = useState<AgentStateName>("idle");
  const [routeNote, setRouteNote] = useState<string | null>(null);
  // 底部面板 tab（v1.107 移除「源码」——Git 视图迁右区源码树）：枚举收敛为三项。
  const [bottomTab, setBottomTab] = useState<"timeline" | "trace" | "evals">(
    "timeline"
  );
  // 可调布局（§7.2：两区可折叠可调宽；localStorage 记忆；v1.110 移除右栏；v1.115 默认宽收窄 220→180）
  const [leftWidth, setLeftWidth] = useState(() => Number(localStorage.getItem("tenon:leftWidth")) || 180);
  const [bottomHeight, setBottomHeight] = useState(() => Number(localStorage.getItem("tenon:bottomHeight")) || 180);

  const tabs = projectId ? tabsByProject[projectId] ?? [] : [];
  const activePath = projectId ? activePathByProject[projectId] ?? null : null;
  const splitPath = projectId ? splitPathByProject[projectId] ?? null : null;
  const sessionId = projectId ? sessionsByProject[projectId] ?? null : null;

  // 渲染期尺寸 clamp（§7.2 v1.74）：只作用渲染，记忆值与项目 ui-state 不改写。
  const effLeft = effectiveLeft(leftWidth, viewport.width);
  const effBottom = effectiveBottom(bottomHeight, viewport.height);
  // narrow 侧栏浮层可见性（v1.78）。
  const sideVisible = narrow ? sideFloat : sidebarOpen;
  // v1.110：浮层内最后一个 tab 关闭（关闭按钮 / WS removed / 文件树删除）时随之收起；
  // 打开瞬间的空 tab 窗口（readFile 未返回）不算关闭，防止浮层刚弹即被收。
  const editorHadTabRef = useRef(false);
  useEffect(() => {
    if (!editorOpen) {
      editorHadTabRef.current = false;
      return;
    }
    if (tabs.length > 0) {
      editorHadTabRef.current = true;
    } else if (editorHadTabRef.current) {
      setEditorOpen(false);
    }
  }, [editorOpen, tabs.length]);

  useEffect(() => {
    tabsByProjectRef.current = tabsByProject;
    activePathByProjectRef.current = activePathByProject;
    splitPathByProjectRef.current = splitPathByProject;
    unsavedRef.current = unsaved;
  }, [tabsByProject, activePathByProject, splitPathByProject, unsaved]);

  const refreshProjects = useCallback(async () => {
    const r = await api.listProjects();
    setProjects(r.projects);
    setSessionsByProject((prev) => {
      const next = { ...prev };
      for (const project of r.projects) {
        // 草稿态项目不参与默认选会话兜底（v1.116）：轮询不得夺走未发送的草稿任务；
        // 兜底也只选已开始（有标题）的会话——无标题会话从未开始，不作激活线程。
        if (!next[project.id] && draftByProjectRef.current[project.id] === undefined) {
          const persisted = project.sessions.find((s) => (s.title ?? "").trim() !== "")?.id;
          if (persisted) next[project.id] = persisted;
        }
      }
      return next;
    });
    return r.projects;
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
      if (typeof saved.bottomHeight === "number") {
        setBottomHeight(Math.min(480, Math.max(80, saved.bottomHeight)));
      }
      if (typeof saved.sidebarOpen === "boolean") setSidebarOpen(saved.sidebarOpen);
      if (typeof saved.timelineOpen === "boolean") setTimelineOpen(saved.timelineOpen);
      // v1.107 枚举删 source：旧 ui-state 残留值不匹配任何分支，回退默认 timeline。
      if (
        saved.bottomTab === "timeline" ||
        saved.bottomTab === "trace" ||
        saved.bottomTab === "evals"
      ) {
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
      const savedSplit =
        saved.splitPath && restored.some((tab) => tab.path === saved.splitPath)
          ? saved.splitPath
          : null;
      setSplitPathByProject((prev) => ({ ...prev, [project.id]: savedSplit }));

      const savedSession =
        saved.sessionId && project.session_runtimes?.includes(saved.sessionId)
          ? saved.sessionId
          : undefined;
      // 复用优先级：未发送首条消息的草稿任务（v1.116）→ ui-state 持久会话
      // （须有 runtime）→ 项目内任一活跃 runtime 会话 → 草稿态。
      // DB 历史会话无 runtime 不可直接复用。
      const existing =
        savedSession ?? project.sessions.find((s) => project.session_runtimes?.includes(s.id))?.id;
      if (draftByProjectRef.current[project.id] !== undefined) {
        // 保持草稿态：跨项目往返不丢未发送的草稿任务。
      } else if (existing) {
        setSessionsByProject((prev) => ({ ...prev, [project.id]: existing }));
      } else {
        // v1.116：无既有会话不再急切新建（避免产生从未开始的空会话），
        // 进入草稿态，首条消息发出时才建会话；boot 期轮询兜底可能已选中
        // 无标题会话，一并清掉（线程与任务列表保持「未开始」口径一致）。
        setSessionsByProject((prev) => {
          if (!(project.id in prev)) return prev;
          const next = { ...prev };
          delete next[project.id];
          return next;
        });
        setDraftByProject((prev) => ({ ...prev, [project.id]: prev[project.id] ?? false }));
      }
      projectUiStateLoaded.current.add(project.id);
    },
    [api]
  );

  const setActivePath = useCallback((path: string | null) => {
    if (!projectId) return;
    setActivePathByProject((prev) => ({ ...prev, [projectId]: path }));
  }, [projectId]);

  const setSplitPath = useCallback((path: string | null) => {
    if (!projectId) return;
    setSplitPathByProject((prev) => ({ ...prev, [projectId]: path }));
  }, [projectId]);

  // 右栏不能与主编辑器相同；主编辑器切到原右栏文件时自动换右栏。
  useEffect(() => {
    if (!projectId || !splitPath || splitPath !== activePath) return;
    const next = tabs.find((tab) => tab.path !== activePath)?.path ?? null;
    setSplitPathByProject((prev) => ({ ...prev, [projectId]: next }));
  }, [projectId, splitPath, activePath, tabs]);

  const setTabs = useCallback((updater: (prev: EditorTab[]) => EditorTab[]) => {
    if (!projectId) return;
    setTabsByProject((prev) => ({ ...prev, [projectId]: updater(prev[projectId] ?? []) }));
  }, [projectId]);

  /** 「对话」组点击：切换项目内激活会话（§7.5）；选择既有会话即弃草稿（v1.116）。 */
  const selectSession = useCallback((pid: string, sid: string) => {
    setDraftByProject((prev) => {
      if (!(pid in prev)) return prev;
      const next = { ...prev };
      delete next[pid];
      return next;
    });
    setSessionsByProject((prev) => ({ ...prev, [pid]: sid }));
  }, []);

  /** 新建会话入口（v1.87 §7.3；v1.116 草稿态）：不再立即建会话——记录待启动意图
   *  （主根 / 受管 worktree），线程切空任务输入；首条消息发出时才落库建会话。
   *  受管 worktree 会话可与主根并行执行的语义不变（§7.3）。 */
  const createProjectSession = useCallback(
    async (project: ProjectSummary, worktree: boolean) => {
      if (project.id !== projectIdRef.current) await activateProject(project);
      setSessionsByProject((prev) => {
        if (!(project.id in prev)) return prev;
        const next = { ...prev };
        delete next[project.id];
        return next;
      });
      setDraftByProject((prev) => ({ ...prev, [project.id]: worktree }));
    },
    [activateProject]
  );

  /** 草稿任务首发（v1.116）：此刻才建会话（按意图附 worktree）并发送首条消息；
   *  provider 留空由 daemon 取默认（v1.97 断链修复语义随迁）。 */
  const sendDraftMessage = useCallback(
    async (text: string) => {
      const pid = projectIdRef.current;
      if (!pid) return;
      const worktree = draftByProjectRef.current[pid] === true;
      // v1.92 移除会话默认档后此处不传档位；未知 provider 会使建会话恒失败。
      const session = await api.createSession(pid, "", worktree ? "managed" : undefined);
      setDraftByProject((prev) => {
        if (!(pid in prev)) return prev;
        const next = { ...prev };
        delete next[pid];
        return next;
      });
      setSessionsByProject((prev) => ({ ...prev, [pid]: session.session_id }));
      // 先刷新项目摘要（runtime / 会话就绪）再发消息；会话行待标题生成后才进列表。
      await refreshProjects();
      await api.sendMessage(session.session_id, text);
    },
    [api, refreshProjects]
  );

  // 打开项目 + 建会话（§7.3：v1.67 打开即静默信任，不再弹 TOFU 确认卡）
  const openProject = useCallback(async (path: string, displayName?: string) => {
    const opened = await api.openProject(path, displayName);
    // TOFU（§12.7）：打开即登记信任元数据；v1.89 不再设置执行门槛。
    if (!opened.trusted) await api.setTrust(opened.id, true);
    const refreshed = await refreshProjects();
    const summary =
      refreshed.find((project) => project.id === opened.id) ??
      ({
        ...opened,
        sessions: [],
        active_sessions: 0,
        dirty_buffers: 0,
        usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
      } satisfies ProjectSummary);
    await activateProject(summary);
  }, [activateProject, api, refreshProjects]);

  /** 移除登记不删盘；daemon 拒绝仍被会话引用的项目。 */
  const removeProject = useCallback(
    async (project: ProjectSummary) => {
      try {
        await api.removeProject(project.id);
        await refreshProjects();
        if (projectIdRef.current === project.id) {
          setProjectId(null);
          projectIdRef.current = null;
        }
      } catch (error) {
        setOpenError(String(error));
      }
    },
    [api, refreshProjects]
  );

  /** T4 一键修复：诊断详情转成机器可验证任务并注入当前项目会话。 */
  const fixDiagnostic = useCallback(
    (diagnostic: EditorDiagnostic) => {
      setInjectedTask({
        token: Date.now(),
        text: [
          t("diagnostic.fix_head", {
            path: diagnostic.path,
            line: diagnostic.line,
            column: diagnostic.column,
          }),
          t("diagnostic.fix_info", { message: diagnostic.message }),
          t("diagnostic.fix_constraint"),
        ].join(" "),
      });
    },
    [t]
  );

  /** 行内指令（§8.5 / S2 / T8）：选区上下文组装后直接发送当前项目会话。 */
  const sendInline = useCallback(
    (instruction: string, target: InlineTarget) => {
      const sid = sessionId;
      if (!sid) return;
      void api.sendMessage(sid, buildInlineTask(instruction, target, t)).catch(() => {});
    },
    [api, sessionId, t]
  );

  const switchProject = useCallback(async (project: ProjectSummary) => {
    await activateProject(project);
  }, [activateProject]);

  const openFile = useCallback(
    async (path: string, line?: number, opts?: { fromFollow?: boolean }) => {
      // 打开文件即弹出编辑器浮层（v1.110，与视口无关）；
      // 跟随模式不抢屏（v1.113）：代理写入只就绪数据，浮层由用户主动打开。
      if (!opts?.fromFollow) setEditorOpen(true);
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

  /** 统一保存（§8.2 v1.75）：待写盘条目走 AutoSaver flush，否则未保存缓冲直接写盘。 */
  const saveNow = useCallback(
    (path: string) =>
      saveNowBuffered(path, {
        autosaver: autosaverRef.current,
        isUnsaved: (p) => Boolean(unsavedRef.current[p]),
        getContent: (p) => {
          const pid = projectIdRef.current;
          if (!pid) return null;
          return (
            (tabsByProjectRef.current[pid] ?? []).find((tab) => tab.path === p)?.content ?? null
          );
        },
        write: async (p, content) => {
          const pid = projectIdRef.current;
          if (!pid) throw new Error("no active project");
          await api.writeFile(pid, p, content);
          // 落盘成功 → 脏缓冲解除（§8.6「未保存缓冲」语义：已保存不再是缓冲）
          await api.clearBuffer(pid, p).catch(() => {});
        },
        onSaved: (p) =>
          setUnsaved((prev) => {
            if (!prev[p]) return prev;
            const next = { ...prev };
            delete next[p];
            return next;
          }),
      }),
    [api]
  );

  /** LSP 写盘前 flush 未保存缓冲（§8.5 / v1.48）：与手动保存同路径。 */
  const flushFileForLsp = useCallback((path: string) => saveNow(path), [saveNow]);

  const refreshFilesAfterLsp = useCallback(
    async (paths: string[]) => {
      const pid = projectIdRef.current;
      if (!pid) return;
      for (const path of paths) {
        try {
          const file = await api.readFile(pid, path);
          setTabsByProject((prev) => ({
            ...prev,
            [pid]: (prev[pid] ?? []).some((tab) => tab.path === path)
              ? prev[pid].map((tab) =>
                  tab.path === path ? { ...tab, content: file.content } : tab
                )
              : prev[pid],
          }));
          setUnsaved((prev) => {
            if (!prev[path]) return prev;
            const next = { ...prev };
            delete next[path];
            return next;
          });
        } catch {
          // 文件可能被 rename；watcher / 文件树会同步。
        }
      }
      setFileTreeVersion((version) => version + 1);
    },
    [api]
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
        setSplitPathByProject((prev) =>
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
      const nextSplit =
        splitPathByProjectRef.current[projectId] === change.path
          ? (tabs.find((tab) => tab.path !== change.path)?.path ?? null)
          : splitPathByProjectRef.current[projectId];
      setSplitPathByProject((prev) => ({ ...prev, [projectId]: nextSplit ?? null }));
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

  // 任务完成提示音触发（§7.5 v1.122）：AgentPanel 每 500ms 轮询上报状态，
  // prev→done 转迁即响。偏好与上一态经 ref 读取，回调保持空依赖——身份变化
  // 会重启 AgentPanel 的轮询 effect。会话切换时重置上一态：新会话的既有
  // 状态不算「刚完成」，防止切到已完成会话误响。
  const prevAgentStateRef = useRef<AgentStateName>("idle");
  const soundDoneRef = useRef(soundDone);
  soundDoneRef.current = soundDone;
  useEffect(() => {
    prevAgentStateRef.current = "idle";
  }, [sessionId]);
  const onStateChange = useCallback((s: AgentStateName) => {
    setAgentState(s);
    const prev = prevAgentStateRef.current;
    prevAgentStateRef.current = s;
    if (soundDoneRef.current && shouldChimeOnTransition(prev, s)) void playDoneChime();
  }, []);
  const handlers = useMemo(
    () => ({
      onPalette: () => setPaletteOpen((v) => !v),
      onGotoFile: () => setFinderOpen(true),
      onSidebar: () => setSidebarOpen((v) => !v),
      onPanel: () => setTimelineOpen((v) => !v),
    onInlineInstruction: () => {
        if (activePath) setInlineOpen(true);
      },
      onStop: () => sessionId && api.control(sessionId, "stop"),
      onSave: () => {
        const p = activePath;
        if (p) void saveNow(p);
      },
      onSettings: () => setSettingsOpen(true),
      onPauseOrClose: () => {
        if (paletteOpen) setPaletteOpen(false);
        else if (sessionId) api.control(sessionId, "pause");
      },
    }),
    [api, sessionId, paletteOpen, activePath, saveNow]
  );
  useShortcuts(handlers);

  const commands: Command[] = useMemo(
    () => [
      { id: "toggle.bottom", label: t("panel.bottom.toggle"), run: () => setTimelineOpen((v) => !v) },
      { id: "open.settings", label: t("settings.open"), run: () => setSettingsOpen(true) },
      {
        id: "editor.inline_completion",
        label: inlineCompletionEnabled
          ? t("inline.disable")
          : t("inline.enable"),
        run: toggleInlineCompletion,
      },
      {
        id: "editor.undo",
        label: t("editor.undo"),
        run: () => editorApiRef.current?.undo(),
      },
      {
        id: "editor.redo",
        label: t("editor.redo"),
        run: () => editorApiRef.current?.redo(),
      },
      { id: "toggle.sidebar", label: t("palette.toggle_sidebar"), run: () => setSidebarOpen((v) => !v) },
      // v1.110：toggle.source = 编辑器浮层开合（源码区=侧栏源码视图 + 浮层编辑器）。
      { id: "toggle.source", label: t("palette.toggle_source"), run: () => setEditorOpen((v) => !v) },
      // v1.122：任务完成提示音开合（ui_prefs sound.done）；标签显动作语义（与 inline_completion 同法）。
      {
        id: "toggle.sound_done",
        label: soundDone ? t("palette.sound_done_off") : t("palette.sound_done_on"),
        run: () => changeSoundDone(!soundDone),
      },
      ...(agentState === "paused"
        ? [
            {
              id: "agent.resume",
              label: t("message.resume"),
              run: () => sessionId && api.control(sessionId, "resume"),
            },
          ]
        : [
            {
              id: "agent.pause",
              label: t("message.pause"),
              run: () => sessionId && api.control(sessionId, "pause"),
            },
          ]),
      {
        id: "agent.readonly_on",
        label: t("palette.readonly_on"),
        run: () => sessionId && api.control(sessionId, "set_readonly", true),
      },
      {
        id: "agent.readonly_off",
        label: t("palette.readonly_off"),
        run: () => sessionId && api.control(sessionId, "set_readonly", false),
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
    [t, api, sessionId, agentState, inlineCompletionEnabled, toggleInlineCompletion, soundDone, changeSoundDone]
  );

  // 底部面板开合（v1.61）：展开态 tabs 行右端收起、收起态细条展开；标签与 tab 按钮共用一份
  const bottomTabTitles: Record<typeof bottomTab, string> = {
    timeline: t("panel.timeline"),
    trace: t("panel.trace"),
    evals: t("panel.evals"),
  };

  // M0：挂载即自动打开项目并建会话（M1 换项目选择页 + TOFU 卡）
  const [openError, setOpenError] = useState<string | null>(null);
  const autoOpenRef = useRef(false);
  useEffect(() => {
    // 守卫：openProject 依赖 projects（refreshProjects 后重建），不加守卫会
    // 无限循环重开 + 反复弹信任确认（E2E 实测缺陷，v1.30 修复）。
    if (autoOpenRef.current) return;
    autoOpenRef.current = true;
    openProject(projectPath).catch((e) => {
      setOpenError(String(e));
    });
  }, [openProject, projectPath]);
  useEffect(() => {
    const id = window.setInterval(() => {
      void refreshProjects().catch(() => {});
    }, 2000);
    return () => window.clearInterval(id);
  }, [refreshProjects]);
  // 项目级 UI 状态去抖持久化（§7.2 / §7.5）：加载完成后才允许覆盖远端。
  useEffect(() => {
    if (!projectId || !projectUiStateLoaded.current.has(projectId)) return;
    const timer = window.setTimeout(() => {
      api.saveProjectUiState(projectId, {
        sessionId: sessionsByProject[projectId],
        tabs: (tabsByProject[projectId] ?? []).map((tab) => tab.path),
        activePath: activePathByProject[projectId] ?? null,
        splitPath: splitPathByProject[projectId] ?? null,
        leftWidth,
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
    splitPathByProject,
    leftWidth,
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
        session_id?: string;
        path?: string;
        type?: string;
        payload?: { title?: string };
      };
      // 对话标题生成完成（v1.58）：本地即时更新会话行，轮询刷新兜底。
      if (!alive) return;
      if (event.type === "session_title" && event.session_id) {
        const title = event.payload?.title ?? "";
        setProjects((prev) =>
          prev.map((project) => ({
            ...project,
            sessions: project.sessions.map((session) =>
              session.id === event.session_id ? { ...session, title } : session
            ),
          }))
        );
        return;
      }
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
      <header className="app-head" data-tauri-drag-region>
        <strong data-tauri-drag-region>{t("app.title")}</strong>
        <span className="spacer" data-tauri-drag-region />
        <ThemePicker api={api} t={t} />
        <LanguagePicker value={localePref} t={t} />
      </header>
      {routeNote && (
        <div className="route-note" data-testid="route-note">
          {routeNote}
        </div>
      )}
      <div
        className={`workspace${narrow ? " compact" : ""}`}
        data-band={band}
        data-testid="workspace"
      >
        <nav className="activity-rail" aria-label={t("rail.label")}>
          {/* v1.120：v1.106 的 rail 顶部切换钮已删——侧栏开合收敛 rail 同视图再点 / 命令面板 toggle.sidebar。 */}
          <button
            type="button"
            className={sidebarOpen && sideView === "projects" ? "rail-btn active" : "rail-btn"}
            data-testid="rail-projects"
            title={t("panel.projects")}
            aria-label={t("panel.projects")}
            aria-pressed={sidebarOpen && sideView === "projects"}
            onClick={() => toggleSideView("projects")}
          >
            <RailIcon view="projects" />
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
          <button
            type="button"
            className="rail-btn"
            data-testid="rail-settings"
            title={t("settings.open")}
            aria-label={t("settings.open")}
            onClick={() => setSettingsOpen(true)}
          >
            <svg
              width={17}
              height={17}
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={1.8}
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden
            >
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
            </svg>
          </button>
        </nav>
        {sideVisible && (
          <>
            <aside
              className={`zone zone-left${narrow ? " zone-float" : ""}`}
              style={
                narrow
                  ? { width: effectiveFloatWidth(leftWidth, viewport.width) }
                  : { width: effLeft, minWidth: 140, maxWidth: 480 }
              }
            >
              {/* projects 视图标题行由 ProjectExplorer 自渲染（含「+」，v1.101），通用标题仅其余视图需要；
                  v1.117：side-head 收起钮移除——侧栏开合收敛 rail 切换钮 / rail 同视图再点 / 命令面板 */}
              {sideView !== "projects" && (
                <div className="side-head">
                  <span className="side-title">
                    {sideView === "search"
                      ? t("search.title")
                      : t("panel.packs")}
                  </span>
                </div>
              )}
              <div className="side-body">
                {sideView === "projects" && (
                  <ProjectExplorer
                    api={api}
                    t={t}
                    projects={projects}
                    projectId={projectId}
                    sessionsByProject={sessionsByProject}
                    openError={openError}
                    refreshToken={fileTreeVersion}
                    onSwitchProject={(project) => void switchProject(project)}
                    onOpenProject={(path, displayName) =>
                      openProject(path, displayName).catch((error) => setOpenError(String(error)))}
                    onRemoveProject={(project) => removeProject(project)}
                    onSelectSession={selectSession}
                    onCreateSession={(project, worktree) =>
                      void createProjectSession(project, worktree).catch((error) =>
                        setOpenError(String(error))
                      )}
                    onRefreshProjects={() => void refreshProjects()}
                    onSessionRemoved={(removedProjectId, removedSessionId) => {
                      setSessionsByProject((prev) => {
                        if (prev[removedProjectId] !== removedSessionId) return prev;
                        const next = { ...prev };
                        delete next[removedProjectId];
                        return next;
                      });
                    }}
                    onOpenFile={openFile}
                    onFileTreeChange={handleFileTreeChange}
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
                  <LanguagePackWizard api={api} projectId={projectId} t={t} />
                )}
              </div>
            </aside>
            {!narrow && (
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
            )}
          </>
        )}
        {narrow && sideFloat && (
          <div
            className="float-backdrop"
            data-testid="float-backdrop"
            onClick={() => setSideFloat(false)}
          />
        )}
        {/* v1.78 复刻 Codex 形态（§7.2）：线程（代理会话）恒为弹性主区。 */}
        <section className="zone zone-thread" style={{ flex: 1, minWidth: 260 }}>
          <AgentPanel
            api={api}
            t={t}
            sessionId={sessionId}
            draft={projectId ? draftByProject[projectId] !== undefined : false}
            onDraftSend={sendDraftMessage}
            onStateChange={onStateChange}
            onLatestDiff={setLatestDiff}
            onPatchLines={(path, lines) => {
              setAiLines((prev) => ({
                ...prev,
                [path]: unionLines(prev[path] ?? [], lines),
              }));
              // 跟随模式（§8.5）：代理写入时自动打开/滚动到首个改动行（可关）。
              if (followModeRef.current && lines.length > 0) {
                // 跟随模式（§8.5 / v1.113）：数据就绪（tab + AI 行 + 定位）但不弹浮层抢屏；
                // 浮层已开时 gotoLine 照常定位到首个改动行。
                void openFile(path, lines[0], { fromFollow: true });
              }
            }}
            onDirtyConflict={setDirtyConflict}
            followMode={followMode}
            onToggleFollow={toggleFollow}
            injectedTask={injectedTask ?? undefined}
            onModelSwitched={(m) => {
              // 切换提示（§11：上下文随迁，model_fallback 事件入 Trace）
              setRouteNote(t("model.switched", { model: m }));
              window.setTimeout(() => setRouteNote(null), 4000);
            }}
          />
        </section>
        {/* v1.110：编辑器应用内浮层——单击文件 / 模糊打开 / 搜索 / 诊断 / 跟随模式
            打开文件即弹出，✕ 关闭返回线程；EditorPane 多标签 / 分栏语义整体迁入。 */}
        {editorOpen && (
        <div className="editor-overlay" data-testid="editor-overlay">
          <div className="editor-overlay-head">
            <span className="side-title">{t("panel.editor")}</span>
            <button
              type="button"
              className="pe-collapse"
              data-testid="editor-overlay-close"
              title={t("editor.close")}
              aria-label={t("editor.close")}
              onClick={() => setEditorOpen(false)}
            >
              <svg width={12} height={12} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M18 6 6 18" />
                <path d="m6 6 12 12" />
              </svg>
            </button>
          </div>
          <div className="editor-overlay-body">
              <EditorPane
            t={t}
            api={api}
            projectId={projectId}
            projectRoot={projects.find((project) => project.id === projectId)?.path}
            sessionId={sessionId}
            inlineCompletionEnabled={inlineCompletionEnabled}
            refreshToken={fileTreeVersion}
            tabs={tabs}
            activePath={activePath}
            splitPath={splitPath}
            onSelectSplit={setSplitPath}
            aiModifiedLines={aiLines}
            unsavedPaths={unsaved}
            unsavedTitle={t("editor.unsaved")}
            goto={gotoLine}
            onSelectionChange={setSelection}
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
              if (splitPath === p) {
                setSplitPath(tabs.find((tab) => tab.path !== p && tab.path !== activePath)?.path ?? null);
              }
            }}
            onChange={(p, content) => {
              setTabs((prev) => prev.map((tab) => (tab.path === p ? { ...tab, content } : tab)));
              // §8.6：用户编辑 → 该文件 AI 角标解除 + 脏缓冲推送（去抖）
              setAiLines((prev) => ({ ...prev, [p]: [] }));
              // 保存模式门控（§8.2 v1.75）：手动模式下不调度去抖写盘，
              // 由 Cmd/Ctrl+S / LSP flush 经 saveNow 显式保存。
              if (saveMode === "auto") autosaverRef.current?.schedule(p, content);
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
            onFlushFile={flushFileForLsp}
            onWorkspaceApplied={refreshFilesAfterLsp}
            bindEditorApi={(editorApi) => {
              editorApiRef.current = editorApi;
            }}
          />
          </div>
        </div>
        )}
      </div>
      {dirtyConflict && (
        <div className="merge-overlay">
          <ThreePaneMerge
            conflict={dirtyConflict}
            labels={{
              title: t("merge.title"),
              conflictNote: t("merge.conflict_note"),
              ours: t("merge.ours"),
              theirs: t("merge.theirs"),
              base: t("merge.base"),
              apply: t("merge.apply"),
              keepMine: t("merge.keep_mine"),
              useAgent: t("merge.use_agent"),
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
      {!timelineOpen && (
        <footer className="bottom-collapsed">
          <button
            type="button"
            className="bottom-toggle"
            data-testid="bottom-open"
            title={t("panel.bottom.open")}
            aria-label={t("panel.bottom.open")}
            onClick={() => setTimelineOpen(true)}
          >
            <span aria-hidden="true">▴</span>
            {bottomTabTitles[bottomTab]}
          </button>
        </footer>
      )}
      {timelineOpen && (
        <footer className="zone-bottom" style={{ height: effBottom }}>
          <ResizeHandle dir="vertical" onResize={(d) =>
            setBottomHeight((h) => {
              const v = Math.min(480, Math.max(80, h - d));
              localStorage.setItem("tenon:bottomHeight", String(v));
              return v;
            })
          } />
          <div className="bottom-head">
            <div className="bottom-tabs">
              <button
                className={bottomTab === "timeline" ? "active" : ""}
                onClick={() => setBottomTab("timeline")}
              >
                {bottomTabTitles.timeline}
              </button>
              <button
                className={bottomTab === "trace" ? "active" : ""}
                onClick={() => setBottomTab("trace")}
                data-testid="tab-trace"
              >
                {bottomTabTitles.trace}
              </button>
              <button
                className={bottomTab === "evals" ? "active" : ""}
                onClick={() => setBottomTab("evals")}
                data-testid="tab-evals"
              >
                {bottomTabTitles.evals}
              </button>
            </div>
            <button
              type="button"
              className="bottom-toggle"
              data-testid="bottom-close"
              title={t("panel.bottom.close")}
              aria-label={t("panel.bottom.close")}
              onClick={() => setTimelineOpen(false)}
            >
              <span aria-hidden="true">▾</span>
            </button>
          </div>
          {bottomTab === "timeline" && (
            <div className="bottom-grid">
              <CheckpointTimeline api={api} t={t} sessionId={sessionId} />
              <DiagnosticsPanel
                api={api}
                t={t}
                projectId={projectId}
                path={activePath}
                refreshToken={fileTreeVersion}
                sessionId={sessionId}
                onOpenFile={(path, line) => void openFile(path, line)}
                onFix={fixDiagnostic}
              />
              <DiffPanel diff={latestDiff} title={t("panel.patch_diff")} emptyText={t("diff.no_changes")} />
              <L4StatusPanel api={api} t={t} projectId={projectId} />
            </div>
          )}
          {bottomTab === "trace" && (
            <AgentTracePanel api={api} sessionId={sessionId} t={t} />
          )}
          {bottomTab === "evals" && <EvalsPanel api={api} t={t} />}
        </footer>
      )}
      <InlineInstruction
        open={inlineOpen}
        selection={selection}
        activePath={activePath}
        t={t}
        onClose={() => setInlineOpen(false)}
        onSend={sendInline}
      />
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        commands={commands}
        t={t}
      />
      {settingsOpen && (
        <SettingsDialog
          api={api}
          t={t}
          settings={settings}
          saveMode={saveMode}
          onSaveModeChange={changeSaveMode}
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

function LanguagePicker({ value, t }: { value: Locale; t: Translate }) {
  const [locale, setLocaleState] = useState<Locale>(value);
  // 父级偏好变化（含 localStorage 恢复）同步回显
  useEffect(() => setLocaleState(value), [value]);
  return (
    <select
      aria-label={t("topbar.language")}
      value={locale}
      onChange={(e) => {
        const v = e.target.value as Locale;
        setLocaleState(v);
        window.dispatchEvent(new CustomEvent(LOCALE_CHANGE, { detail: v }));
      }}
    >
      <option value="auto">{t("language.auto")}</option>
      {/* 语言名以各自母语显示（i18n 惯例），不经 t()；清单由 LOCALES 注册表驱动（v1.100） */}
      {LOCALES.map((l) => (
        <option key={l.tag} value={l.tag}>
          {l.nativeName}
        </option>
      ))}
    </select>
  );
}

/** 外观档（§7.5）：深色 / 浅色 / 跟随系统；即时生效 + 双写
 *  localStorage（快路径）与 daemon /ui-prefs（跨启动权威，端口动态
 *  导致 localStorage 按 origin 隔离不可依赖）。
 *  v1.119：下拉 select 改单图标按钮三态循环（跟随系统 → 浅色 → 深色）。 */
const THEME_CYCLE: ThemePreference[] = ["system", "light", "dark"];

function ThemeIcon({ pref }: { pref: ThemePreference }) {
  const svg = {
    width: 14,
    height: 14,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 2,
    strokeLinecap: "round",
    strokeLinejoin: "round",
    "aria-hidden": true,
  } as const;
  if (pref === "system") {
    return (
      <svg {...svg}>
        <rect x="2" y="3" width="20" height="14" rx="2" />
        <path d="M8 21h8M12 17v4" />
      </svg>
    );
  }
  if (pref === "light") {
    return (
      <svg {...svg}>
        <circle cx="12" cy="12" r="4" />
        <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
      </svg>
    );
  }
  return (
    <svg {...svg}>
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
    </svg>
  );
}

function ThemePicker({ api, t }: { api: TenonApi; t: Translate }) {
  const [pref, setPref] = useState<ThemePreference>(() => loadThemePreference());
  // 跟随系统档：系统深浅切换时重应用
  useEffect(() => watchSystemTheme(() => applyTheme(pref)), [pref]);
  const setTheme = (v: ThemePreference) => {
    setPref(v);
    saveThemePreference(v);
    applyTheme(v);
    api.setUiPrefs({ theme: v });
  };
  // 钮面图标示当前档，点击推进下一档（末端回绕）；label 合成当前档名供读屏与悬停
  const label = `${t("topbar.theme")}: ${t(`theme.${pref}`)}`;
  return (
    <button
      type="button"
      className="theme-btn"
      aria-label={label}
      title={label}
      data-testid="theme-picker"
      data-theme-pref={pref}
      onClick={() => setTheme(THEME_CYCLE[(THEME_CYCLE.indexOf(pref) + 1) % THEME_CYCLE.length])}
    >
      <ThemeIcon pref={pref} />
    </button>
  );
}
