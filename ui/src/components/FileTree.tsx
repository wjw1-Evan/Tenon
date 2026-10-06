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

function TreeDir({
  api,
  entry,
  projectId,
  refreshToken,
  busy,
  t,
  onOpenFile,
  onMove,
  onContextMenu,
}: {
  api: TenonApi;
  entry: Entry;
  projectId: string;
  refreshToken: number;
  busy: boolean;
  t: Translate;
  onOpenFile: (path: string) => void;
  onMove: (from: string, to: string) => void;
  onContextMenu: (entry: Entry, pos: { x: number; y: number }) => void;
}) {
  const [open, setOpen] = useState(false);
  const [dropActive, setDropActive] = useState(false);
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
    <li className={dropActive ? "tree-dir drop-target" : "tree-dir"}>
      <div
        className="tree-row"
        onContextMenu={(event) => {
          event.preventDefault();
          onContextMenu(entry, { x: event.clientX, y: event.clientY });
        }}
        onDragOver={(event) => {
          event.preventDefault();
          setDropActive(true);
        }}
        onDragLeave={() => setDropActive(false)}
        onDrop={(event) => {
          event.preventDefault();
          setDropActive(false);
          const from = event.dataTransfer.getData("application/x-tenon-path");
          if (!from || from === entry.path || from.startsWith(`${entry.path}/`)) return;
          const targetName = from.split("/").pop() ?? "";
          onMove(from, joinPath(entry.path, targetName));
        }}
      >
        <button
          type="button"
          className="tree-name"
          aria-expanded={open}
          draggable={!busy}
          onDragStart={(event) => {
            // 文件夹行可拖（v1.139）：与文件行同一私有 MIME——投入任务输入框插 @目录路径，
            // 落到其他目录行仍为移动语义。
            event.dataTransfer.setData?.("application/x-tenon-path", entry.path);
            event.dataTransfer.effectAllowed = "move";
          }}
          onClick={() => setOpen((v) => !v)}
        >
          {open ? "▾" : "▸"} {entry.name}
        </button>
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
              onMove={onMove}
              onContextMenu={onContextMenu}
            />
          ))}
          {open && loaded !== null && entries.length === 0 && (
            <li className="muted" aria-label={t("tree.empty_dir")} />
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
  onMove: (from: string, to: string) => void;
  onContextMenu: (entry: Entry, pos: { x: number; y: number }) => void;
}) {
  const { api, entry, projectId, refreshToken, busy, t, onOpenFile, onMove, onContextMenu } = props;
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
        onMove={onMove}
        onContextMenu={onContextMenu}
      />
    );
  }
  return (
    <li
      className="tree-file"
      data-status={entry.git_status}
      draggable={!busy}
      onContextMenu={(event) => {
        event.preventDefault();
        onContextMenu(entry, { x: event.clientX, y: event.clientY });
      }}
      onDragStart={(event) => {
        event.dataTransfer.setData?.("application/x-tenon-path", entry.path);
        event.dataTransfer.effectAllowed = "move";
      }}
    >
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
  const [rootDropActive, setRootDropActive] = useState(false);
  const [prompt, setPrompt] = useState<PromptState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 行右键菜单（v1.141）：文件 / 文件夹行右键 → 编辑文件名 / 删除（复用 hover 动作同流程）。
  const [menu, setMenu] = useState<{ x: number; y: number; entry: Entry } | null>(null);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setMenu(null);
    };
    window.addEventListener("click", close);
    window.addEventListener("resize", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("resize", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

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
        throw new Error(response.results[0]?.error ?? t("tree.op_failed"));
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

  const moveEntry = (from: string, to: string) => {
    if (from === to || from.startsWith(`${to}/`)) return;
    void run({ op: "move", from, to }, { type: "renamed", from, to, kind: from.includes(".") ? "file" : "dir" });
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
      <ul
        className={rootDropActive ? "tree drop-target" : "tree"}
        data-testid="file-tree"
        onDragOver={(event) => {
          event.preventDefault();
          setRootDropActive(true);
        }}
        onDragLeave={(event) => {
          if (event.currentTarget === event.target) setRootDropActive(false);
        }}
        onDrop={(event) => {
          event.preventDefault();
          setRootDropActive(false);
          const from = event.dataTransfer.getData("application/x-tenon-path");
          if (!from || !from.includes("/")) return;
          const name = from.split("/").pop() ?? "";
          moveEntry(from, name);
        }}
      >
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
            onMove={moveEntry}
            onContextMenu={(entry, pos) => setMenu({ ...pos, entry })}
          />
        ))}
      </ul>
      {menu && (
        <div
          className="tree-context"
          data-testid="tree-context-menu"
          role="menu"
          style={{ left: menu.x, top: menu.y }}
        >
          <button
            type="button"
            role="menuitem"
            disabled={busy}
            onClick={() => {
              const target = menu.entry;
              setMenu(null);
              setPrompt({
                parent: target.kind === "dir" ? target.path : parentOf(target.path),
                target,
                value: target.name,
              });
            }}
          >
            {t("tree.rename")}
          </button>
          <button
            type="button"
            role="menuitem"
            disabled={busy}
            onClick={() => {
              const target = menu.entry;
              setMenu(null);
              deleteEntry(target);
            }}
          >
            {t("tree.delete")}
          </button>
        </div>
      )}
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
