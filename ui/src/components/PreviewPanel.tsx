// 浏览器预览面板（§7.6 v2.77）：底部面板第五 tab——用户在输入框填本地 dev server
// 地址（3000 / localhost:3000 / http://127.0.0.1:3000/x 均可），面板经 daemon 反代
//（preview.rs：剥 X-Frame-Options / CSP frame-ancestors + 改写根绝对 URL）把页面
// 嵌进 iframe，并通过反代注入的控制台桥把 console / onerror / unhandledrejection
// 汇到面板内。跨源 DevTools 拿不到的控制台输出，这里是唯一取径。
//
// 桥消息用频道号（`?__tenon=<id>`）自证来源：iframe.contentWindow 在 jsdom 里可能
// 为空，频道号是更可靠且可测的归属判定。
import { useCallback, useEffect, useRef, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { parsePreviewTarget, previewFrameUrl, type PreviewTarget } from "../lib/preview";

interface ConsoleEntry {
  id: number;
  level: string;
  text: string;
  ts: number;
}

/** 控制台保留上限（超出丢最旧——长时间运行的页面日志会无限增长）。 */
const MAX_LOGS = 500;
/** 视口档（0 = 自适应面板宽度）。 */
const WIDTHS: { value: number }[] = [{ value: 0 }, { value: 375 }, { value: 768 }, { value: 1024 }];

function storageKey(projectId: string | null): string {
  return `tenon:preview:${projectId ?? "default"}`;
}

function readSaved(projectId: string | null): string {
  try {
    return localStorage.getItem(storageKey(projectId)) ?? "";
  } catch {
    return "";
  }
}

export function PreviewPanel({
  api,
  projectId,
  t,
}: {
  api: TenonApi;
  projectId: string | null;
  t: Translate;
}) {
  const [input, setInput] = useState(() => readSaved(projectId));
  const [target, setTarget] = useState<PreviewTarget | null>(null);
  const [src, setSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [frameKey, setFrameKey] = useState(0);
  const [width, setWidth] = useState(0);
  const [logs, setLogs] = useState<ConsoleEntry[]>([]);
  const [showLogs, setShowLogs] = useState(true);
  const [currentUrl, setCurrentUrl] = useState<string>("");
  const channelRef = useRef<string>(
    `p${Math.random().toString(36).slice(2, 10)}`
  );
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const consoleRef = useRef<HTMLDivElement | null>(null);
  const nextIdRef = useRef(1);

  /** 打开 / 刷新：解析输入 → 取反代前缀 → 组 iframe 地址。 */
  const open = useCallback(
    async (raw: string) => {
      const parsed = parsePreviewTarget(raw);
      if (!parsed) {
        setTarget(null);
        setSrc(null);
        setError(t("preview.invalid"));
        return;
      }
      try {
        const info = await api.previewInfo();
        const url = new URL(previewFrameUrl(info.origin, info.prefix, parsed));
        url.searchParams.set("__tenon", channelRef.current);
        setTarget(parsed);
        setError(null);
        setLogs([]);
        setCurrentUrl(parsed.url);
        setSrc(url.toString());
        setFrameKey((k) => k + 1);
        try {
          localStorage.setItem(storageKey(projectId), raw.trim());
        } catch {
          // 隐私模式等不可写：仅本次会话生效
        }
      } catch {
        setError(t("preview.unavailable"));
      }
    },
    [api, projectId, t]
  );

  // 项目切换：换回该项目的记忆地址并重开
  useEffect(() => {
    const saved = readSaved(projectId);
    setInput(saved);
    setTarget(null);
    setSrc(null);
    setError(null);
    setLogs([]);
    setCurrentUrl("");
    if (saved) void open(saved);
    // open 依赖 projectId/t，此处即项目切换语义，故只随 projectId 触发
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);

  // 控制台桥：只接受本面板 iframe 的频道号
  useEffect(() => {
    const onMessage = (ev: MessageEvent) => {
      const data = ev.data as
        | { __tenon_preview?: boolean; channel?: string; kind?: string; level?: string; text?: string; ts?: number; url?: string }
        | undefined;
      if (!data || data.__tenon_preview !== true) return;
      if (data.channel !== channelRef.current) return;
      if (data.kind === "console") {
        const entry: ConsoleEntry = {
          id: nextIdRef.current++,
          level: data.level ?? "log",
          text: data.text ?? "",
          ts: data.ts ?? Date.now(),
        };
        setLogs((prev) => [...prev, entry].slice(-MAX_LOGS));
      } else if (data.kind === "nav") {
        setCurrentUrl(data.url ?? "");
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, []);

  // 新日志滚动到底
  useEffect(() => {
    const host = consoleRef.current;
    if (host) host.scrollTop = host.scrollHeight;
  }, [logs]);

  const onSubmit = () => void open(input);

  return (
    <div className="preview-panel" data-testid="preview-panel">
      <div className="preview-toolbar">
        <input
          className="preview-url"
          data-testid="preview-url"
          value={input}
          placeholder={t("preview.url_placeholder")}
          spellCheck={false}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") onSubmit();
          }}
        />
        <button type="button" data-testid="preview-go" onClick={onSubmit}>
          {t("preview.open")}
        </button>
        <button
          type="button"
          data-testid="preview-reload"
          title={t("preview.reload")}
          disabled={!src}
          onClick={() => {
            setFrameKey((k) => k + 1);
            setLogs([]);
          }}
        >
          ⟳
        </button>
        <button
          type="button"
          data-testid="preview-external"
          title={t("preview.external")}
          disabled={!target}
          onClick={() => {
            if (target) window.open(target.url, "_blank", "noopener");
          }}
        >
          ↗
        </button>
        <label className="preview-width">
          {t("preview.width")}
          <select
            data-testid="preview-width"
            value={width}
            onChange={(e) => setWidth(Number(e.target.value))}
          >
            {WIDTHS.map((w) => (
              <option key={w.value} value={w.value}>
                {w.value === 0 ? t("preview.width_fit") : `${w.value}px`}
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          data-testid="preview-console-toggle"
          className={showLogs ? "active" : ""}
          onClick={() => setShowLogs((v) => !v)}
        >
          {t("preview.console")} {logs.length > 0 ? `(${logs.length})` : ""}
        </button>
      </div>
      {error && (
        <div className="preview-error" data-testid="preview-error" role="alert">
          {error}
        </div>
      )}
      <div className="preview-body">
        <div className="preview-stage">
          {src ? (
            <iframe
              key={frameKey}
              ref={iframeRef}
              data-testid="preview-frame"
              title={t("panel.preview")}
              src={src}
              className="preview-frame"
              style={{ width: width === 0 ? "100%" : `${width}px` }}
            />
          ) : (
            <div className="preview-empty" data-testid="preview-empty">
              {t("preview.empty")}
            </div>
          )}
        </div>
        {showLogs && (
          <div className="preview-console">
            <div className="preview-console-head">
              <span data-testid="preview-address" title={currentUrl}>
                {currentUrl || t("preview.console")}
              </span>
              <button
                type="button"
                data-testid="preview-console-clear"
                onClick={() => setLogs([])}
              >
                {t("preview.clear")}
              </button>
            </div>
            <div
              className="preview-console-body"
              data-testid="preview-console"
              ref={consoleRef}
            >
              {logs.length === 0 ? (
                <div className="preview-console-empty">{t("preview.logs_empty")}</div>
              ) : (
                logs.map((entry) => (
                  <div
                    key={entry.id}
                    className={`preview-log preview-log-${entry.level}`}
                    data-testid={`preview-log-${entry.level}`}
                  >
                    <span className="preview-log-level">{entry.level}</span>
                    <span className="preview-log-text">{entry.text}</span>
                  </div>
                ))
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
