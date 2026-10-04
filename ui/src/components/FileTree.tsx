// 文件树（设计方案 §8.1）：懒加载层级、git 状态装饰、watcher 同步与项目内
// 重命名 / 移动 / 删除（v1.72 起不含创建——项目内创建由会话大模型决策执行）。
import { useEffect, useState } from "react";
import type { FileOperation, TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface Entry {
  path: string;
  name: string;
  kind: "dir" | "file";
  git_status: string;
}

export type FileTreeChange =
  | { type: "renamed"; from: string; to: string; kind: Entry["kind"] }
  | { type: "deleted"; path: string };

interface Props {
  api: TenonApi;
  t: Translate;
  projectId: string | null;
  /** ProjectRuntime 文件事件版本；变化即刷新（§6.4 / §8.1）。 */
  refreshToken?: number;
  onOpenFile: (path: string) => void;
  onOperation?: (change: FileTreeChange) => void;
}

const STATUS_DOT: Record<string, string> = {
  modified: "var(--modified, #d9a514)",
  added: "var(--ok, #2da44e)",
  untracked: "var(--ok, #2da44e)",
  deleted: "var(--error, #d43d3d)",
  renamed: "var(--verify, #7d4fd3)",
};

interface PromptState {
  parent: string;
  target: Entry;
  value: string;
}

function parentOf(path: string) {
  const parts = path.split("/");
  parts.pop();
  return parts.join("/");
}

function joinPath(parent: string, name: string) {
  return parent ? `${parent}/${name}` : name;
}

function loadEntries(
  api: TenonApi,
  projectId: string,
  path: string,
  alive: boolean,
  setter: (entries: Entry[]) => void
) {
  api
    .tree(projectId, path)
    .then((r) => {
      if (alive) setter(r.entries ?? []);
    })
    .catch(() => {
      if (alive) setter([]);
    });
}

function ActionButton({
  label,
  onClick,
  disabled,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className="tree-action"
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={(event) => {
        event.stopPropagation();
        onClick();
      }}
    >
      {label === "delete" ? "✕" : "✎"}
    </button>
  );
}

function TreeActions({
  entry,
  t,
  busy,
  onPrompt,
  onDelete,
}: {
  entry?: Entry;
  t: Translate;
  busy: boolean;
  onPrompt: (state: PromptState) => void;
  onDelete: (entry: Entry) => void;
}) {
  const parent = entry ? (entry.kind === "dir" ? entry.path : parentOf(entry.path)) : "";
  return (
    <span className="tree-actions">
      {entry && (
        <ActionButton
          label={t("tree.rename")}
          disabled={busy}
          onClick={() => onPrompt({ parent, target: entry, value: entry.name })}
        />
      )}
      {entry && (
        <ActionButton
          label={t("tree.delete")}
          disabled={busy}
          onClick={() => onDelete(entry)}
        />
      )}
    </span>
  );
}

function TreeDir({
  api,
  entry,
  projectId,
  refreshToken,
  busy,
  t,
  onOpenFile,
  onPrompt,
  onDelete,
}: {
  api: TenonApi;
  entry: Entry;
  projectId: string;
  refreshToken: number;
  busy: boolean;
  t: Translate;
  onOpenFile: (path: string) => void;
  onPrompt: (state: PromptState) => void;
  onDelete: (entry: Entry) => void;
}) {
  const [open, setOpen] = useState(false);
  const [entries, setEntries] = useState<Entry[]>([]);
  const [loaded, setLoaded] = useState<{ path: string; token: number } | null>(null);

  useEffect(() => {
    if (!open) return;
    if (loaded?.path === entry.path && loaded.token === refreshToken) return;
    let alive = true;
    loadEntries(api, projectId, entry.path, alive, (next) => {
      if (!alive) return;
      setEntries(next);
      setLoaded({ path: entry.path, token: refreshToken });
    });
    return () => {
      alive = false;
    };
  }, [api, projectId, entry.path, open, refreshToken, loaded]);

  return (
    <li className="tree-dir">
      <div className="tree-row">
        <button
          type="button"
          className="tree-name"
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
        >
          {open ? "▾" : "▸"} {entry.name}
        </button>
        <TreeActions
          entry={entry}
          t={t}
          busy={busy}
          onPrompt={onPrompt}
          onDelete={onDelete}
        />
      </div>
      {open && (
        <ul>
          {entries.map((child) => (
            <TreeEntryRow
              key={child.path}
              api={api}
              entry={child}
              projectId={projectId}
              refreshToken={refreshToken}
              busy={busy}
              t={t}
              onOpenFile={onOpenFile}
              onPrompt={onPrompt}
              onDelete={onDelete}
            />
          ))}
          {open && loaded !== null && entries.length === 0 && (
            <li className="muted" aria-label="empty directory" />
          )}
        </ul>
      )}
    </li>
  );
}

function TreeEntryRow(props: {
  api: TenonApi;
  entry: Entry;
  projectId: string;
  refreshToken: number;
  busy: boolean;
  t: Translate;
  onOpenFile: (path: string) => void;
  onPrompt: (state: PromptState) => void;
  onDelete: (entry: Entry) => void;
}) {
  const { api, entry, projectId, refreshToken, busy, t, onOpenFile, onPrompt, onDelete } = props;
  if (entry.kind === "dir") {
    return (
      <TreeDir
        api={api}
        entry={entry}
        projectId={projectId}
        refreshToken={refreshToken}
        busy={busy}
        t={t}
        onOpenFile={onOpenFile}
        onPrompt={onPrompt}
        onDelete={onDelete}
      />
    );
  }
  return (
    <li className="tree-file" data-status={entry.git_status}>
      <div className="tree-row">
        <button type="button" className="tree-name" onClick={() => onOpenFile(entry.path)}>
          {entry.name}
        </button>
        {STATUS_DOT[entry.git_status] && (
          <span
            className="status-dot"
            style={{ background: STATUS_DOT[entry.git_status] }}
            aria-label={entry.git_status}
          />
        )}
        <TreeActions
          entry={entry}
          t={t}
          busy={busy}
          onPrompt={onPrompt}
          onDelete={onDelete}
        />
      </div>
    </li>
  );
}

export function FileTree({
  api,
  t,
  projectId,
  refreshToken = 0,
  onOpenFile,
  onOperation,
}: Props) {
  const [entries, setEntries] = useState<Entry[]>([]);
  const [prompt, setPrompt] = useState<PromptState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!projectId) return;
    let alive = true;
    loadEntries(api, projectId, "", alive, (next) => setEntries(next));
    return () => {
      alive = false;
    };
  }, [api, projectId, refreshToken]);

  const run = async (operation: FileOperation, change: FileTreeChange) => {
    if (!projectId) return;
    setBusy(true);
    setError(null);
    try {
      const response = await api.fileOps(projectId, [operation]);
      if (!response.results[0]?.ok) {
        throw new Error(response.results[0]?.error ?? "operation failed");
      }
      onOperation?.(change);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const rename = async (state: PromptState, name: string) => {
    const entry = state.target;
    const to = joinPath(state.parent, name);
    await run(
      { op: "rename", from: entry.path, to },
      { type: "renamed", from: entry.path, to, kind: entry.kind }
    );
  };

  const submitPrompt = () => {
    if (!prompt || !prompt.value.trim()) return;
    const name = prompt.value.trim();
    void rename(prompt, name).finally(() => setPrompt(null));
  };

  const deleteEntry = (entry: Entry) => {
    if (!window.confirm(`delete ${entry.path}?`)) return;
    void run({ op: "delete", path: entry.path }, { type: "deleted", path: entry.path });
  };

  if (!projectId) {
    return (
      <div className="tree muted" data-testid="file-tree">
        {t("tree.empty")}
      </div>
    );
  }

  return (
    <div className="file-tree-shell">
      <div className="tree-toolbar">
        {busy && <span className="tree-busy">{t("tree.working")}</span>}
      </div>
      {error && (
        <div className="tree-error" role="alert" data-testid="tree-error">
          {error}
        </div>
      )}
      <ul className="tree" data-testid="file-tree">
        {entries.map((entry) => (
          <TreeEntryRow
            key={entry.path}
            api={api}
            entry={entry}
            projectId={projectId}
            refreshToken={refreshToken}
            busy={busy}
            t={t}
            onOpenFile={onOpenFile}
            onPrompt={setPrompt}
            onDelete={deleteEntry}
          />
        ))}
      </ul>
      {prompt && (
        <div className="tree-prompt-overlay" role="dialog" aria-modal="true">
          <form
            className="tree-prompt"
            onSubmit={(event) => {
              event.preventDefault();
              submitPrompt();
            }}
          >
            <label htmlFor="tree-prompt-input">{t("tree.rename")}</label>
            <input
              id="tree-prompt-input"
              value={prompt.value}
              autoFocus
              onChange={(event) => setPrompt({ ...prompt, value: event.target.value })}
            />
            <div className="tree-prompt-actions">
              <button type="button" onClick={() => setPrompt(null)}>
                {t("tree.cancel")}
              </button>
              <button type="submit" disabled={!prompt.value.trim()}>
                {t("tree.save")}
              </button>
            </div>
          </form>
        </div>
      )}
    </div>
  );
}
