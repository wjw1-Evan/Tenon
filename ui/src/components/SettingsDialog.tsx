// 设置面板（§7.2 / §15）：外观 / 语言 / 会话默认档 / 代理参数。
// 外观与语言即时生效；会话与代理参数经 PUT /settings 持久化（新会话生效）。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { LOCALE_CHANGE, type Locale } from "../lib/i18n";
import {
  applyTheme,
  loadThemePreference,
  saveThemePreference,
  type ThemePreference,
} from "../lib/theme";

export interface SettingsData {
  session: {
    mode?: string;
    first_edit_buffer_ms?: number;
    approval_timeout_s?: number;
    [k: string]: unknown;
  };
  exec?: { command_timeout_s?: number; [k: string]: unknown };
  [k: string]: unknown;
}

interface Props {
  api: TenonApi;
  t: Translate;
  settings: SettingsData | null;
  onClose: () => void;
  onSaved: (s: SettingsData) => void;
}

export function SettingsDialog({ api, t, settings, onClose, onSaved }: Props) {
  const [theme, setTheme] = useState<ThemePreference>(() => loadThemePreference());
  const [locale, setLocale] = useState<Locale>("auto");
  const [mode, setMode] = useState("interactive");
  const [bufferMs, setBufferMs] = useState(2000);
  const [approvalTimeout, setApprovalTimeout] = useState(120);
  const [commandTimeout, setCommandTimeout] = useState(120);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // 从已拉取的全局设置回填（外观 / 语言为本地即时项，不入 daemon）
  useEffect(() => {
    if (!settings) return;
    setMode(String(settings.session?.mode ?? "interactive"));
    setBufferMs(Number(settings.session?.first_edit_buffer_ms ?? 2000));
    setApprovalTimeout(Number(settings.session?.approval_timeout_s ?? 120));
    setCommandTimeout(Number(settings.exec?.command_timeout_s ?? 120));
  }, [settings]);

  if (!settings) return null;

  async function save() {
    setBusy(true);
    setError(null);
    try {
      saveThemePreference(theme);
      applyTheme(theme);
      window.dispatchEvent(new CustomEvent(LOCALE_CHANGE, { detail: locale }));
      const next = await api.putSettings({
        session: {
          mode,
          first_edit_buffer_ms: bufferMs,
          approval_timeout_s: approvalTimeout,
        },
        exec: { command_timeout_s: commandTimeout },
      });
      onSaved(next);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="merge-overlay" data-testid="settings-overlay">
      <div className="merge-pane settings-pane" role="dialog" aria-label={t("settings.title")}>
        <div className="merge-head">
          <strong>{t("settings.title")}</strong>
        </div>

        <div className="settings-grid">
          <label htmlFor="set-theme">{t("settings.theme")}</label>
          <select
            id="set-theme"
            value={theme}
            onChange={(e) => {
              const v = e.target.value as ThemePreference;
              setTheme(v);
              saveThemePreference(v);
              applyTheme(v);
            }}
          >
            <option value="system">{t("theme.system")}</option>
            <option value="dark">{t("theme.dark")}</option>
            <option value="light">{t("theme.light")}</option>
          </select>

          <label htmlFor="set-locale">{t("settings.language")}</label>
          <select
            id="set-locale"
            value={locale}
            onChange={(e) => setLocale(e.target.value as Locale)}
          >
            <option value="auto">Auto</option>
            <option value="en">English</option>
            <option value="zh-CN">中文</option>
          </select>

          <label htmlFor="set-mode">{t("settings.mode")}</label>
          <select id="set-mode" value={mode} onChange={(e) => setMode(e.target.value)}>
            <option value="interactive">{t("settings.mode_interactive")}</option>
            <option value="auto">{t("settings.mode_auto")}</option>
          </select>

          <label htmlFor="set-buffer">{t("settings.buffer")}</label>
          <input
            id="set-buffer"
            type="number"
            min={0}
            max={10000}
            value={bufferMs}
            onChange={(e) => setBufferMs(Number(e.target.value))}
          />

          <label htmlFor="set-approval">{t("settings.approval_timeout")}</label>
          <input
            id="set-approval"
            type="number"
            min={5}
            max={3600}
            value={approvalTimeout}
            onChange={(e) => setApprovalTimeout(Number(e.target.value))}
          />

          <label htmlFor="set-cmd-timeout">{t("settings.command_timeout")}</label>
          <input
            id="set-cmd-timeout"
            type="number"
            min={1}
            max={3600}
            value={commandTimeout}
            onChange={(e) => setCommandTimeout(Number(e.target.value))}
          />
        </div>

        <p className="muted settings-note">{t("settings.note_new_sessions")}</p>
        {error && <div role="alert">{error}</div>}
        <div className="merge-actions">
          <button disabled={busy} data-testid="settings-save" onClick={() => void save()}>
            {t("settings.save")}
          </button>
          <button disabled={busy} onClick={onClose}>
            {t("settings.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
