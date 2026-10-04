// 模型路由（设计方案 §11 / §7.2 v1.46，v1.51 起内嵌代理面板任务输入框底行）：
// 「当前模型按钮 → 模型选择对话框」，参考主流 coding agent 的 model picker——
// 对话框列 GET /models 全部 provider，当前项高亮，点击经 POST /session/:id/model
// 会话级热切换（上下文随迁）；✦ 路由建议收进对话框底部。全局默认模型在设置面板模型分区（v1.40）。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface ModelEntry {
  name: string;
  default_model: string;
  local: boolean;
  is_default: boolean;
}

/** provider 命名约定（附录 E）：`glm` / `glm_responses` / `glm_anthropic` 为同
 * 端点的三种协议封装；对话框按后缀标注协议，避免同名模型三选一无可辨差异。 */
function protocolLabel(t: Translate, name: string): string {
  if (name.endsWith("_responses")) return t("model.protocol_responses");
  if (name.endsWith("_anthropic")) return t("model.protocol_anthropic");
  return t("model.protocol_openai");
}

interface Props {
  api: TenonApi;
  sessionId: string | null;
  t: Translate;
  onSwitched?: (model: string) => void;
}

export function ModelRoutingPanel({ api, sessionId, t, onSwitched }: Props) {
  const [models, setModels] = useState<ModelEntry[]>([]);
  const [current, setCurrent] = useState<string>("");
  const [dialogOpen, setDialogOpen] = useState(false);
  const [suggestText, setSuggestText] = useState("");
  const [suggest, setSuggest] = useState<{
    pure_read: boolean;
    source: Record<string, unknown>;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.models().then((r) => {
      setModels(r.models ?? []);
      const def = (r.models ?? []).find((m) => m.is_default);
      setCurrent(def?.name ?? "");
    }).catch(() => {});
  }, [api]);

  // Esc 关闭对话框（打开中才挂）
  useEffect(() => {
    if (!dialogOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setDialogOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dialogOpen]);

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
    setBusy(true);
    setError(null);
    try {
      const r = await api.switchSessionModel(sessionId ?? "", name);
      setCurrent(name);
      onSwitched?.(r.model);
      setDialogOpen(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const activeModel = models.find((m) => m.name === current);

  return (
    <div className="model-panel">
      <button
        type="button"
        className="model-trigger"
        data-testid="model-trigger"
        aria-label={t("model.dialog_title")}
        aria-haspopup="dialog"
        aria-expanded={dialogOpen}
        onClick={() => setDialogOpen((v) => !v)}
      >
        <span className="model-trigger-name" data-testid="model-current">
          {current || t("model.none")}
        </span>
        {activeModel?.default_model ? (
          <span className="model-trigger-model">{activeModel.default_model}</span>
        ) : null}
        <span className="model-trigger-caret" aria-hidden="true">▾</span>
      </button>

      {dialogOpen && (
        <div
          className="merge-overlay model-overlay"
          data-testid="model-dialog-overlay"
          onClick={() => setDialogOpen(false)}
        >
          <div
            className="merge-pane model-dialog"
            role="dialog"
            aria-label={t("model.dialog_title")}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="merge-head">
              <strong>{t("model.dialog_title")}</strong>
              <button
                type="button"
                className="model-dialog-close"
                aria-label={t("model.close")}
                data-testid="model-dialog-close"
                onClick={() => setDialogOpen(false)}
              >
                ✕
              </button>
            </div>
            <p className="muted model-dialog-note">
              {sessionId
                ? t("model.session_hint")
                : t("model.no_session_hint")}
            </p>
            <div className="model-list" data-testid="model-list">
              {models.map((m) => {
                const active = m.name === current;
                return (
                  <button
                    type="button"
                    key={m.name}
                    className={active ? "model-option active" : "model-option"}
                    data-testid={`model-option-${m.name}`}
                    disabled={!sessionId || busy}
                    aria-pressed={active}
                    onClick={() => void switchTo(m.name)}
                  >
                    <span className="model-option-check" aria-hidden="true">
                      {active ? "✓" : ""}
                    </span>
                    <span className="model-option-name">{m.name}</span>
                    <span className="muted model-option-meta">
                      {m.default_model}
                      {` · ${protocolLabel(t, m.name)}`}
                      {m.local ? ` · ${t("model.local_free")}` : ""}
                    </span>
                  </button>
                );
              })}
              {models.length === 0 && (
                <p className="muted model-option-empty">{t("model.empty")}</p>
              )}
            </div>
            {error && <div role="alert">{error}</div>}
            <div className="model-suggest-foot">
              <input
                value={suggestText}
                placeholder={t("model.suggest_placeholder")}
                aria-label={t("model.suggest_placeholder")}
                onChange={(e) => setSuggestText(e.target.value)}
                data-testid="suggest-input"
              />
              <button onClick={suggestNow} disabled={busy} data-testid="suggest-btn">
                {t("model.suggest_btn")}
              </button>
            </div>
            {suggest && (
              <div className="model-suggest" data-testid="suggest-result">
                {suggest.pure_read
                  ? t("model.suggest_read")
                  : t("model.suggest_write")}
                <span className="muted"> ({JSON.stringify(suggest.source)})</span>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
