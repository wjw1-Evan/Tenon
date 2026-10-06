// 项目浏览器（左侧栏「项目」视图，v1.88 对齐 Codex projects sidebar 内容布局）：
// 视图标题行右端常驻添加入口（v1.101 移除孤行工具行与 Name / Updated 列头）；
// 每项目一行可折叠文件夹（点击行即隐式激活并展开），展开区恒为该项目会话列表；
// 「源码」钮上下拆分侧栏（v1.139）：下区 = active 项目文件树（pe-source-pane），
// 点文件行经 App 弹出编辑器浮层，主区线程不动。
import { useEffect, useState } from "react";
import type { ProjectSummary, TenonApi } from "../lib/api";
import { useResolvedLocale, type Translate } from "../lib/i18n";
import { RUNNING_STATES, STATE_COLORS, type AgentStateName } from "../lib/stateColors";
import { FileTree, type FileTreeChange } from "./FileTree";
import { ResizeHandle } from "./ResizeHandle";

const PE_EXPANDED_KEY = "tenon:peExpanded";
/** Codex 项目行展开后默认只物化最近会话，长列表显式展开。 */
const RECENT_SESSION_LIMIT = 10;
/** 侧栏源码区高度（v1.139）：localStorage 记忆。 */
const PE_SOURCE_HEIGHT_KEY = "tenon:sideSourceHeight";

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
  /** 侧栏源码区开合（§7.2 v1.139，App 会话级状态）：开 = 侧栏上下拆分，下区为 active 项目文件树。 */
  sourceOpen: boolean;
  /** 打开源码区（v1.139）：隐式激活该项目后拆分侧栏（主区线程不动）。 */
  onOpenSource: (project: ProjectSummary) => void;
  /** 收起源码区（v1.139）：侧栏回到整栏任务流。 */
  onCloseSource: () => void;
  /** 源码区文件树增量刷新（§6.4 / §8.1 ProjectRuntime 事件版本）。 */
  refreshToken: number;
  /** 源码区单击文件行：经 App 弹出编辑器浮层（v1.110）。 */
  onOpenFile: (path: string) => void;
  /** 源码区文件操作（v1.72 移动 / 删除）回调。 */
  onFileTreeChange: (change: FileTreeChange) => void;
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
  sourceOpen,
  onOpenSource,
  onCloseSource,
  refreshToken,
  onOpenFile,
  onFileTreeChange,
  onSwitchProject,
  onOpenProject,
  onRemoveProject,
  onSelectSession,
  onCreateSession,
  onRefreshProjects,
  onSessionRemoved,
}: Props) {
  const localeTag = useResolvedLocale();
  const [expanded, setExpanded] = useState<Set<string>>(() => loadSet(PE_EXPANDED_KEY));
  // 侧栏源码区高度（v1.139）：localStorage 记忆。
  const [sourceHeight, setSourceHeight] = useState(
    () => Number(localStorage.getItem(PE_SOURCE_HEIGHT_KEY)) || 260
  );
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
  // 浏览器模式目录选择浮层（v1.133）：GET /fs/dirs 逐层进入——桌面壳走 Tauri 原生目录对话框。
  // 开合（pickerOpen）与数据（picker）分离：加载失败时浮层仍可呈现错误态。
  const [pickerOpen, setPickerOpen] = useState(false);
  const [picker, setPicker] = useState<{
    path: string;
    parent: string | null;
    entries: Array<{ name: string; path: string }>;
  } | null>(null);
  const [pickerLoading, setPickerLoading] = useState(false);
  const [pickerError, setPickerError] = useState<string | null>(null);

  const loadDirs = async (path?: string) => {
    setPickerLoading(true);
    setPickerError(null);
    try {
      setPicker(await api.listDirs(path));
    } catch (e) {
      setPickerError(e instanceof Error ? e.message : String(e));
    } finally {
      setPickerLoading(false);
    }
  };
  const closePicker = () => {
    setPickerOpen(false);
    setPicker(null);
    setPickerError(null);
  };
  // 「选择当前目录」：回填路径输入框并自动项目名（nameEdited 语义与手输一致）。
  const choosePicked = () => {
    if (!picker || pickerLoading) return;
    setPath(picker.path);
    if (!nameEdited) setName(basenameOf(picker.path));
    closePicker();
  };

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

  // Esc 先收目录选择浮层（v1.133），再收添加模态（打开中 busy 时不关）。
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (adding && busyId !== "__add__") {
        if (pickerOpen) closePicker();
        else closeAddDialog();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [adding, busyId, pickerOpen]);

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
          {/* v1.143：添加项目迁入标题行（新任务左侧），底部常驻添加行随之移除。 */}
          <button
            type="button"
            className="pe-head-add"
            data-testid="project-add"
            aria-expanded={adding}
            disabled={busyId === "__add__"}
            title={t("projects.add_title")}
            aria-label={t("projects.add_title")}
            onClick={() => setAdding(true)}
          >
            + {t("projects.add_title")}
          </button>
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
            const sourceActive = sourceOpen && project.id === projectId;
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
                  {/* 项目名后「任务 / 源码」钮（v1.118 文字钮；v1.139 = 侧栏上下拆分开合），
                      钮面显示目标视图名；active 项目源码区开时点「任务」收起源码区，
                      其余项目点「源码」隐式激活并拆分。 */}
                  <button
                    type="button"
                    className="pe-view-toggle"
                    data-testid={`pe-source-${project.id}`}
                    aria-label={t(sourceActive ? "projects.tab_tasks" : "projects.tab_source")}
                    title={t(sourceActive ? "projects.tab_tasks" : "projects.tab_source")}
                    onClick={() => (sourceActive ? onCloseSource() : onOpenSource(project))}
                  >
                    {t(sourceActive ? "projects.tab_tasks" : "projects.tab_source")}
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
                    {/* v1.137：展开区恒为会话列表——行内「源码」文件树分支随源码工作台迁出侧栏。 */}
                    {renderSessions(project)}
                  </div>
                )}
              </li>
            );
          })}
          {projects.length === 0 && <li className="pe-empty">{t("projects.empty")}</li>}
        </ul>
        {/* v1.143：底部添加行移除——添加项目钮迁标题行（新任务左侧）。 */}
      </section>

      {/* 侧栏源码区（§7.2 v1.139）：下区 = active 项目文件树——与任务输入框同屏，
          文件 / 文件夹可直接拖入输入框插 @路径；点文件行经 App 弹出编辑器浮层。 */}
      {sourceOpen && projectId && (
        <>
          <ResizeHandle
            dir="vertical"
            testId="resize-side-source"
            onResize={(d) =>
              setSourceHeight((h) => {
                const v = Math.min(560, Math.max(140, h - d));
                localStorage.setItem(PE_SOURCE_HEIGHT_KEY, String(v));
                return v;
              })
            }
          />
          <div
            className="pe-source-pane"
            data-testid="pe-source-pane"
            style={{ height: sourceHeight }}
          >
            <div className="pe-source-head">
              <span className="side-title">{t("projects.tab_source")}</span>
              <button
                type="button"
                className="pe-collapse"
                data-testid="pe-source-close"
                title={t("editor.close")}
                aria-label={t("editor.close")}
                onClick={onCloseSource}
              >
                <svg width={12} height={12} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M18 6 6 18" />
                  <path d="m6 6 12 12" />
                </svg>
              </button>
            </div>
            <FileTree
              api={api}
              t={t}
              projectId={projectId}
              refreshToken={refreshToken}
              onOpenFile={onOpenFile}
              onOperation={onFileTreeChange}
            />
          </div>
        </>
      )}

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
                {/* v1.133：浏览按钮恒显示——桌面壳走 Tauri 原生目录对话框，
                    浏览器模式打开应用内目录选择浮层（GET /fs/dirs）。 */}
                <button
                  type="button"
                  className="pe-action"
                  data-testid="project-add-browse"
                  onClick={() => {
                    if (window.__TAURI_INTERNALS__ !== undefined) void browseForDirectory();
                    else {
                      setPickerOpen(true);
                      void loadDirs();
                    }
                  }}
                >
                  {t("projects.browse")}
                </button>
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

      {/* 目录选择浮层（v1.133）：浏览器模式经 GET /fs/dirs 逐层进入本地目录；
          叠于添加模态之上（后渲染同级覆盖），Esc / 遮罩仅收浮层回到模态。 */}
      {adding && pickerOpen && (
        <div
          className="pe-overlay pe-overlay-picker"
          data-testid="project-add-picker"
          onClick={(event) => {
            if (event.target === event.currentTarget) closePicker();
          }}
        >
          <div
            className="pe-dialog"
            role="dialog"
            aria-modal="true"
            aria-label={t("projects.folder_picker_title")}
          >
            <div className="pe-dialog-head">
              <strong>{t("projects.folder_picker_title")}</strong>
            </div>
            {picker && (
              <>
                <div
                  className="pe-picker-path"
                  data-testid="project-add-picker-path"
                  title={picker.path}
                >
                  {picker.path}
                </div>
                <div className="pe-picker-toolbar">
                  <button
                    type="button"
                    className="pe-action"
                    data-testid="project-add-picker-up"
                    disabled={picker.parent === null || pickerLoading}
                    aria-label={t("projects.folder_up")}
                    title={t("projects.folder_up")}
                    onClick={() => void loadDirs(picker.parent ?? undefined)}
                  >
                    ↑ {t("projects.folder_up")}
                  </button>
                  <button
                    type="button"
                    className="pe-action"
                    data-testid="project-add-picker-home"
                    disabled={pickerLoading}
                    aria-label={t("projects.folder_home")}
                    title={t("projects.folder_home")}
                    onClick={() => void loadDirs()}
                  >
                    ⌂ {t("projects.folder_home")}
                  </button>
                </div>
                <ul className="pe-picker-list" data-testid="project-add-picker-list">
                  {picker.entries.map((entry) => (
                    <li key={entry.path}>
                      <button
                        type="button"
                        className="pe-picker-row"
                        data-testid={`picker-dir-${entry.name}`}
                        disabled={pickerLoading}
                        title={entry.path}
                        onClick={() => void loadDirs(entry.path)}
                      >
                        {entry.name}
                      </button>
                    </li>
                  ))}
                  {!pickerLoading && picker.entries.length === 0 && (
                    <li className="pe-picker-empty">{t("projects.folder_empty")}</li>
                  )}
                </ul>
              </>
            )}
            {pickerLoading && (
              <div className="pe-picker-state" data-testid="project-add-picker-loading">
                {t("projects.folder_loading")}
              </div>
            )}
            {pickerError && (
              <div className="tree-error" role="alert" data-testid="project-add-picker-error">
                {t("projects.folder_error")} · {pickerError}
              </div>
            )}
            <div className="pe-dialog-actions">
              <button
                type="button"
                className="pe-action"
                data-testid="project-add-picker-cancel"
                onClick={closePicker}
              >
                {t("projects.cancel")}
              </button>
              <button
                type="button"
                data-testid="project-add-picker-choose"
                disabled={pickerLoading || !picker}
                onClick={choosePicked}
              >
                {t("projects.folder_choose")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
