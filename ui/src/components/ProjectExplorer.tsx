// 项目浏览器（左侧栏「项目」视图，v1.63 参考 ZCode 客户端侧栏项目文件夹；v1.70 移除视图 tab）：
// 单一项目文件夹树——每项目一行可折叠文件夹（点击行即隐式激活并展开），
// 展开后行下内嵌该项目会话列表；行尾「源码」按钮切换该行下内嵌的该项目文件树；
// 列表底部常驻「添加项目」（v1.43 模态）。
import { useEffect, useState } from "react";
import type { PortfolioTask, ProjectSummary, TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { FileTree, type FileTreeChange } from "./FileTree";

const PE_EXPANDED_KEY = "tenon:peExpanded";
const PE_FILES_KEY = "tenon:peFiles";

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
  portfolioTasks: PortfolioTask[];
  openError: string | null;
  /** ProjectRuntime 文件事件版本；透传文件树增量刷新（§6.4 / §8.1）。 */
  refreshToken: number;
  onSwitchProject: (project: ProjectSummary) => void;
  /** displayName 提供时落库为项目显示名；空 / 缺省回退路径末段（§6.4）。 */
  onOpenProject: (path: string, displayName?: string) => Promise<void> | void;
  onRemoveProject: (project: ProjectSummary) => Promise<void> | void;
  onSelectSession: (projectId: string, sessionId: string) => void;
  onOpenFile: (path: string) => void;
  onFileTreeChange: (change: FileTreeChange) => void;
}

/** 状态文案：优先使用 state.* 翻译，缺失回退原始状态。 */
function stateLabel(t: Translate, status: string) {
  const key = `state.${status}`;
  const label = t(key);
  return label === key ? status : label;
}

/** 文件夹行紧凑徽标：运行中会话 / 待审批 / 脏缓冲（成本不入行，见 §7.1）。 */
function folderBadges(t: Translate, project: ProjectSummary) {
  return [
    project.active_sessions ? `${project.active_sessions} ${t("projects.active")}` : null,
    project.pending_approvals.length
      ? `${project.pending_approvals.length} ${t("projects.approvals")}`
      : null,
    project.dirty_buffers ? `${project.dirty_buffers} ${t("projects.dirty")}` : null,
  ]
    .filter(Boolean)
    .join(" · ");
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
  portfolioTasks,
  openError,
  refreshToken,
  onSwitchProject,
  onOpenProject,
  onRemoveProject,
  onSelectSession,
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
    const tasks = portfolioTasks.filter((task) =>
      task.children.some((child) => child.project_id === project.id)
    );
    return (
      <ul className="pe-chat-list" data-testid={`chat-list-${project.id}`}>
        {project.sessions.map((session) => (
          <li key={session.id}>
            <button
              type="button"
              className={
                session.id === activeSessionId ? "pe-chat-row active" : "pe-chat-row"
              }
              onClick={() => onSelectSession(project.id, session.id)}
              title={session.id}
            >
              <span className="pe-chat-name">
                {sessionDisplayName(
                  session,
                  counts.get(session.title?.trim() || session.model || session.id) ?? 1
                )}
              </span>
              <span className="pe-chat-status">{stateLabel(t, session.status)}</span>
            </button>
          </li>
        ))}
        {tasks.map((task) => {
          const child = task.children.find((c) => c.project_id === project.id);
          return (
            <li key={task.id}>
              <button
                type="button"
                className={
                  child && child.session_id === activeSessionId
                    ? "pe-chat-row active"
                    : "pe-chat-row"
                }
                onClick={() => child && onSelectSession(project.id, child.session_id)}
                title={task.title}
              >
                <span className="pe-chat-name">{task.title}</span>
                <span className="pe-chat-status">{stateLabel(t, task.status)}</span>
              </button>
            </li>
          );
        })}
        {project.sessions.length === 0 && tasks.length === 0 && (
          <li className="pe-empty">{t("projects.chats_empty")}</li>
        )}
      </ul>
    );
  };

  return (
    <div className="project-explorer" data-testid="project-explorer">
      <section className="pe-section pe-chats">
        <ul className="pe-tree" data-testid="project-list">
          {projects.map((project) => {
            const isOpen = expanded.has(project.id);
            const showFiles = filesOpen.has(project.id);
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
                    <span className="pe-project-name">{project.display_name}</span>
                    <span className="pe-project-meta">{folderBadges(t, project)}</span>
                  </button>
                  <div className="pe-row-actions">
                    <button
                      type="button"
                      className="pe-action"
                      data-testid={`project-files-${project.id}`}
                      aria-pressed={showFiles}
                      title={t("projects.source")}
                      onClick={() => toggleFiles(project.id)}
                    >
                      {t("projects.source")}
                    </button>
                    <button
                      type="button"
                      className="pe-action danger"
                      data-testid={`project-remove-${project.id}`}
                      disabled={busyId === project.id || project.sessions.length > 0}
                      title={
                        project.sessions.length
                          ? t("projects.remove_blocked")
                          : t("projects.remove")
                      }
                      onClick={() => void runProjectAction(project, onRemoveProject)}
                    >
                      {t("projects.remove")}
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
        <button
          type="button"
          className="pe-tree-add"
          data-testid="project-add"
          disabled={busyId === "__add__"}
          aria-expanded={adding}
          onClick={() => setAdding(true)}
        >
          ＋ {t("projects.add")}
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
