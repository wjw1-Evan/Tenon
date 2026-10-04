// 模型路由面板（设计方案 §11）：显式路由 + 纯读任务轻模型建议（一键采纳）。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";

interface Props {
  api: TenonApi;
  sessionId: string | null;
  onSwitched?: (model: string) => void;
}

export function ModelRoutingPanel({ api, sessionId, onSwitched }: Props) {
  const [models, setModels] = useState<
    Array<{ name: string; default_model: string; local: boolean; is_default: boolean }>
  >([]);
  const [current, setCurrent] = useState<string>("");
  const [suggestText, setSuggestText] = useState("");
  const [suggest, setSuggest] = useState<{
    pure_read: boolean;
    source: Record<string, unknown>;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [suggestOpen, setSuggestOpen] = useState(false);

  useEffect(() => {
    api.models().then((r) => {
      setModels(r.models ?? []);
      const def = (r.models ?? []).find((m) => m.is_default);
      setCurrent(def?.name ?? "");
    }).catch(() => {});
  }, [api]);

  async function suggestNow() {
    if (!suggestText.trim()) return;
    setBusy(true);
    try {
      setSuggest(await api.modelSuggest(suggestText.trim()));
    } finally {
      setBusy(false);
    }
  }

  async function switchTo(name: string) {
    if (!sessionId) return;
    setBusy(true);
    try {
      const r = await api.switchSessionModel(sessionId, name);
      setCurrent(name);
      onSwitched?.(r.model);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="model-panel" data-testid="model-panel">
      <select
        aria-label="model"
        value={current}
        disabled={!sessionId || busy}
        onChange={(e) => switchTo(e.target.value)}
        data-testid="model-select"
      >
        {models.map((m) => (
          <option key={m.name} value={m.name}>
            {m.name}
            {m.local ? "（本地 · 0 成本）" : ""} — {m.default_model}
          </option>
        ))}
      </select>
      <button
        type="button"
        className={suggestOpen ? "model-suggest-toggle open" : "model-suggest-toggle"}
        data-testid="suggest-toggle"
        title="路由建议"
        aria-label="路由建议"
        aria-expanded={suggestOpen}
        onClick={() => setSuggestOpen((v) => !v)}
      >
        ✦
      </button>
      {suggestOpen && (
        <div className="model-suggest-pop" data-testid="suggest-popover">
          <div className="model-row">
            <input
              value={suggestText}
              placeholder="输入任务，获取路由建议…"
              onChange={(e) => setSuggestText(e.target.value)}
              data-testid="suggest-input"
            />
            <button onClick={suggestNow} disabled={busy} data-testid="suggest-btn">建议</button>
          </div>
          {suggest && (
            <div className="model-suggest" data-testid="suggest-result">
              {suggest.pure_read
                ? "纯读任务——建议轻模型（一键采纳不自动改派，§11）"
                : "写任务——保持默认模型"}
              <span className="muted">
                {" "}
                ({JSON.stringify(suggest.source)})
              </span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
