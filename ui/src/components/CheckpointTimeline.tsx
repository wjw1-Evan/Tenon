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
}

export function CheckpointTimeline({ api, t, sessionId }: Props) {
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
    } finally {
      setBusy(false);
    }
  }

  /** 撤销最近一次回滚（§10.3 unrevert）：会话级控制指令。 */
  async function unrevert() {
    if (!sessionId) return;
    setBusy(true);
    try {
      await api.control(sessionId, "unrollback");
    } catch {
      // 无可撤销的回滚时静默（时间轴保持原状）
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="timeline" data-testid="timeline">
      <div className="timeline-head">
        <span>{t("panel.timeline")}</span>
        <button
          type="button"
          className="timeline-unrevert"
          disabled={busy || !sessionId}
          onClick={() => void unrevert()}
        >
          {t("timeline.unrevert")}
        </button>
      </div>
      {checkpoints.length === 0 && <div className="muted">{t("timeline.empty")}</div>}
      <ul>
        {[...checkpoints].reverse().map((cp) => (
          <li key={cp.id} className="timeline-node">
            <code className="tree-oid">{cp.tree.slice(0, 8)}</code>
            <time
              className="timeline-time"
              dateTime={cp.created_at}
              title={new Date(cp.created_at).toLocaleString()}
            >
              {new Date(cp.created_at).toLocaleTimeString()}
            </time>
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
