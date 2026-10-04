// 活动文件 LSP 诊断（§8.1 / §8.5 / 附录 D T4）：刷新、跳转语义与一键 AI 修复。
import { useEffect, useMemo, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

export interface EditorDiagnostic {
  path: string;
  line: number;
  column: number;
  severity: number;
  source?: string;
  code?: string;
  message: string;
}

interface Props {
  api: TenonApi;
  t: Translate;
  projectId: string | null;
  path: string | null;
  /** ProjectRuntime / 文件事件 / 项目切换触发刷新（§6.4 / §8.1）。 */
  refreshToken: number;
  sessionId: string | null;
  onOpenFile: (path: string, line?: number) => void;
  onFix: (diagnostic: EditorDiagnostic) => void;
}

function severityLabel(t: Translate, severity: number) {
  if (severity <= 1) return t("diagnostic.error");
  if (severity === 2) return t("diagnostic.warning");
  return t("diagnostic.info");
}

/** 无内置语言包的文件类型（daemon 返回 503）按「无诊断」降级，不作为错误轰炸。 */
function isNoLanguagePackError(message: string): boolean {
  return message.includes("语言包不可用");
}

function normalize(value: unknown, path: string): EditorDiagnostic[] {
  const raw = Array.isArray(value)
    ? value
    : Array.isArray((value as { items?: unknown[] })?.items)
      ? (value as { items: unknown[] }).items
      : [];
  return raw.flatMap((item) => {
    const record = item as {
      message?: unknown;
      severity?: unknown;
      source?: unknown;
      code?: unknown;
      range?: {
        start?: { line?: unknown; character?: unknown };
      };
    };
    const message = typeof record.message === "string" ? record.message : "";
    if (!message) return [];
    return [{
      path,
      line: Number(record.range?.start?.line ?? 0) + 1,
      column: Number(record.range?.start?.character ?? 0) + 1,
      severity: Number(record.severity ?? 1),
      source: typeof record.source === "string" ? record.source : undefined,
      code: record.code === null || record.code === undefined ? undefined : String(record.code),
      message,
    }];
  });
}

export function DiagnosticsPanel({
  api,
  t,
  projectId,
  path,
  refreshToken,
  sessionId,
  onOpenFile,
  onFix,
}: Props) {
  const [items, setItems] = useState<EditorDiagnostic[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [noLanguagePack, setNoLanguagePack] = useState(false);
  const [manualRefresh, setManualRefresh] = useState(0);
  const signature = useMemo(
    () => `${projectId ?? ""}\u0000${path ?? ""}\u0000${refreshToken}\u0000${manualRefresh}`,
    [projectId, path, refreshToken, manualRefresh]
  );

  useEffect(() => {
    if (!projectId || !path) {
      setItems([]);
      setError(null);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(() => {
      setLoading(true);
      api
        .lsp({ project_id: projectId, path, action: "diagnostics" })
        .then((response) => {
          if (!alive) return;
          setItems(normalize(response.result, path));
          setError(null);
          setNoLanguagePack(false);
        })
        .catch((e) => {
          if (!alive) return;
          setItems([]);
          const message = String(e);
          if (isNoLanguagePackError(message)) {
            setError(null);
            setNoLanguagePack(true);
          } else {
            setError(message);
            setNoLanguagePack(false);
          }
        })
        .finally(() => {
          if (alive) setLoading(false);
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [api, projectId, path, signature]);

  const errors = items.filter((item) => item.severity <= 1).length;
  const warnings = items.filter((item) => item.severity === 2).length;

  return (
    <section className="diagnostics-panel" data-testid="diagnostics-panel" aria-label={t("diagnostic.title")}>
      <div className="diff-title">
        {t("diagnostic.title")}
        <span className="diagnostic-summary" data-testid="diagnostic-summary">
          {t("diagnostic.error")}: {errors} · {t("diagnostic.warning")}: {warnings}
        </span>
      </div>
      {!path && <div className="muted">{t("diagnostic.no_file")}</div>}
      {path && (
        <>
          <div className="diagnostic-path muted" title={path}>{path}</div>
          {loading && <div className="muted">{t("diagnostic.loading")}</div>}
          {!loading && noLanguagePack && (
            <div className="muted" data-testid="diagnostics-no-language-pack">
              {t("diagnostic.no_language_pack")}
            </div>
          )}
          {error && (
            <div className="tree-error" role="alert">
              {t("diagnostic.unavailable")}: {error}
            </div>
          )}
          {!loading && !error && items.length === 0 && (
            <div className="muted" data-testid="diagnostics-clean">{t("diagnostic.clean")}</div>
          )}
          <ul className="diagnostic-list">
            {items.map((item, index) => (
              <li
                key={`${item.line}-${item.column}-${index}`}
                className={item.severity <= 1 ? "diagnostic error" : "diagnostic"}
                data-testid="diagnostic-item"
              >
                <div className="diagnostic-main">
                  <strong>{severityLabel(t, item.severity)}</strong>
                    <button
                      type="button"
                      className="diagnostic-location"
                      onClick={() => onOpenFile(item.path, item.line)}
                    >
                    {item.path}:{item.line}:{item.column}
                  </button>
                </div>
                <p>{item.message}</p>
                <div className="diagnostic-actions">
                  <span className="muted">
                    {[item.source, item.code].filter(Boolean).join(" ")}
                  </span>
                  <button
                    type="button"
                    className="diagnostic-fix"
                    data-testid={`diagnostic-fix-${index}`}
                    disabled={!sessionId}
                    onClick={() => onFix(item)}
                  >
                    {t("diagnostic.fix")}
                  </button>
                </div>
              </li>
            ))}
          </ul>
          <button
            type="button"
            className="diagnostic-refresh"
            data-testid="diagnostic-refresh"
            disabled={loading || !path}
            onClick={() => setManualRefresh((value) => value + 1)}
          >
            {t("diagnostic.refresh")}
          </button>
        </>
      )}
    </section>
  );
}
