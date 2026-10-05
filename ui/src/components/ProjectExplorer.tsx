// 项目浏览器（左侧栏「项目」视图，v1.88 对齐 Codex projects sidebar 内容布局）：
// 视图标题行右端常驻添加入口（v1.101 移除孤行工具行与 Name / Updated 列头）；
// 每项目一行可折叠文件夹（点击行即隐式激活并展开），展开区顶部「任务 | 源码」
// 行内切换（v1.110）：任务=会话列表，源码=该项目文件树（单击文件开编辑器浮层）。
import { useEffect, useState } from "react";
import type { ProjectSummary, TenonApi } from "../lib/api";
import { useResolvedLocale, type Translate } from "../lib/i18n";
import { RUNNING_STATES, STATE_COLORS, type AgentStateName } from "../lib/stateColors";
import { FileTree, type FileTreeChange } from "./FileTree";

const PE_EXPANDED_KEY = "tenon:peExpanded";
const PE_VIEW_KEY = "tenon:peView";
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

/** 展开区视图（v1.110）：per-project「任务 | 源码」，默认任务。 */
type PeView = "tasks" | "files";

function loadPeView(): Record<string, PeView> {
  try {
    const raw = localStorage.getItem(PE_VIEW_KEY);
    return raw ? (JSON.parse(raw) as Record<string, PeView>) : {};
  } catch {
    return {};
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
  /** ProjectRuntime 文件事件版本；源码视图文件树增量刷新（§6.4 / §8.1）。 */
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
  /** 会话被归档 / 删除后回调（v1.103）：App 清理激活选择并刷新摘要。 */
  onSessionRemoved?: (projectId: string, sessionId: string) => void;
  /** 源码视图打开文件（v1.110）：经 App openFile 弹出编辑器浮层。 */
  onOpenFile: (path: string) => void;
  onFileTreeChange: (change: FileTreeChange) => void;
}

/** v1.114：状态图标自带视觉语义，state.* 文案转行 title（stateLabel 随 v1.101 圆点方案退役）。 */

/** 任务状态图标（v1.114 Codex 规格）：运行态 spinner 圆环、done=✓、error=✗，
 *  其余状态回退 §7.5 STATE_COLORS 色点（单一来源不变）。 */
function StatusIcon({ status }: { status: string }) {
  if (RUNNING_STATES.has(status as AgentStateName)) {
    return <span className="pe-status-spin" data-state={status} aria-hidden="true" />;
  }
  if (status === "done") {
    return (
      <span className="pe-status-icon ok" data-state={status} aria-hidden="true">
        <svg width={11} height={11} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.6} strokeLinecap="round" strokeLinejoin="round">
          <path d="M20 6 9 17l-5-5" />
        </svg>
      </span>
    );
  }
  if (status === "error") {
    return (
      <span className="pe-status-icon err" data-state={status} aria-hidden="true">
        <svg width={11} height={11} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.6} strokeLinecap="round" strokeLinejoin="round">
          <path d="M18 6 6 18" />
          <path d="m6 6 12 12" />
        </svg>
      </span>
    );
  }
  return (
    <span
      className="pe-status-dot"
      aria-hidden="true"
      data-state={status}
      style={{ background: STATE_COLORS[status as AgentStateName] ?? "#8a8f98" }}
    />
  );
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
function formatUpdatedAt(value: string | null, t: Translate, locale: string): string {
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
  return new Date(time).toLocaleDateString(locale);
}

/** 显示层标题清洗（v1.101）：剥离截断 prompt 引導前缀与首尾引号；只影响显示，不改库。 */
const TITLE_PROMPT_PREFIX_RE =
  /^the user(?:'|’)?s?\s+(?:message|says?|wants?|asks?|requests?)[^:]{0,32}:\s*/i;

function cleanDisplayTitle(raw: string): string {
  let s = raw.trim();
  let prev = "";
  while (s !== prev) {
    prev = s;
    s = s.replace(TITLE_PROMPT_PREFIX_RE, "").trim();
  }
  return s.replace(/^["'“”「『«]+|["'”』»]+$/g, "").trim();
}

/** 会话显示基名：自动标题（清洗后）优先（v1.58）；无标题回退模型名，再回退短 id。 */
function displayBaseName(session: ProjectSummary["sessions"][number]): string {
  return cleanDisplayTitle(session.title ?? "") || session.model || session.id;
}

function sessionDisplayName(session: ProjectSummary["sessions"][number], duplicates = 1) {
  const base = displayBaseName(session);
  return duplicates > 1 ? `${base} ·${session.id.slice(-4)}` : base;
}

/** 同显示名（标题 / 模型名均计入）多会话时以短 id 后缀区分。 */
function countSessionNames(sessions: ProjectSummary["sessions"]) {
  const counts = new Map<string, number>();
  for (const session of sessions) {
    counts.set(displayBaseName(session), (counts.get(displayBaseName(session)) ?? 0) + 1);
  }
  return counts;
}

/** 路径末段作为默认项目名（与 daemon 空名回退一致）。 */
function basenameOf(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] ?? "";
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

/** 线性加号：文本「+」字形墨迹受字体度量影响偏移行中心（Windows 字体更甚），SVG 保证与边框行对齐。 */
function PlusIcon() {
  return (
    <svg
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
      <path d="M12 5v14M5 12h14" />
    </svg>
  );
}

/** 线性分支图标（v1.125）：U+23A1「⎡」是数学多行括号的上半块，12px 下呈残缺角括号状被误读为渲染
    缺陷——换为与字体回退解耦的 stroke SVG，语义（受管 worktree 新任务）与 testid / aria 不变。 */
function WorktreeIcon() {
  return (
    <svg
      width={12}
      height={12}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2.4}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M6 3v12" />
      <circle cx={18} cy={6} r={3} />
      <circle cx={6} cy={18} r={3} />
      <path d="M18 9a9 9 0 0 1-9 9" />
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
  onSessionRemoved,
  onOpenFile,
  onFileTreeChange,
}: Props) {
  const localeTag = useResolvedLocale();
  const [expanded, setExpanded] = useState<Set<string>>(() => loadSet(PE_EXPANDED_KEY));
  // 展开区「任务 | 源码」行内切换（v1.110）：per-project 记忆，默认任务。
  const [peView, setPeViewState] = useState<Record<string, PeView>>(loadPeView);
  const setPeView = (pid: string, view: PeView) => {
    setPeViewState((prev) => {
      const next = { ...prev, [pid]: view };
      try {
        localStorage.setItem(PE_VIEW_KEY, JSON.stringify(next));
      } catch {
        // 存储不可用时仅当次会话内生效
      }
      return next;
    });
  };
  const [adding, setAdding] = useState(false);
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  /** 用户手动改过项目名后，路径变更不再覆盖名称。 */
  const [nameEdited, setNameEdited] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [worktreeBusyId, setWorktreeBusyId] = useState<string | null>(null);
  /** 「已归档」折叠组展开集合（v1.103）；不落盘，默认收起。 */
  const [archivedOpen, setArchivedOpen] = useState<Set<string>>(new Set());
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

  // v1.103：归档 / 还原 / 删除（§15）。409 守卫（运行中 / 未收尾 worktree）经刷新后的摘要呈现。
  const archiveSessionRow = async (sessionId: string) => {
    try {
      await api.archiveSession(sessionId);
      onSessionRemoved?.(projectId ?? "", sessionId);
      onRefreshProjects?.();
    } catch {}
  };

  const unarchiveSessionRow = async (sessionId: string) => {
    try {
      await api.unarchiveSession(sessionId);
      onRefreshProjects?.();
    } catch {}
  };

  const deleteSessionRow = async (projectId: string, sessionId: string) => {
    if (!window.confirm(t("projects.delete_confirm"))) return;
    try {
      await api.deleteSession(sessionId, true);
      onSessionRemoved?.(projectId, sessionId);
      onRefreshProjects?.();
    } catch {}
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
    // v1.116：只展示真正开始的任务——无标题回退（显示名 = 模型名 / 短 id）的会话
    // 从未发送过首条消息（或标题尚未生成），不进列表；标题生成（session_title）后即现。
    const started = (session: ProjectSummary["sessions"][number]) =>
      displayBaseName(session) !== (session.model || session.id);
    const counts = countSessionNames(project.sessions.filter(started));
    const activeSessionId = sessionsByProject[project.id] ?? null;
    const orderedSessions = project.sessions
      .filter(started)
      .sort((a, b) => (b.updated_at ?? "").localeCompare(a.updated_at ?? ""));
    const showAll = allSessionsOpen.has(project.id);
    const sessions =
      orderedSessions.length > RECENT_SESSION_LIMIT && !showAll
        ? orderedSessions.slice(0, RECENT_SESSION_LIMIT)
        : orderedSessions;
    return (
      <ul className="pe-chat-list" data-testid={`chat-list-${project.id}`}>
        {sessions.map((session) => {
          // v1.101：已回滚 / 无标题回退（显示模型名）会话整行灰显降噪。
          const fallbackName = displayBaseName(session) === (session.model || session.id);
          const muted = session.status === "rolled_back" || fallbackName;
          return (
          <li key={session.id} className="pe-chat-item">
            <button
              type="button"
              data-testid={`chat-row-${session.id}`}
              className={
                (session.id === activeSessionId ? "pe-chat-row active" : "pe-chat-row") +
                (muted ? " muted" : "")
              }
              onClick={() => onSelectSession(project.id, session.id)}
              title={session.worktree_path ? `${session.id} · ${session.worktree_path}` : session.id}
            >
              <StatusIcon status={session.status} />
              <span className="pe-chat-name">
                {sessionDisplayName(
                  session,
                  counts.get(displayBaseName(session)) ?? 1
                )}
                {session.worktree_path ? " ⎇" : ""}
              </span>
              <span
                className="pe-chat-time"
                title={session.updated_at ? new Date(session.updated_at).toLocaleString(localeTag) : undefined}
              >
                {formatUpdatedAt(session.updated_at ?? null, t, localeTag)}
              </span>
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
            {/* v1.103：未收尾 worktree 行只留合并 / 丢弃；运行中行不渲染归档 / 删除。 */}
            {!session.worktree_path && !RUNNING_STATES.has(session.status as AgentStateName) && (
              <span className="pe-row-actions">
                <button
                  type="button"
                  className="pe-action"
                  data-testid={`session-archive-${session.id}`}
                  title={t("projects.archive")}
                  onClick={() => void archiveSessionRow(session.id)}
                >
                  {t("projects.archive")}
                </button>
                <button
                  type="button"
                  className="pe-action danger"
                  data-testid={`session-delete-${session.id}`}
                  title={t("projects.delete")}
                  onClick={() => void deleteSessionRow(project.id, session.id)}
                >
                  {t("projects.delete")}
                </button>
              </span>
            )}
          </li>
          );
        })}
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
        {orderedSessions.length === 0 && (
          <li className="pe-empty">{t("projects.chats_empty")}</li>
        )}
        {/* 已归档组（v1.103）：默认收起，展开后行内还原 / 删除；未开始会话同样不显示（v1.116）。 */}
        {(project.archived_sessions?.filter(started).length ?? 0) > 0 && (
          <li className="pe-archived">
            <button
              type="button"
              className="pe-show-all"
              data-testid={`archived-toggle-${project.id}`}
              aria-expanded={archivedOpen.has(project.id)}
              onClick={() =>
                setArchivedOpen((prev) => {
                  const next = new Set(prev);
                  if (next.has(project.id)) next.delete(project.id);
                  else next.add(project.id);
                  return next;
                })
              }
            >
              {t("projects.archived_group", { count: project.archived_sessions!.filter(started).length })}
            </button>
            {archivedOpen.has(project.id) && (
              <ul className="pe-chat-list pe-archived-list">
                {project.archived_sessions!.filter(started).map((session) => (
                  <li key={session.id} className="pe-chat-item">
                    <button
                      type="button"
                      className="pe-chat-row muted"
                      onClick={() => onSelectSession(project.id, session.id)}
                      title={session.id}
                    >
                      <StatusIcon status={session.status} />
                      <span className="pe-chat-name">
                        {sessionDisplayName(session)}
                        {session.worktree_path ? " ⎇" : ""}
                      </span>
                    </button>
                    <span className="pe-row-actions">
                      <button
                        type="button"
                        className="pe-action"
                        data-testid={`session-unarchive-${session.id}`}
                        title={t("projects.unarchive")}
                        onClick={() => void unarchiveSessionRow(session.id)}
                      >
                        {t("projects.unarchive")}
                      </button>
                      <button
                        type="button"
                        className="pe-action danger"
                        data-testid={`session-delete-${session.id}`}
                        title={t("projects.delete")}
                        onClick={() => void deleteSessionRow(project.id, session.id)}
                      >
                        {t("projects.delete")}
                      </button>
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </li>
        )}
        {/* v1.114：新任务入口上移——视图标题行「＋ 新任务」+ 分组行尾 hover 分支图标（v1.125 由「⎡」字符换 SVG）。 */}
      </ul>
    );
  };

  return (
    <div className="project-explorer" data-testid="project-explorer">
      {/* 视图标题行（v1.101；v1.114 新任务主按钮——作用 active 项目，
          添加项目让位至列表底部常驻行；v1.115 撤销 v1.106 收起钮，
          侧栏开合收敛 rail 切换钮 / rail 同视图再点 / 命令面板）。 */}
      <div className="pe-head">
        <span className="side-title">{t("panel.projects")}</span>
        <span className="pe-head-actions">
          <button
            type="button"
            className="pe-new-task"
            data-testid="project-new-task"
            disabled={!projectId || busyId === "__add__"}
            title={t("projects.new_task")}
            aria-label={t("projects.new_task")}
            onClick={() => {
              const active = projects.find((project) => project.id === projectId);
              if (active) onCreateSession(active, false);
            }}
          >
            + {t("projects.new_task")}
          </button>
        </span>
      </div>
      <section className="pe-section pe-chats">
        <ul className="pe-tree" data-testid="project-list">
          {projects.map((project) => {
            const isOpen = expanded.has(project.id);
            const updatedAt = latestUpdatedAt(project);
            const badges = folderBadges(t, project);
            const sourceOn = (peView[project.id] ?? "tasks") === "files";
            return (
              <li key={project.id} className="pe-group">
                <div
                  className={
                    project.id === projectId ? "pe-group-row active" : "pe-group-row"
                  }
                >
                  <button
                    type="button"
                    className="pe-group-btn"
                    data-testid={`project-item-${project.id}`}
                    aria-expanded={isOpen}
                    disabled={busyId === project.id}
                    title={project.path}
                    onClick={() => toggleFolder(project)}
                  >
                    <ChevronIcon />
                    <span className="pe-group-name">{project.display_name}</span>
                    {badges && <span className="pe-group-meta">{badges}</span>}
                  </button>
                  {/* 项目名后「任务 / 源码」视图切换（v1.118 文字钮，显示目标视图名）。 */}
                  <button
                    type="button"
                    className="pe-view-toggle"
                    data-testid={`pe-source-${project.id}`}
                    aria-label={t(sourceOn ? "projects.tab_tasks" : "projects.tab_source")}
                    title={t(sourceOn ? "projects.tab_tasks" : "projects.tab_source")}
                    onClick={() => setPeView(project.id, sourceOn ? "tasks" : "files")}
                  >
                    {t(sourceOn ? "projects.tab_tasks" : "projects.tab_source")}
                  </button>
                  <span
                    className="pe-updated"
                    title={updatedAt ? new Date(updatedAt).toLocaleString(localeTag) : undefined}
                  >
                    {formatUpdatedAt(updatedAt, t, localeTag)}
                  </span>
                  {/* 分组行尾 hover 操作区（v1.118）：worktree 新任务 / 移除登记。 */}
                  <div className="pe-group-actions" data-testid={`pe-view-${project.id}`}>
                    <button
                      type="button"
                      className="pe-gact"
                      data-testid={`session-new-worktree-${project.id}`}
                      aria-label={t("projects.new_worktree_session")}
                      disabled={busyId === project.id}
                      title={t("projects.new_worktree_session")}
                      onClick={() => onCreateSession(project, true)}
                    >
                      <WorktreeIcon />
                    </button>
                    <button
                      type="button"
                      className="pe-gact danger"
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
                {isOpen && (
                  <div className="pe-detail">
                    {sourceOn ? (
                      <div className="pe-files">
                        <FileTree
                          api={api}
                          t={t}
                          projectId={project.id}
                          refreshToken={refreshToken}
                          onOpenFile={onOpenFile}
                          onOperation={onFileTreeChange}
                        />
                      </div>
                    ) : (
                      renderSessions(project)
                    )}
                  </div>
                )}
              </li>
            );
          })}
          {projects.length === 0 && <li className="pe-empty">{t("projects.empty")}</li>}
        </ul>
        {/* 列表底部常驻添加入口（v1.114 回归 v1.43 语义；v1.101 标题行「+」让位新任务按钮）。 */}
        <button
          type="button"
          className="pe-add-row"
          data-testid="project-add"
          disabled={busyId === "__add__"}
          aria-expanded={adding}
          onClick={() => setAdding(true)}
        >
          <PlusIcon />
          {t("projects.add_title")}
        </button>
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
