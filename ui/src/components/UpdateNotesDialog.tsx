import { useEffect, useState } from "react";
import type { Translate } from "../lib/i18n";

/** v1.152 更新日志载荷（§6.2 壳 IPC `get_update_notes`）。 */
export interface UpdateNotes {
  version: string;
  notes: string | null;
}

const RELEASES_BASE = "https://github.com/wjw1-Evan/Tenon/releases/tag";

/** Tauri IPC invoke（Tauri 2 自动注入 __TAURI_INTERNALS__，main.tsx 同款）。 */
async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const internals = window.__TAURI_INTERNALS__ as
    | { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<T> }
    | undefined;
  if (!internals) throw new Error("not in Tauri");
  return internals.invoke(cmd, args);
}

/** 发布说明行的轻量可读化：`* `/`- ` 列表行转 • 前缀，其余原样（纯文本渲染，不引 markdown 依赖）。 */
export function noteLines(notes: string): { bullet: boolean; text: string }[] {
  return notes
    .split(/\r?\n/)
    .filter((line) => line.trim().length > 0)
    .map((line) => {
      const m = line.match(/^\s*[-*]\s+(.*)$/);
      return m ? { bullet: true, text: m[1] } : { bullet: false, text: line.trim() };
    });
}

/**
 * 更新成功后的更新日志启动弹窗（v1.152 / §6.2）：仅桌面壳存在（浏览器 Web 版
 * 无 __TAURI_INTERNALS__ 不渲染）。挂载即 peek 壳 IPC（不消费），「知道了」
 * 才调 dismiss_update_notes 确认，重载仍可见；不支持点击遮罩关闭——
 * 误触即标记已读，日志就再也看不到了。
 */
export function UpdateNotesDialog({ t }: { t: Translate }) {
  const [notes, setNotes] = useState<UpdateNotes | null>(null);

  useEffect(() => {
    if (!window.__TAURI_INTERNALS__) return;
    let cancelled = false;
    tauriInvoke<UpdateNotes | null>("get_update_notes")
      .then((payload) => {
        if (!cancelled && payload) setNotes(payload);
      })
      .catch(() => {
        // 壳 IPC 不可达（异常态）：静默不打扰
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!notes) return null;

  const releaseUrl = `${RELEASES_BASE}/v${notes.version}`;
  const dismiss = () => {
    setNotes(null);
    void tauriInvoke("dismiss_update_notes").catch(() => {});
  };
  const openRelease = () => {
    tauriInvoke("plugin:shell|open", { path: releaseUrl }).catch(() => {
      window.open(releaseUrl, "_blank", "noopener");
    });
  };

  const lines = notes.notes ? noteLines(notes.notes) : [];
  return (
    <div className="merge-overlay" data-testid="update-notes-overlay">
      <div
        className="merge-pane update-notes-pane"
        role="dialog"
        aria-label={t("update.notes.title", { version: notes.version })}
      >
        <div className="merge-head">
          <strong data-testid="update-notes-title">
            {t("update.notes.title", { version: notes.version })}
          </strong>
          <button
            type="button"
            className="provider-remove update-notes-close"
            aria-label={t("update.notes.dismiss")}
            data-testid="update-notes-close"
            onClick={dismiss}
          >
            ✕
          </button>
        </div>
        <div className="update-notes-body" data-testid="update-notes-body">
          <div className="update-notes-section-title">{t("update.notes.body_title")}</div>
          {lines.length > 0 ? (
            <div className="update-notes-list">
              {lines.map((line, i) =>
                line.bullet ? (
                  <div key={i} className="update-notes-item">
                    <span className="update-notes-bullet">•</span>
                    <span>{line.text}</span>
                  </div>
                ) : (
                  <div key={i} className="update-notes-line">
                    {line.text}
                  </div>
                )
              )}
            </div>
          ) : (
            <div className="update-notes-empty" data-testid="update-notes-empty">
              {t("update.notes.empty")}
            </div>
          )}
        </div>
        <div className="update-notes-foot">
          <button
            type="button"
            className="update-notes-link"
            data-testid="update-notes-link"
            onClick={openRelease}
          >
            {t("update.notes.view_full")}
          </button>
          <button
            type="button"
            className="update-notes-ok"
            data-testid="update-notes-dismiss"
            onClick={dismiss}
          >
            {t("update.notes.dismiss")}
          </button>
        </div>
      </div>
    </div>
  );
}
