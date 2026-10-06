import { useEffect, useState } from "react";
import type { Translate } from "../lib/i18n";
import { noteLines, type UpdateNotes } from "./UpdateNotesDialog";

const RELEASES_BASE = "https://github.com/wjw1-Evan/Tenon/releases/tag";

/** Tauri IPC invoke（Tauri 2 自动注入 __TAURI_INTERNALS__，UpdateNotesDialog 同款）。 */
async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const internals = window.__TAURI_INTERNALS__ as
    | { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<T> }
    | undefined;
  if (!internals) throw new Error("not in Tauri");
  return internals.invoke(cmd, args);
}

/** 发现间隔：下载完成的壳事件可能早于 UI 挂载，挂载即 peek + 轮询兜底（get_handshake 同风格）。 */
const POLL_MS = 15_000;

/**
 * 更新就绪通知卡（v1.154 / §6.2）：恒自动更新的下载完成后等待用户确认——
 * 右下角非阻断卡片展示版本与更新内容，「立即更新」经 install_update IPC
 * 安装并重启（备选外链打开 GitHub Release 手动下载，装完同为最新版）。
 * 仅桌面壳存在（浏览器 Web 版无 __TAURI_INTERNALS__ 不渲染）。
 */
export function UpdateReadyToast({ t }: { t: Translate }) {
  const [ready, setReady] = useState<UpdateNotes | null>(null);
  const [installing, setInstalling] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  /** 「下次再说」只收起本次会话提示；壳侧下载物保留，下轮检查 / 重启后仍可见。 */
  const [hidden, setHidden] = useState(false);

  useEffect(() => {
    if (!window.__TAURI_INTERNALS__) return;
    let cancelled = false;
    const peek = () =>
      tauriInvoke<UpdateNotes | null>("get_update_ready")
        .then((payload) => {
          if (!cancelled && payload) setReady(payload);
        })
        .catch(() => {
          // 壳 IPC 不可达（异常态）：静默不打扰
        });
    peek();
    const timer = window.setInterval(peek, POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  if (!ready || hidden) return null;

  const releaseUrl = `${RELEASES_BASE}/v${ready.version}`;
  const lines = ready.notes ? noteLines(ready.notes) : [];
  const install = async () => {
    setInstalling(true);
    setFailed(null);
    try {
      // 成功即重启，不会走到 then；失败回落展示原因
      await tauriInvoke("install_update");
    } catch (e) {
      setInstalling(false);
      setFailed(String(e));
    }
  };
  const openRelease = () => {
    tauriInvoke("plugin:shell|open", { path: releaseUrl }).catch(() => {
      window.open(releaseUrl, "_blank", "noopener");
    });
  };

  return (
    <div className="update-ready-toast" data-testid="update-ready-toast" role="status">
      <div className="update-ready-title" data-testid="update-ready-title">
        {t("update.ready.title", { version: ready.version })}
      </div>
      <div className="update-ready-body" data-testid="update-ready-body">
        {lines.length > 0 ? (
          lines.map((line, i) =>
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
          )
        ) : (
          <div className="update-notes-empty">{t("update.notes.empty")}</div>
        )}
      </div>
      {failed && (
        <div className="update-ready-failed" data-testid="update-ready-failed">
          {failed}
        </div>
      )}
      <div className="update-ready-foot">
        <button
          type="button"
          className="update-ready-link"
          data-testid="update-ready-link"
          onClick={openRelease}
        >
          {t("update.notes.view_full")}
        </button>
        <button
          type="button"
          className="update-ready-later"
          data-testid="update-ready-later"
          disabled={installing}
          onClick={() => setHidden(true)}
        >
          {t("update.ready.later")}
        </button>
        <button
          type="button"
          className="update-ready-install"
          data-testid="update-ready-install"
          disabled={installing}
          onClick={() => void install()}
        >
          {t("update.ready.install")}
        </button>
      </div>
    </div>
  );
}
