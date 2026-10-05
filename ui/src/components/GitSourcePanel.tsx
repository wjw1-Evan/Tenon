// Git source view（§8.1）：branch / changes / commits / inline blame。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

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
  activePath: string | null;
  refreshToken: number;
  onOpenFile: (path: string, line?: number) => void;
}

function formatTime(timestamp: number) {
  if (!timestamp) return "";
  return new Date(timestamp * 1000).toLocaleString();
}

export function GitSourcePanel({
  api,
  t,
  projectId,
  activePath,
  refreshToken,
  onOpenFile,
}: Props) {
  const [view, setView] = useState<GitView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!projectId) return;
    let alive = true;
    setLoading(true);
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
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [api, projectId, activePath, refreshToken]);

  if (!projectId) {
    return <div className="git-source muted">{t("source.no_project")}</div>;
  }

  return (
    <div className="git-source" data-testid="git-source">
      <div className="source-head">
        <strong>{t("source.title")}</strong>
        <span className="muted">
          {loading
            ? t("source.loading")
            : view?.repository
              ? `${t("source.branch")}: ${view.branch ?? "HEAD"}`
              : t("source.not_git")}
        </span>
      </div>
      {error && (
        <div className="tree-error" role="alert">
          {error}
        </div>
      )}
      {view?.repository && (
        <div className="source-grid">
          <section className="source-section" data-testid="source-changes">
            <div className="source-label">{t("source.changes")}</div>
            <ul>
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
          </section>

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
                    {commit.author} · {formatTime(commit.timestamp)}
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
      {!view?.repository && <div className="muted">{t("source.not_git_hint")}</div>}
    </div>
  );
}
