// Checkpoint 时间轴（设计方案 §7.1 / §10.3）：事件 + 快照点；回滚 / 撤销回滚。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface Checkpoint {
  id: string;
  tree: string;
  files: string[];
  created_at: string;
}

interface Props {
  api: TenonApi;
  t: Translate;
  sessionId: string | null;
  onRolledBack?: () => void;
}

export function CheckpointTimeline({ api, t, sessionId, onRolledBack }: Props) {
  const [checkpoints, setCheckpoints] = useState<Checkpoint[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!sessionId) return;
    let alive = true;
    const load = () =>
      api.checkpoints(sessionId).then((r) => {
        if (alive) setCheckpoints(r.checkpoints ?? []);
      }).catch(() => {});
    load();
    const timer = setInterval(load, 1500);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [api, sessionId]);

  async function rollback(id: string) {
    setBusy(true);
    try {
      await api.rollbackCheckpoint(id);
      onRolledBack?.();
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="timeline" data-testid="timeline">
      <div className="timeline-head">{t("panel.timeline")}</div>
      {checkpoints.length === 0 && <div className="muted">{t("timeline.empty")}</div>}
      <ul>
        {[...checkpoints].reverse().map((cp) => (
          <li key={cp.id} className="timeline-node">
            <code className="tree-oid">{cp.tree.slice(0, 8)}</code>
            <span className="files">{cp.files.join(", ") || "—"}</span>
            <button disabled={busy} onClick={() => rollback(cp.id)}>
              {t("timeline.rollback")}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
