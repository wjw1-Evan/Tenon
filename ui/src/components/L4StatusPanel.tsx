// L4 索引诊断（§10.1 / v1.35）：WS 状态推送 + 权威 stats 快照，不再固定轮询。
import { useCallback, useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import { useResolvedLocale, type Translate } from "../lib/i18n";

interface L4Status {
  project_id: string;
  chunks: number;
  status?: {
    state: string;
    updated_at: string;
    error?: string | null;
  };
}

interface Props {
  api: TenonApi;
  t: Translate;
  projectId: string | null;
}

export function L4StatusPanel({ api, t, projectId }: Props) {
  const localeTag = useResolvedLocale();
  const [status, setStatus] = useState<L4Status | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [rebuilding, setRebuilding] = useState(false);

  const refresh = useCallback(async () => {
    if (!projectId) return;
    try {
      setStatus(await api.l4Stats(projectId));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [api, projectId]);

  useEffect(() => {
    if (!projectId) return;
    let alive = true;
    let socket: WebSocket | null = null;
    let retry: number | null = null;
    void refresh();

    const handleEvent = (raw: unknown) => {
      const event = raw as { type?: string; project_id?: string };
      if (alive && event.type === "l4_status" && event.project_id === projectId) {
        void refresh();
      }
    };

    const connect = async () => {
      if (!alive) return;
      try {
        socket = await api.connectEvents(handleEvent, projectId);
        socket.onclose = () => {
          if (!alive) return;
          retry = window.setTimeout(() => void connect(), 1000);
        };
        // 订阅建立后补拉一次，避免初始 fetch 与 WS 订阅之间的窗口漏掉最新状态。
        void refresh();
      } catch {
        if (alive) retry = window.setTimeout(() => void connect(), 1000);
      }
    };
    void connect();

    return () => {
      alive = false;
      if (retry !== null) window.clearTimeout(retry);
      socket?.close();
    };
  }, [api, projectId, refresh]);

  const rebuild = async () => {
    if (!projectId) return;
    setRebuilding(true);
    try {
      await api.l4Rebuild(projectId);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setRebuilding(false);
    }
  };

  if (!projectId) {
    return (
      <div className="l4-panel muted" data-testid="l4-panel">
        {t("l4.no_project")}
      </div>
    );
  }

  const state = status?.status?.state ?? "idle";
  return (
    <div className="l4-panel" data-testid="l4-panel" aria-label={t("l4.title")}>
      <div className="l4-head">
        <strong>{t("l4.title")}</strong>
        <span
          className={`l4-state l4-${state}`}
          data-testid="l4-state"
          aria-label={state}
        >
          {t(`l4.state.${state}`, { defaultValue: state })}
        </span>
      </div>
      <div className="l4-metrics">
        <span data-testid="l4-chunks">
          {t("l4.chunks")}: {status?.chunks ?? "—"}
        </span>
        {status?.status?.updated_at && (
          <span className="muted">{new Date(status.status.updated_at).toLocaleTimeString(localeTag)}</span>
        )}
      </div>
      {status?.status?.error && (
        <div className="tree-error" role="alert">
          {status.status.error}
        </div>
      )}
      {error && (
        <div className="tree-error" role="alert">
          {error}
        </div>
      )}
      <button
        type="button"
        className="l4-rebuild"
        disabled={rebuilding || state === "queued" || state === "indexing"}
        onClick={() => void rebuild()}
        data-testid="l4-rebuild"
      >
        {rebuilding || state === "queued" || state === "indexing"
          ? t("l4.working")
          : t("l4.rebuild")}
      </button>
    </div>
  );
}
