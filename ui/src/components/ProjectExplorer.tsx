// 项目浏览器（左侧栏「项目」视图，v1.88 对齐 Codex projects sidebar 内容布局）：
// 顶部搜索 + 添加入口，Name / Updated 列头统一项目索引密度；每项目一行可折叠文件夹
// （点击行即隐式激活并展开），默认内嵌最近 10 条会话；行尾「源码」按钮切换文件树。
// 「All activity」为同构列表组，承接跨项目监控但不占据仪表盘式首屏。
import { useEffect, useState } from "react";
import type { ProjectSummary, TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { RUNNING_STATES } from "../lib/stateColors";
import { FileTree, type FileTreeChange } from "./FileTree";

const PE_EXPANDED_KEY = "tenon:peExpanded";
const PE_FILES_KEY = "tenon:peFiles";
/** Codex 项目行展开后默认只物化最近会话，长列表显式展开。 */
const RECENT_SESSION_LIMIT = 10;

function loadSet(key: string): Set<string> {
  try {
    const raw = localStorage.getItem(key);
    return new Set(raw ? (JSON.parse(raw) as string[]) : []);
  } catch {
    return new Set();
  }
}

function persistSet(key: string, value: Set<string>) {
  try {
    localStorage.setItem(key, JSON.stringify([...value]));
  } catch {
    // 存储不可用时仅当次会话内生效
  }
}

interface Props {
  api: TenonApi;
  t: Translate;
  projects: ProjectSummary[];
  projectId: string | null;
  /** 各项目当前激活会话（§7.5 项目级 UI 状态）。 */
  sessionsByProject: Record<string, string>;
  openError: string | null;
  /** ProjectRuntime 文件事件版本；透传文件树增量刷新（§6.4 / §8.1）。 */
  refreshToken: number;
  onSwitchProject: (project: ProjectSummary) => void;
  /** displayName 提供时落库为项目显示名；空 / 缺省回退路径末段（§6.4）。 */
  onOpenProject: (path: string, displayName?: string) => Promise<void> | void;
  onRemoveProject: (project: ProjectSummary) => Promise<void> | void;
  onSelectSession: (projectId: string, sessionId: string) => void;
  /** 新建会话（v1.87）：worktree=true 创建受管 worktree 会话，可与主根并行。 */
  onCreateSession: (project: ProjectSummary, worktree: boolean) => void;
  /** 受管 worktree 合并 / 丢弃后刷新项目摘要（会话状态与文件树）。 */
  onRefreshProjects?: () => void;
  onOpenFile: (path: string) => void;
  onFileTreeChange: (change: FileTreeChange) => void;
}

const RUNNING = RUNNING_STATES as ReadonlySet<string>;
const ACTIVITY_FILTERS = ["all", "running", "done"] as const;
type ActivityFilter = (typeof ACTIVITY_FILTERS)[number];

/** 状态点配色（§7.5 状态色）：全局活动行与汇总条共用。 */
function statusDotColor(status: string): string {
  return (
    ({
      sensing: "#2f6fed",
      deciding: "#5b6b7a",
      executing: "#d9a514",
      verifying: "#7d4fd3",
      fixing: "#7d4fd3",
      paused: "#8a8f98",
      error: "#d43d3d",
      done: "#2da44e",
      rolled_back: "#8a8f98",
      idle: "#8a8f98",
    } as Record<string, string>)[status] ?? "#8a8f98"
  );
}

/** 状态文案：优先使用 state.* 翻译，缺失回退原始状态。 */
function stateLabel(t: Translate, status: string) {
  const key = `state.${status}`;
  const label = t(key);
  return label === key ? status : label;
}

/** 文件夹行紧凑徽标：运行中会话 / 脏缓冲（成本不入行，见 §7.1）。 */
function folderBadges(t: Translate, project: ProjectSummary) {
  return [
    project.active_sessions ? `${project.active_sessions} ${t("projects.active")}` : null,
    project.dirty_buffers ? `${project.dirty_buffers} ${t("projects.dirty")}` : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

/** 项目最近会话更新时间：作为 Updated 列的轻量排序 / 扫读线索。 */
function latestUpdatedAt(project: ProjectSummary): string | null {
  return project.sessions.reduce<string | null>(
    (latest, session) => (!latest || session.updated_at > latest ? session.updated_at : latest),
    null
  );
}

/** 相对时间只用于侧栏扫读；精确时间保留在行 title，不做 daemon 依赖。 */
function formatUpdatedAt(value: string | null, t: Translate): string {
  if (!value) return t("projects.updated_never");
  const time = Date.parse(value);
  if (Number.isNaN(time)) return t("projects.updated_never");
  const elapsed = Math.max(0, Date.now() - time);
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return t("relative.now");
  if (minutes < 60) return t("relative.minutes_ago", { count: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("relative.hours_ago", { count: hours });
  const days = Math.floor(hours / 24);
  if (days < 30) return t("relative.days_ago", { count: days });
  return new Date(time).toLocaleDateString();
}

/** 会话显示名：自动标题优先（v1.58）；无标题回退模型名，同名多会话附短 id 后缀。 */
function sessionDisplayName(session: ProjectSummary["sessions"][number], duplicates = 1) {
  const base = session.title?.trim() || session.model || session.id;
  return duplicates > 1 ? `${base} ·${session.id.slice(-4)}` : base;
}

/** 同显示名（标题 / 模型名均计入）多会话时以短 id 后缀区分。 */
function countSessionNames(sessions: ProjectSummary["sessions"]) {
  const counts = new Map<string, number>();
  for (const session of sessions) {
    const key = session.title?.trim() || session.model || session.id;
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return counts;
}

/** 路径末段作为默认项目名（与 daemon 空名回退一致）。 */
function basenameOf(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] ?? "";
}

/** 文件夹图标：线性风格，对齐活动栏 rail 图标。 */
function FolderIcon() {
  return (
    <svg
      className="pe-folder-icon"
      width={14}
      height={14}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M3 7V5a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    </svg>
  );
}

/** 折叠箭头：线性 chevron，展开时经 CSS 旋转 90°（跨平台字形一致）。 */
function ChevronIcon() {
  return (
    <svg
      className="pe-caret"
      width={10}
      height={10}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2.4}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="m9 6 6 6-6 6" />
    </svg>
  );
}

/**
 * 选择项目目录（§6.4 添加项目模态）：桌面壳内走 Tauri 原生目录对话框；
 * 浏览器模式无绝对路径来源，由用户手输路径。
 */
async function pickDirectory(): Promise<string | null> {
  const internals = window.__TAURI_INTERNALS__ as
    | { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> }
    | undefined;
  if (!internals) return null;
  const picked = (await internals.invoke("plugin:dialog|open", {
    options: { directory: true, multiple: false, title: "Tenon" },
  })) as unknown;
  return typeof picked === "string" && picked ? picked : null;
}

export function ProjectExplorer({
  api,
  t,
  projects,
  projectId,
  sessionsByProject,
  openError,
  refreshToken,
  onSwitchProject,
  onOpenProject,
  onRemoveProject,
  onSelectSession,
  onCreateSession,
  onRefreshProjects,
  onOpenFile,
  onFileTreeChange,
}: Props) {
  const [expanded, setExpanded] = useState<Set<string>>(() => loadSet(PE_EXPANDED_KEY));
  // 行内嵌源码文件树的展开集合（v1.70），与文件夹展开互不影响。
  const [filesOpen, setFilesOpen] = useState<Set<string>>(() => loadSet(PE_FILES_KEY));
  const [adding, setAdding] = useState(false);
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  /** 用户手动改过项目名后，路径变更不再覆盖名称。 */
  const [nameEdited, setNameEdited] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  // 全局活动条（v1.87 §7.2）：跨项目聚合监控。
  const [activityOpen, setActivityOpen] = useState(false);
  const [activityFilter, setActivityFilter] = useState<ActivityFilter>("all");
  const [stoppingId, setStoppingId] = useState<string | null>(null);
  const [worktreeBusyId, setWorktreeBusyId] = useState<string | null>(null);
  /** 超过最近 10 条的项目的显式展开集合；不落盘，保持项目列表轻量。 */
  const [allSessionsOpen, setAllSessionsOpen] = useState<Set<string>>(new Set());

  const runProjectAction = async (
    project: ProjectSummary,
    action: (project: ProjectSummary) => Promise<void> | void
  ) => {
    setBusyId(project.id);
    try {
      await action(project);
    } finally {
      setBusyId(null);
    }
  };

  const toggleFiles = (projectId: string) => {
    setFilesOpen((prev) => {
      const next = new Set(prev);
      if (next.has(projectId)) next.delete(projectId);
      else next.add(projectId);
      persistSet(PE_FILES_KEY, next);
      return next;
    });
  };

  const addProject = async () => {
    const trimmed = path.trim();
    if (!trimmed || busyId) return;
    setBusyId("__add__");
    try {
      await onOpenProject(trimmed, name.trim() || undefined);
      setPath("");
      setName("");
      setNameEdited(false);
      setAdding(false);
    } finally {
      setBusyId(null);
    }
  };

  const closeAddDialog = () => {
    if (busyId === "__add__") return;
    setAdding(false);
    setPath("");
    setName("");
    setNameEdited(false);
  };

  const globalRows = projects
    .flatMap((project) =>
      project.sessions.map((session) => ({
        project,
        session
      }))
    )
    .sort((a, b) => (b.session.updated_at ?? "").localeCompare(a.session.updated_at ?? ""));
  const runningCount = globalRows.filter((row) => RUNNING.has(row.session.status)).length;
  const doneCount = globalRows.filter((row) => row.session.status === "done").length;
  const visibleRows = globalRows.filter((row) => {
    if (activityFilter === "running") return RUNNING.has(row.session.status);
    if (activityFilter === "done") return row.session.status === "done";
    return true;
  });

  const jumpToSession = (row: (typeof globalRows)[number]) => {
    if (row.project.id !== projectId) onSwitchProject(row.project);
    onSelectSession(row.project.id, row.session.id);
  };

  const stopSession = async (sessionId: string) => {
    setStoppingId(sessionId);
    try {
      await api.control(sessionId, "stop");
    } finally {
      setStoppingId(null);
    }
  };

  const mergeWorktree = async (sessionId: string) => {
    setWorktreeBusyId(sessionId);
    try {
      await api.mergeWorktreeSession(sessionId);
      onRefreshProjects?.();
    } catch {
      // 409 冲突 / 失败经刷新后的项目摘要与会话状态呈现（无内联 toast 面）。
    } finally {
      setWorktreeBusyId(null);
    }
  };

  const discardWorktree = async (sessionId: string) => {
    if (!window.confirm(t("projects.worktree_discard_confirm"))) return;
    setWorktreeBusyId(sessionId);
    try {
      await api.discardWorktreeSession(sessionId, true);
      onRefreshProjects?.();
    } finally {
      setWorktreeBusyId(null);
    }
  };

  // active 项目默认展开（v1.63：切换即见会话全貌）。
  useEffect(() => {
    if (!projectId) return;
    setExpanded((prev) => {
      if (prev.has(projectId)) return prev;
      const next = new Set(prev).add(projectId);
      persistSet(PE_EXPANDED_KEY, next);
      return next;
    });
  }, [projectId]);

  // Esc 仅关闭添加模态（打开中 busy 时不关）。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (adding && busyId !== "__add__") closeAddDialog();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [adding, busyId]);

  const browseForDirectory = async () => {
    const picked = await pickDirectory();
    if (!picked) return;
    setPath(picked);
    if (!nameEdited) setName(basenameOf(picked));
  };

  // 点击文件夹行：非当前项目先隐式激活（v1.60），已激活项目再点仅收起 / 展开。
  const toggleFolder = (project: ProjectSummary) => {
    if (project.id !== projectId) onSwitchProject(project);
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(project.id)) next.delete(project.id);
      else next.add(project.id);
      persistSet(PE_EXPANDED_KEY, next);
      return next;
    });
  };

  /** 展开文件夹下的会话与组合任务子项（归属该项目的任务按会话呈现）。 */
  const renderSessions = (project: ProjectSummary) => {
    const counts = countSessionNames(project.sessions);
    const activeSessionId = sessionsByProject[project.id] ?? null;
    const orderedSessions = [...project.sessions].sort((a, b) =>
      (b.updated_at ?? "").localeCompare(a.updated_at ?? "")
    );
    const showAll = allSessionsOpen.has(project.id);
    const sessions =
      orderedSessions.length > RECENT_SESSION_LIMIT && !showAll
        ? orderedSessions.slice(0, RECENT_SESSION_LIMIT)
        : orderedSessions;
    return (
      <ul className="pe-chat-list" data-testid={`chat-list-${project.id}`}>
        {sessions.map((session) => (
          <li key={session.id} className="pe-chat-item">
            <button
              type="button"
              data-testid={`chat-row-${session.id}`}
              className={
                session.id === activeSessionId ? "pe-chat-row active" : "pe-chat-row"
              }
              onClick={() => onSelectSession(project.id, session.id)}
              title={session.worktree_path ? `${session.id} · ${session.worktree_path}` : session.id}
            >
              <span className="pe-chat-name">
                {sessionDisplayName(
                  session,
                  counts.get(session.title?.trim() || session.model || session.id) ?? 1
                )}
                {session.worktree_path ? " ⎇" : ""}
              </span>
              <span className="pe-chat-status">{stateLabel(t, session.status)}</span>
            </button>
            {session.worktree_path && (
              <span className="pe-row-actions">
                <button
                  type="button"
                  className="pe-action"
                  disabled={worktreeBusyId === session.id}
                  title={t("projects.worktree_merge")}
                  onClick={() => void mergeWorktree(session.id)}
                >
                  {t("projects.worktree_merge")}
                </button>
                <button
                  type="button"
                  className="pe-action danger"
                  disabled={worktreeBusyId === session.id}
                  title={t("projects.worktree_discard")}
                  onClick={() => void discardWorktree(session.id)}
                >
                  {t("projects.worktree_discard")}
                </button>
              </span>
            )}
          </li>
        ))}
        {orderedSessions.length > RECENT_SESSION_LIMIT && (
          <li>
            <button
              type="button"
              className="pe-show-all"
              data-testid={`session-show-all-${project.id}`}
              onClick={() =>
                setAllSessionsOpen((prev) => {
                  const next = new Set(prev);
                  if (next.has(project.id)) next.delete(project.id);
                  else next.add(project.id);
                  return next;
                })
              }
            >
              {showAll ? t("projects.show_less") : t("projects.show_more")}
            </button>
          </li>
        )}
        {project.sessions.length === 0 && (
          <li className="pe-empty">{t("projects.chats_empty")}</li>
        )}
        {/* 新会话入口（v1.87 §7.3）：主根 / 受管 worktree（可与主根并行执行）。 */}
        <li className="pe-session-actions">
          <button
            type="button"
            className="pe-action"
            data-testid={`session-new-${project.id}`}
            onClick={() => onCreateSession(project, false)}
          >
            + {t("projects.new_session")}
          </button>
          <button
            type="button"
            className="pe-action"
            data-testid={`session-new-worktree-${project.id}`}
            title={t("projects.new_worktree_session")}
            onClick={() => onCreateSession(project, true)}
          >
            + ⎇ {t("projects.new_session")}
          </button>
        </li>
      </ul>
    );
  };

  return (
    <div className="project-explorer" data-testid="project-explorer">
      {/* Codex projects sidebar：常驻添加入口；列表用 Name / Updated 统一节奏。 */}
      <div className="pe-toolbar">
        <button
          type="button"
          className="pe-add"
          data-testid="project-add"
          aria-label={t("projects.add_title")}
          disabled={busyId === "__add__"}
          aria-expanded={adding}
          title={t("projects.add_title")}
          onClick={() => setAdding(true)}
        >
          +
        </button>
      </div>
      <div className="pe-columns">
        <span>{t("projects.column.name")}</span>
        <span>{t("projects.column.updated")}</span>
      </div>
      <section className="pe-section pe-chats">
        <ul className="pe-tree" data-testid="project-list">
          {/* All activity 与项目行同构，跨项目监控不再用首屏胶囊打断项目索引。 */}
          <li className="pe-activity-group">
            <section className="pe-section pe-activity" data-testid="global-activity">
              <button
                type="button"
                className="pe-activity-bar"
                data-testid="global-activity-bar"
                aria-expanded={activityOpen}
                onClick={() => setActivityOpen((open) => !open)}
              >
                <span
                  className="pe-dot"
                  style={{ background: runningCount ? "#d9a514" : "#8a8f98" }}
                />
                <span className="pe-activity-label">{t("activity.title")}</span>
                <span className="pe-activity-counts">
                  <span className="pe-activity-count">{t("activity.running")} {runningCount}</span>
                  <span className="pe-activity-count">{t("activity.done")} {doneCount}</span>
                </span>
                <ChevronIcon />
              </button>
              {activityOpen && (
                <div className="pe-activity-panel" data-testid="global-activity-list">
                  <div className="pe-activity-filters">
                    {ACTIVITY_FILTERS.map((filter) => (
                      <button
                        key={filter}
                        type="button"
                        className={activityFilter === filter ? "active" : ""}
                        onClick={() => setActivityFilter(filter)}
                      >
                        {t(`activity.filter.${filter}`)}
                      </button>
                    ))}
                  </div>
                  {visibleRows.length === 0 && <div className="pe-empty">{t("activity.empty")}</div>}
                  <ul className="pe-activity-list">
                    {visibleRows.map(({ project, session }) => (
                      <li key={session.id} className="pe-activity-item">
                        <button
                          type="button"
                          className="pe-activity-row"
                          onClick={() => jumpToSession({ project, session })}
                          title={session.id}
                        >
                          <span
                            className="pe-dot"
                            style={{ background: statusDotColor(session.status) }}
                          />
                          <span className="pe-activity-project">{project.display_name}</span>
                          <span className="pe-activity-title">
                            {sessionDisplayName(session)}
                            {session.worktree_path ? " ⎇" : ""}
                          </span>
                          <span className="pe-chat-status">{stateLabel(t, session.status)}</span>
                        </button>
                        {RUNNING.has(session.status) && (
                          <button
                            type="button"
                            className="pe-action danger"
                            disabled={stoppingId === session.id}
                            onClick={() => void stopSession(session.id)}
                          >
                            {t("activity.stop")}
                          </button>
                        )}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </section>
          </li>
          {projects.map((project) => {
            const isOpen = expanded.has(project.id);
            const showFiles = filesOpen.has(project.id);
            const updatedAt = latestUpdatedAt(project);
            return (
              <li key={project.id} className="pe-folder">
                <div
                  className={
                    project.id === projectId ? "pe-folder-row active" : "pe-folder-row"
                  }
                >
                  <button
                    type="button"
                    className="pe-folder-btn"
                    data-testid={`project-item-${project.id}`}
                    aria-expanded={isOpen}
                    disabled={busyId === project.id}
                    title={project.path}
                    onClick={() => toggleFolder(project)}
                  >
                    <ChevronIcon />
                    <FolderIcon />
                    <span className="pe-project-main">
                      <span className="pe-project-name">{project.display_name}</span>
                      <span className="pe-project-meta">{folderBadges(t, project)}</span>
                    </span>
                    <span
                      className="pe-updated"
                      title={updatedAt ? new Date(updatedAt).toLocaleString() : undefined}
                    >
                      {formatUpdatedAt(updatedAt, t)}
                    </span>
                  </button>
                  <div className="pe-row-actions">
                    <button
                      type="button"
                      className="pe-action"
                      data-testid={`project-files-${project.id}`}
                      aria-label={t("projects.source")}
                      aria-pressed={showFiles}
                      title={t("projects.source")}
                      onClick={() => toggleFiles(project.id)}
                    >
                      {"</>"}
                    </button>
                    <button
                      type="button"
                      className="pe-action danger"
                      data-testid={`project-remove-${project.id}`}
                      aria-label={t("projects.remove")}
                      disabled={busyId === project.id || project.sessions.length > 0}
                      title={
                        project.sessions.length
                          ? t("projects.remove_blocked")
                          : t("projects.remove")
                      }
                      onClick={() => void runProjectAction(project, onRemoveProject)}
                    >
                      ×
                    </button>
                  </div>
                </div>
                {isOpen && renderSessions(project)}
                {showFiles && (
                  <div className="pe-source" data-testid={`source-tree-${project.id}`}>
                    <FileTree
                      api={api}
                      t={t}
                      projectId={project.id}
                      refreshToken={refreshToken}
                      onOpenFile={onOpenFile}
                      onOperation={onFileTreeChange}
                    />
                  </div>
                )}
              </li>
            );
          })}
          {projects.length === 0 && <li className="pe-empty">{t("projects.empty")}</li>}
        </ul>
      </section>

      {adding && (
        <form
          className="pe-overlay"
          data-testid="project-add-form"
          onClick={(event) => {
            if (event.target === event.currentTarget) closeAddDialog();
          }}
          onSubmit={(event) => {
            event.preventDefault();
            void addProject();
          }}
        >
          <div
            className="pe-dialog"
            role="dialog"
            aria-modal="true"
            aria-label={t("projects.add_title")}
          >
            <div className="pe-dialog-head">
              <strong>{t("projects.add_title")}</strong>
            </div>
            <label className="pe-dialog-field">
              <span>{t("projects.path")}</span>
              <div className="pe-dialog-row">
                <input
                  aria-label={t("projects.path")}
                  data-testid="project-add-path"
                  value={path}
                  placeholder="/absolute/path/to/project"
                  autoFocus
                  onChange={(event) => {
                    const next = event.target.value;
                    setPath(next);
                    if (!nameEdited) setName(basenameOf(next));
                  }}
                />
                {window.__TAURI_INTERNALS__ !== undefined && (
                  <button
                    type="button"
                    className="pe-action"
                    data-testid="project-add-browse"
                    onClick={() => void browseForDirectory()}
                  >
                    {t("projects.browse")}
                  </button>
                )}
              </div>
            </label>
            <label className="pe-dialog-field">
              <span>{t("projects.name")}</span>
              <input
                aria-label={t("projects.name")}
                data-testid="project-add-name"
                value={name}
                placeholder={t("projects.name_hint")}
                onChange={(event) => {
                  setName(event.target.value);
                  setNameEdited(true);
                }}
              />
            </label>
            {openError && (
              <div className="tree-error" role="alert">
                {openError}
              </div>
            )}
            <div className="pe-dialog-actions">
              <button
                type="button"
                className="pe-action"
                data-testid="project-add-cancel"
                onClick={closeAddDialog}
              >
                {t("projects.cancel")}
              </button>
              <button type="submit" disabled={!path.trim() || busyId === "__add__"}>
                {t("projects.open")}
              </button>
            </div>
          </div>
        </form>
      )}
    </div>
  );
}
