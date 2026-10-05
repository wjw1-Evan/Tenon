// 设置面板（§7.2 / §15）：外观 / 语言 / 编辑器（保存方式，v1.75）/ 会话默认档 /
// 代理参数 / 模型分区（v1.40）。外观、语言与保存方式即时生效（保存方式存
// daemon ui_prefs，§7.5）；其余经 PUT /settings 持久化——会话与代理参数对新
// 会话生效，模型保存即重建 provider 表（默认模型对新会话生效，既有会话保持
// 各自 provider）。密钥不变式（§11）：面板只填环境变量引用名，永不输入 / 回显明文。
import { useEffect, useState } from "react";
import type { ProviderSettings, SettingsData, TenonApi, UpdateStatusData } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { LOCALE_CHANGE, type Locale } from "../lib/i18n";
import {
  applyTheme,
  loadThemePreference,
  saveThemePreference,
  type ThemePreference,
} from "../lib/theme";
import { PluginSettings } from "./PluginSettings";
import { SkillsSettings, type SkillsProjectOption } from "./SkillsSettings";

export type { SettingsData } from "../lib/api";

interface LayaStatus {
  enabled?: boolean;
  downloaded?: boolean;
  version?: string | null;
}

interface ProviderRow {
  name: string;
  kind: string;
  base_url: string;
  model: string;
  api_key_env: string;
  /** 在设置覆盖表中：可删除（纯配置文件条目只能编辑，删除即恢复配置文件值）。 */
  overridden: boolean;
}

/** 常用提供商预设（§11 / v1.40）：OpenAI 兼容端点覆盖 DeepSeek / Ollama / 智谱 GLM。 */
const PRESETS: Record<string, { kind: string; base_url: string }> = {
  openai: { kind: "openai", base_url: "https://api.openai.com/v1" },
  anthropic: { kind: "anthropic", base_url: "https://api.anthropic.com" },
  deepseek: { kind: "openai", base_url: "https://api.deepseek.com" },
  ollama: { kind: "openai", base_url: "http://localhost:11434/v1" },
  zhipu: { kind: "openai", base_url: "https://open.bigmodel.cn/api/paas/v4" },
  custom: { kind: "openai", base_url: "" },
};

function rowsFromSettings(settings: SettingsData | null): ProviderRow[] {
  const providers = settings?.models?.providers ?? {};
  return Object.entries(providers).map(([name, p]) => ({
    name,
    kind: String(p.kind ?? "openai"),
    base_url: String(p.base_url ?? ""),
    model: String(p.model ?? ""),
    api_key_env: String(p.api_key_env ?? ""),
    overridden: Boolean(p.overridden),
  }));
}

/** v1.121：Codex 式设置信息架构——左栏分类，右栏只渲染当前分类。 */
type SettingsSection = "general" | "models" | "permissions" | "plugins" | "skills" | "updates";

const SETTINGS_SECTIONS: SettingsSection[] = [
  "general",
  "models",
  "permissions",
  "plugins",
  "skills",
  "updates",
];

interface Props {
  api: TenonApi;
  t: Translate;
  settings: SettingsData | null;
  /** 保存方式（§8.2 v1.75）：App 持有权威状态（ui_prefs 回填 + 即时切换）。 */
  saveMode: "auto" | "manual";
  onSaveModeChange: (mode: "auto" | "manual") => void;
  /** 已打开项目（v1.130 技能分区项目作用域选择器；缺省 = 仅全局作用域可用）。 */
  projects?: SkillsProjectOption[];
  onClose: () => void;
  onSaved: (s: SettingsData) => void;
}

export function SettingsDialog({ api, t, settings, saveMode, onSaveModeChange, projects = [], onClose, onSaved }: Props) {
  const [bufferMs, setBufferMs] = useState(2000);
  const [commandTimeout, setCommandTimeout] = useState(120);
  const [deniedTools, setDeniedTools] = useState("");
  const [maxCost, setMaxCost] = useState("");
  const [updateChannel, setUpdateChannel] = useState<"manual" | "auto">("manual");
  const [updateStatus, setUpdateStatus] = useState<UpdateStatusData | null>(null);
  const [updateBusy, setUpdateBusy] = useState(false);
  const [defaultModel, setDefaultModel] = useState("");
  const [providers, setProviders] = useState<ProviderRow[]>([]);
  const [preset, setPreset] = useState("custom");
  const [laya, setLaya] = useState<LayaStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [section, setSection] = useState<SettingsSection>("general");
  /** 技能停用名单（§13.4 v1.130）：Skills 分区即时 PUT，App 侧经 onSaved 回写。 */
  const [skillsDisabled, setSkillsDisabled] = useState<string[]>([]);

  // 从已拉取的全局设置回填（外观 / 语言为本地即时项，不入 daemon）
  useEffect(() => {
    if (!settings) return;
    setBufferMs(Number(settings.session?.first_edit_buffer_ms ?? 2000));
    setCommandTimeout(Number(settings.exec?.command_timeout_s ?? 120));
    const policy = settings.team_policy ?? {};
    setDeniedTools((policy.denied_tools ?? []).join(", "));
    setMaxCost(policy.max_cost_usd == null ? "" : String(policy.max_cost_usd));
    setUpdateChannel(settings.update?.channel === "auto" ? "auto" : "manual");
    setDefaultModel(String(settings.models?.default ?? ""));
    setProviders(rowsFromSettings(settings));
    setSkillsDisabled(settings.skills?.disabled ?? []);
  }, [settings]);

  // Laya 状态（§15 GET /models）只读展示
  useEffect(() => {
    api
      .models()
      .then((r) => setLaya(r.laya ?? null))
      .catch(() => setLaya(null));
  }, [api]);

  useEffect(() => {
    let alive = true;
    api
      .getUpdates()
      .then((status) => {
        if (alive) setUpdateStatus(status);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [api]);

  if (!settings) return null;

  function addProvider() {
    const base = PRESETS[preset] ?? PRESETS.custom;
    const names = new Set(providers.map((p) => p.name));
    let name = preset;
    let i = 2;
    while (names.has(name)) name = `${preset}-${i++}`;
    setProviders([
      ...providers,
      { name, kind: base.kind, base_url: base.base_url, model: "", api_key_env: "", overridden: true },
    ]);
  }

  function updateProvider(name: string, patch: Partial<ProviderRow>) {
    setProviders((rows) => rows.map((p) => (p.name === name ? { ...p, ...patch } : p)));
  }

  function removeProvider(name: string) {
    setProviders((rows) => rows.filter((p) => p.name !== name));
  }

  async function checkForUpdate() {
    setUpdateBusy(true);
    try {
      setUpdateStatus(await api.checkUpdates());
    } catch (e) {
      setUpdateStatus((status) => ({
        current_version: status?.current_version ?? "",
        channel: status?.channel ?? updateChannel,
        last_error: String(e),
      }));
    } finally {
      setUpdateBusy(false);
    }
  }

  async function applyStagedUpdate() {
    setUpdateBusy(true);
    try {
      const current = await api.getUpdates();
      setUpdateStatus(current);
      if (!current.staged) throw new Error(t("settings.updates.no_staged"));
      await api.applyUpdates();
      setUpdateStatus(await api.getUpdates());
    } catch (e) {
      setUpdateStatus((status) => ({
        current_version: status?.current_version ?? "",
        channel: status?.channel ?? updateChannel,
        last_error: String(e),
      }));
    } finally {
      setUpdateBusy(false);
    }
  }

  async function save() {
    setBusy(true);
    setError(null);
    try {
      let policyMaxCost: number | null = null;
      if (maxCost.trim()) {
        policyMaxCost = Number(maxCost);
        if (!Number.isFinite(policyMaxCost) || policyMaxCost < 0) {
          throw new Error(t("settings.policy.cost_invalid"));
        }
      }
      await api.putTeamPolicy({
        denied_tools: deniedTools
          .split(",")
          .map((name) => name.trim())
          .filter(Boolean),
        max_cost_usd: policyMaxCost,
      });
      const providerPayload: Record<string, ProviderSettings> = {};
      for (const p of providers) {
        const entry: ProviderSettings = { kind: p.kind || "openai", base_url: p.base_url.trim() };
        if (p.model.trim()) entry.model = p.model.trim();
        if (p.api_key_env.trim()) entry.api_key_env = p.api_key_env.trim();
        providerPayload[p.name] = entry;
      }
      const next = await api.putSettings({
        session: {
          first_edit_buffer_ms: bufferMs,
        },
        exec: { command_timeout_s: commandTimeout },
        update: { channel: updateChannel },
        models: { default: defaultModel, providers: providerPayload },
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
        <div className="merge-head settings-head">
          <strong>{t("settings.title")}</strong>
        </div>

        <div className="settings-body">
          <nav className="settings-nav" role="tablist" aria-label={t("settings.title")} data-testid="settings-nav">
            {SETTINGS_SECTIONS.map((key) => (
              <button
                key={key}
                type="button"
                id={`settings-tab-${key}`}
                role="tab"
                aria-controls={`settings-panel-${key}`}
                aria-selected={section === key}
                className={section === key ? "settings-nav-item active" : "settings-nav-item"}
                data-testid={`settings-nav-${key}`}
                onClick={() => setSection(key)}
              >
                {t(`settings.categories.${key}`)}
              </button>
            ))}
          </nav>

          <div
            className="settings-panel"
            role="tabpanel"
            id={`settings-panel-${section}`}
            aria-labelledby={`settings-tab-${section}`}
            aria-label={t(`settings.categories.${section}`)}
            data-testid={`settings-panel-${section}`}
          >
            {section === "general" && (
              <div className="settings-section" data-testid="settings-general-section">
                <div className="settings-section-title">{t("settings.general.title")}</div>
                <div className="settings-grid">
                  <label htmlFor="set-save-mode">{t("settings.saveMode")}</label>
                  <select
                    id="set-save-mode"
                    data-testid="settings-save-mode"
                    value={saveMode}
                    onChange={(e) => onSaveModeChange(e.target.value as "auto" | "manual")}
                  >
                    <option value="auto">{t("settings.saveMode_auto")}</option>
                    <option value="manual">{t("settings.saveMode_manual")}</option>
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
              </div>
            )}

            {section === "models" && (
              <div className="settings-section" data-testid="settings-models-section">
                <div className="settings-section-title">{t("settings.models")}</div>
                <div className="settings-grid">
                  <label htmlFor="set-default-model">{t("settings.models.default")}</label>
                  <select
                    id="set-default-model"
                    value={defaultModel}
                    onChange={(e) => setDefaultModel(e.target.value)}
                    data-testid="settings-default-model"
                  >
                    <option value="">{t("settings.models.default_none")}</option>
                    {providers.map((p) => (
                      <option key={p.name} value={p.name}>
                        {p.name}
                        {p.model ? ` — ${p.model}` : ""}
                      </option>
                    ))}
                  </select>
                </div>

                <div className="provider-list" data-testid="provider-list">
                  {providers.map((p) => (
                    <div className="provider-row" key={p.name} data-testid={`provider-row-${p.name}`}>
                      <code className="provider-name" title={p.name}>{p.name}</code>
                      <select
                        aria-label={`${t("settings.models.kind")} · ${p.name}`}
                        value={p.kind}
                        onChange={(e) => updateProvider(p.name, { kind: e.target.value })}
                      >
                        <option value="openai">{t("settings.models.kind_openai")}</option>
                        <option value="anthropic">{t("settings.models.kind_anthropic")}</option>
                        <option value="openai_responses">
                          {t("settings.models.kind_openai_responses")}
                        </option>
                      </select>
                      <input
                        aria-label={`${t("settings.models.base_url")} · ${p.name}`}
                        placeholder="https://…"
                        value={p.base_url}
                        onChange={(e) => updateProvider(p.name, { base_url: e.target.value })}
                      />
                      <input
                        aria-label={`${t("settings.models.model")} · ${p.name}`}
                        placeholder={t("settings.models.model_hint")}
                        value={p.model}
                        onChange={(e) => updateProvider(p.name, { model: e.target.value })}
                      />
                      <input
                        aria-label={`${t("settings.models.api_key_env")} · ${p.name}`}
                        placeholder="OPENAI_API_KEY"
                        value={p.api_key_env}
                        onChange={(e) => updateProvider(p.name, { api_key_env: e.target.value })}
                      />
                      {p.overridden && (
                        <button
                          type="button"
                          className="provider-remove"
                          aria-label={`${t("settings.models.delete")} · ${p.name}`}
                          data-testid={`provider-remove-${p.name}`}
                          onClick={() => removeProvider(p.name)}
                        >
                          ✕
                        </button>
                      )}
                    </div>
                  ))}
                </div>

                <div className="provider-add">
                  <select
                    aria-label={t("settings.models.preset")}
                    value={preset}
                    onChange={(e) => setPreset(e.target.value)}
                    data-testid="provider-preset"
                  >
                    <option value="openai">{t("settings.models.preset_openai")}</option>
                    <option value="anthropic">{t("settings.models.preset_anthropic")}</option>
                    <option value="deepseek">{t("settings.models.preset_deepseek")}</option>
                    <option value="ollama">{t("settings.models.preset_ollama")}</option>
                    <option value="zhipu">{t("settings.models.preset_zhipu")}</option>
                    <option value="custom">{t("settings.models.preset_custom")}</option>
                  </select>
                  <button type="button" data-testid="provider-add" onClick={addProvider}>
                    {t("settings.models.add")}
                  </button>
                </div>

                <div className="laya-status" data-testid="laya-status">
                  <span className="settings-section-sub">{t("settings.models.laya")}</span>
                  {laya ? (
                    <span className="muted">
                      {laya.enabled ? t("settings.models.laya_enabled") : t("settings.models.laya_disabled")}
                      {" · "}
                      {laya.downloaded
                        ? t("settings.models.laya_downloaded")
                        : t("settings.models.laya_not_downloaded")}
                      {laya.version ? ` · ${t("settings.models.laya_version")} ${laya.version}` : ""}
                    </span>
                  ) : (
                    <span className="muted">{t("settings.models.laya_unknown")}</span>
                  )}
                </div>

                <p className="muted settings-note">{t("settings.models.note")}</p>
              </div>
            )}

            {section === "permissions" && (
              <div className="settings-section" data-testid="settings-policy-section">
                <div className="settings-section-title">{t("settings.policy")}</div>
                <div className="settings-grid">
                  <label htmlFor="set-denied-tools">{t("settings.policy.denied_tools")}</label>
                  <input
                    id="set-denied-tools"
                    data-testid="settings-denied-tools"
                    placeholder="git_push, create_pr"
                    value={deniedTools}
                    onChange={(e) => setDeniedTools(e.target.value)}
                  />

                  <label htmlFor="set-policy-cost">{t("settings.policy.cost")}</label>
                  <input
                    id="set-policy-cost"
                    data-testid="settings-policy-cost"
                    type="number"
                    min={0}
                    max={1000000}
                    step={0.01}
                    value={maxCost}
                    onChange={(e) => setMaxCost(e.target.value)}
                  />
                </div>
                <p className="muted settings-note">{t("settings.policy.note")}</p>
              </div>
            )}

            {section === "plugins" && <PluginSettings api={api} t={t} />}

            {section === "skills" && (
              <SkillsSettings
                api={api}
                t={t}
                projects={projects}
                disabled={skillsDisabled}
                onDisabledChange={setSkillsDisabled}
                onSaved={onSaved}
              />
            )}

            {section === "updates" && (
              <div className="settings-section" data-testid="settings-updates-section">
                <div className="settings-section-title">{t("settings.update.channel")}</div>
                <div className="settings-grid">
                  <label htmlFor="set-update">{t("settings.update.channel")}</label>
                  <select
                    id="set-update"
                    data-testid="settings-update-channel"
                    value={updateChannel}
                    onChange={(e) => setUpdateChannel(e.target.value as "manual" | "auto")}
                  >
                    <option value="manual">{t("settings.update.manual")}</option>
                    <option value="auto">{t("settings.update.auto")}</option>
                  </select>
                </div>

                <div className="settings-section" data-testid="settings-updates-status">
                  <div className="settings-section-title">{t("settings.updates.status")}</div>
                  <div className="settings-grid">
                    <span>{t("settings.updates.current")}</span>
                    <code>{updateStatus?.current_version || "—"}</code>

                    <span>{t("settings.updates.last_check")}</span>
                    <span className="muted">
                      {updateStatus?.last_check_at || t("settings.updates.never")}
                      {updateStatus?.last_error ? ` · ${updateStatus.last_error}` : ""}
                    </span>

                    <span>{t("settings.updates.staged")}</span>
                    <span className="muted">
                      {updateStatus?.staged
                        ? `v${updateStatus.staged.version} · ${t("settings.updates.restart_required")}`
                        : t("settings.updates.none")}
                    </span>
                  </div>
                  <div className="provider-add">
                    <button
                      type="button"
                      data-testid="settings-update-check"
                      disabled={updateBusy}
                      onClick={() => void checkForUpdate()}
                    >
                      {t("settings.updates.check")}
                    </button>
                    <button
                      type="button"
                      data-testid="settings-update-apply"
                      disabled={updateBusy || !updateStatus?.staged}
                      onClick={() => void applyStagedUpdate()}
                    >
                      {t("settings.updates.apply")}
                    </button>
                  </div>
                  <p className="muted settings-note">{t("settings.updates.note")}</p>
                </div>
              </div>
            )}
          </div>
        </div>

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
