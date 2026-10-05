// 插件管理分区（设计方案 §7.1 / §13）：已装清单、registry 检索、校验后直执安装。
// 权限 diff 返回供安装结果审计展示；凭据 / registry 条目不回显明文。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface InstalledPlugin {
  id: string;
  version: string;
  permissions: string[];
  installed_at: string;
}

interface RegistryHit {
  id: string;
  version: string;
  sha256: string;
  signature: string;
  url: string;
  description?: string;
}

interface Props {
  api: TenonApi;
  t: Translate;
}

export function PluginSettings({ api, t }: Props) {
  const [installed, setInstalled] = useState<InstalledPlugin[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<RegistryHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  async function refresh() {
    setLoading(true);
    try {
      const result = await api.listPlugins();
      setInstalled(result.installed ?? []);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void refresh();
  }, [api]);

  async function search() {
    if (!query.trim() || searching) return;
    setSearching(true);
    setError(null);
    setMessage(null);
    try {
      const result = await api.searchPlugins(query.trim());
      setHits(result.hits ?? []);
    } catch (e) {
      setError(String(e));
    } finally {
      setSearching(false);
    }
  }

  async function beginInstall(hit: RegistryHit) {
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const installedPermissions = installed
        .filter((plugin) => plugin.id === hit.id)
        .flatMap((plugin) => plugin.permissions);
      const result = await api.installPlugin(hit, installedPermissions);
      if (result.installed) {
        setMessage(`${hit.id} v${hit.version}`);
        await refresh();
      } else {
        setError(t("settings.plugins.unexpected"));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="settings-section" data-testid="settings-plugins-section">
      <div className="settings-section-title">{t("settings.plugins")}</div>

      {error && (
        <div role="alert" className="tree-error">
          {error}
        </div>
      )}
      {message && (
        <div role="status" className="muted">
          {t("settings.plugins.installed")}: {message}
        </div>
      )}

      <div className="settings-grid">
        <label htmlFor="plugin-search">{t("settings.plugins.search")}</label>
        <div className="provider-add">
          <input
            id="plugin-search"
            data-testid="plugin-search"
            value={query}
            placeholder={t("settings.plugins.search_hint")}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void search();
            }}
          />
          <button
            type="button"
            data-testid="plugin-search-button"
            disabled={searching || !query.trim()}
            onClick={() => void search()}
          >
            {searching ? t("settings.plugins.searching") : t("settings.plugins.search_action")}
          </button>
        </div>
      </div>

      {hits.length > 0 && (
        <div className="provider-list" data-testid="plugin-hits">
          {hits.map((hit) => (
            <div className="provider-row" key={`${hit.id}@${hit.version}`}>
              <code className="provider-name">{hit.id}</code>
              <span>v{hit.version}</span>
              <span className="muted">{hit.description}</span>
              <button
                type="button"
                data-testid={`plugin-install-${hit.id}`}
                disabled={busy}
                onClick={() => void beginInstall(hit)}
              >
                {t("settings.plugins.install")}
              </button>
            </div>
          ))}
        </div>
      )}

      <div className="provider-list" data-testid="installed-plugins">
        <div className="settings-section-sub">{t("settings.plugins.installed_list")}</div>
        {loading ? (
          <span className="muted">{t("settings.plugins.loading")}</span>
        ) : installed.length === 0 ? (
          <span className="muted">{t("settings.plugins.empty")}</span>
        ) : (
          installed.map((plugin) => (
            <div className="provider-row" key={plugin.id}>
              <code className="provider-name">{plugin.id}</code>
              <span>v{plugin.version}</span>
              <span className="muted">{plugin.permissions.join(", ") || t("settings.plugins.no_permissions")}</span>
            </div>
          ))
        )}
      </div>

      <p className="muted settings-note">{t("settings.plugins.note")}</p>
    </div>
  );
}
