// 文件树（设计方案 §8.1）：git 状态装饰；点击在编辑器打开。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface Entry {
  path: string;
  name: string;
  kind: "dir" | "file";
  git_status: string;
}

interface Props {
  api: TenonApi;
  t: Translate;
  projectId: string | null;
  onOpenFile: (path: string) => void;
}

const STATUS_DOT: Record<string, string> = {
  modified: "#d9a514",
  added: "#2da44e",
  untracked: "#2da44e",
  deleted: "#d43d3d",
  renamed: "#7d4fd3",
};

export function FileTree({ api, t, projectId, onOpenFile }: Props) {
  const [entries, setEntries] = useState<Entry[]>([]);

  useEffect(() => {
    if (!projectId) return;
    let alive = true;
    api.tree(projectId).then((r) => {
      if (alive) setEntries(r.entries ?? []);
    }).catch(() => {});
    return () => {
      alive = false;
    };
  }, [api, projectId]);

  if (!projectId) {
    return <div className="tree muted" data-testid="file-tree">{t("tree.empty")}</div>;
  }

  return (
    <ul className="tree" data-testid="file-tree">
      {entries.map((e) => (
        <li
          key={e.path}
          className={e.kind === "dir" ? "tree-dir" : "tree-file"}
          onClick={() => e.kind === "file" && onOpenFile(e.path)}
          data-status={e.git_status}
        >
          <span className="tree-name">{e.name}</span>
          {STATUS_DOT[e.git_status] && (
            <span
              className="status-dot"
              style={{ background: STATUS_DOT[e.git_status] }}
              aria-label={e.git_status}
            />
          )}
        </li>
      ))}
    </ul>
  );
}
