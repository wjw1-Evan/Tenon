// 源码树（§7.2 v1.107）：右区审查窗格左缘单列合并源码树——文件浏览与 git 状态
// 同树（FileTree git 状态装饰），顶部「全部 / 仅变更」过滤，列尾「源码控制」
// 折叠组收编分支 / 最近提交 / 活动 blame。v1.107 自左侧项目行内嵌文件树与
// 底部 Source 页收敛而来（GitSourcePanel 并入本组件）。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import { useResolvedLocale, type Translate } from "../lib/i18n";
import { FileTree, type FileTreeChange } from "./FileTree";

interface GitView {
  repository: boolean;
  branch: string | null;
  branches: Array<{ name: string; current: boolean; commit: string; upstream?: string }>;
  changes: Array<{
    path: string;
    old_path?: string;
    index_status: string;
    worktree_status: string;
  }>;
  commits: Array<{
    id: string;
    short_id: string;
    summary: string;
    author: string;
    email: string;
    timestamp: number;
  }>;
  blame?: {
    path: string;
    lines: Array<{
      line: number;
      commit: string;
      author: string;
      email: string;
      timestamp: number;
      summary: string;
      content: string;
    }>;
  } | null;
}

interface Props {
  api: TenonApi;
  t: Translate;
  projectId: string | null;
  /** ProjectRuntime 文件事件版本；驱动文件树与 git 视图刷新（§6.4 / §8.1）。 */
  refreshToken: number;
  /** 活动文件：blame 按其请求（§8.1 源代码视图）。 */
  activePath: string | null;
  onOpenFile: (path: string, line?: number) => void;
  onFileTreeChange: (change: FileTreeChange) => void;
  /** 收起整列（v1.107）：恢复走右区左缘细条。 */
  onCollapse: () => void;
}

type SourceFilter = "all" | "changes";

function formatTime(timestamp: number, locale: string) {
  if (!timestamp) return "";
  return new Date(timestamp * 1000).toLocaleString(locale);
}

export function SourcePanel({
  api,
  t,
  projectId,
  refreshToken,
  activePath,
  onOpenFile,
  onFileTreeChange,
  onCollapse,
}: Props) {
  const localeTag = useResolvedLocale();
  const [filter, setFilter] = useState<SourceFilter>("all");
  const [view, setView] = useState<GitView | null>(null);
  const [error, setError] = useState<string | null>(null);
  // 「源码控制」折叠组：默认收起，保持源码树轻量（§7.2）。
  const [controlOpen, setControlOpen] = useState(false);

  useEffect(() => {
    if (!projectId) return;
    let alive = true;
    api
      .getGitView(projectId, activePath)
      .then((result) => {
        if (alive) {
          setView(result);
          setError(null);
        }
      })
      .catch((e) => {
        if (alive) setError(String(e));
      });
    return () => {
      alive = false;
    };
  }, [api, projectId, activePath, refreshToken]);

  return (
    <div className="source-dock" data-testid="source-dock">
      <div className="source-dock-head">
        <span className="side-title">{t("source.title")}</span>
        {view?.repository && (
          <div className="source-filter" data-testid="source-filter">
            <button
              type="button"
              className={filter === "all" ? "active" : ""}
              data-testid="source-filter-all"
              aria-pressed={filter === "all"}
              onClick={() => setFilter("all")}
            >
              {t("source.filter_all")}
            </button>
            <button
              type="button"
              className={filter === "changes" ? "active" : ""}
              data-testid="source-filter-changes"
              aria-pressed={filter === "changes"}
              onClick={() => setFilter("changes")}
            >
              {t("source.filter_changes")}
            </button>
          </div>
        )}
        <button
          type="button"
          className="pe-collapse"
          data-testid="source-collapse"
          title={t("source.collapse")}
          aria-label={t("source.collapse")}
          onClick={onCollapse}
        >
          <svg
            width={12}
            height={12}
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth={2}
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <path d="m11 17-5-5 5-5" />
            <path d="m18 17-5-5 5-5" />
          </svg>
        </button>
      </div>
      {error && (
        <div className="tree-error" role="alert">
          {error}
        </div>
      )}
      <div className="source-dock-body">
        {!projectId || !view?.repository ? (
          <div className="muted source-dock-hint">
            {projectId ? t("source.not_git_hint") : t("source.no_project")}
          </div>
        ) : filter === "all" ? (
          <FileTree
            api={api}
            t={t}
            projectId={projectId}
            refreshToken={refreshToken}
            onOpenFile={onOpenFile}
            onOperation={onFileTreeChange}
          />
        ) : (
          <ul className="source-changes" data-testid="source-changes">
            {view.changes.map((file) => (
              <li key={`${file.path}:${file.index_status}${file.worktree_status}`}>
                <button
                  type="button"
                  onClick={() => onOpenFile(file.path)}
                  title={file.path}
                >
                  <span className="source-status">
                    {file.index_status}
                    {file.worktree_status}
                  </span>
                  <span>{file.path}</span>
                </button>
              </li>
            ))}
            {view.changes.length === 0 && <li className="muted">{t("source.clean")}</li>}
          </ul>
        )}
      </div>
      {projectId && view?.repository && (
        <div className="source-control">
          <button
            type="button"
            className="source-control-toggle"
            data-testid="source-control-toggle"
            aria-expanded={controlOpen}
            onClick={() => setControlOpen((v) => !v)}
          >
            <span aria-hidden="true">{controlOpen ? "▾" : "▸"}</span>
            {t("source.control")}
            <span className="source-control-branch muted">{view.branch ?? "HEAD"}</span>
          </button>
          {controlOpen && (
            <div className="source-control-body">
              <section className="source-section" data-testid="source-branches">
                <div className="source-label">{t("source.branches")}</div>
                <ul>
                  {view.branches.map((branch) => (
                    <li key={branch.name}>
                      <span className={branch.current ? "source-branch current" : "source-branch"}>
                        <span data-testid={`source-branch-${branch.name}`}>{branch.name}</span>
                        {branch.current ? ` · ${t("source.current")}` : ""}
                      </span>
                    </li>
                  ))}
                </ul>
              </section>
              <section className="source-section" data-testid="source-commits">
                <div className="source-label">{t("source.commits")}</div>
                <ul>
                  {view.commits.map((commit) => (
                    <li key={commit.id}>
                      <div className="source-commit">
                        <code>{commit.short_id}</code>
                        <span>{commit.summary}</span>
                      </div>
                      <div className="muted">
                        {commit.author} · {formatTime(commit.timestamp, localeTag)}
                      </div>
                    </li>
                  ))}
                </ul>
              </section>
              {view.blame && (
                <section className="source-section source-blame" data-testid="source-blame">
                  <div className="source-label">
                    {t("source.blame")}: {view.blame.path}
                  </div>
                  <ul>
                    {view.blame.lines.map((line) => (
                      <li key={line.line}>
                        <button type="button" onClick={() => onOpenFile(view.blame!.path, line.line)}>
                          <code>{line.line}</code>
                          <span className="source-meta">
                            {line.author} · {line.summary}
                          </span>
                          <span className="source-content">{line.content}</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                </section>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
