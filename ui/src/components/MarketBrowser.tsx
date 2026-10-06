// 市场子视图（设计方案 §13.5 / v1.145）：市场源管理（owner/repo）+ 清单条目
// 列表 + 安装/更新/卸载。skills 与 plugins 两个设置分区共用（kind 过滤条目）；
// 已装态由父级传入（skill = /skills 的 market sidecar，mcp = settings mcp.servers）。
// 社区内容按不可信数据对待（§12.5）：MCP 安装前完整展示命令行面（用户点击即同意）。
import { useEffect, useState } from "react";
import type { MarketEntryData, TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

export interface InstalledMarketEntry {
  source?: string | null;
  path?: string | null;
  version?: string | null;
}

interface ManifestView {
  name?: string;
  entries: MarketEntryData[];
}

interface Props {
  api: TenonApi;
  t: Translate;
  kind: "skill" | "mcp";
  /** 已安装表：条目 name → 溯源（决定 已装/更新/安装 态）。 */
  installed: Record<string, InstalledMarketEntry>;
  /** 安装 / 卸载成功后由父级刷新已装表。 */
  onChanged: () => void;
}

const SOURCE_PATTERN = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;

export function MarketBrowser({ api, t, kind, installed, onChanged }: Props) {
  const [sources, setSources] = useState<string[]>([]);
  const [newSource, setNewSource] = useState("");
  const [manifests, setManifests] = useState<Record<string, ManifestView>>({});
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  async function refresh() {
    setLoading(true);
    try {
      const { sources: list } = await api.listMarketSources();
      setSources(list ?? []);
      const results = await Promise.allSettled(
        (list ?? []).map(async (source) => {
          const [owner, repo] = source.split("/");
          return [source, await api.getMarketManifest(owner, repo)] as const;
        })
      );
      const next: Record<string, ManifestView> = {};
      for (const r of results) {
        if (r.status === "fulfilled") {
          next[r.value[0]] = {
            name: r.value[1].name,
            entries: r.value[1].entries ?? [],
          };
        }
      }
      setManifests(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api]);

  async function saveSources(next: string[]) {
    setBusy(true);
    setError(null);
    try {
      const r = await api.putMarketSources(next);
      setSources(r.sources ?? next);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function addSource() {
    const source = newSource.trim();
    if (!source || busy) return;
    if (!SOURCE_PATTERN.test(source)) {
      setError(t("settings.market.invalid_source"));
      return;
    }
    if (sources.includes(source)) {
      setError(t("settings.market.duplicate_source"));
      return;
    }
    setNewSource("");
    await saveSources([...sources, source]);
  }

  function entryState(entry: MarketEntryData): "install" | "update" | "installed" {
    const cur = installed[entry.name];
    if (!cur) return "install";
    if (entry.version && cur.version && entry.version !== cur.version) return "update";
    return "installed";
  }

  async function install(entry: MarketEntryData) {
    if (busy) return;
    // MCP 条目安装前完整展示命令行面（§13.5：用户点击安装即同意该命令）。
    if (kind === "mcp" && entry.command) {
      const face = [entry.command, ...(entry.args ?? [])].join(" ");
      const ok = window.confirm(`${t("settings.market.install_confirm")}\n\n$ ${face}`);
      if (!ok) return;
    }
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const source = marketSourceOf(entry) ?? sources[0] ?? "";
      const r = await api.marketInstall(source, kind, entry.name);
      if (r.installed) {
        setMessage(
          r.updated
            ? `${t("settings.market.updated")} · ${entry.name}`
            : `${t("settings.market.installed_toast")} · ${entry.name}${kind === "mcp" && r.note ? ` · ${r.note}` : ""}`
        );
        onChanged();
      } else {
        setError(r.error ?? t("settings.market.failed"));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function uninstall(name: string) {
    if (busy) return;
    if (!window.confirm(`${t("settings.market.uninstall_confirm")}\n${name}`)) return;
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      const r = await api.marketUninstall(kind, name);
      if (r.uninstalled) {
        setMessage(`${t("settings.market.uninstalled_toast")} · ${name}`);
        onChanged();
      } else {
        setError(r.error ?? t("settings.market.failed"));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  /** 条目所属市场源（其清单包含该条目的 source；§13.5 安装以清单为准）。 */
  function marketSourceOf(entry: MarketEntryData): string | null {
    for (const [source, view] of Object.entries(manifests)) {
      if (view.entries.some((e) => e.kind === kind && e.name === entry.name)) {
        return source;
      }
    }
    return null;
  }

  const kindLabel = kind === "skill" ? t("settings.market.kind_skill") : t("settings.market.kind_mcp");

  return (
    <div className="market-browser" data-testid={`market-browser-${kind}`}>
      <div className="settings-section-sub">{t("settings.market.sources")}</div>
      {sources.length === 0 && !loading && (
        <p className="muted">{t("settings.market.no_sources")}</p>
      )}
      {sources.map((source) => (
        <div className="provider-row" key={source} data-testid={`market-source-${source}`}>
          <code className="provider-name">{source}</code>
          <span className="muted">
            {manifests[source]?.entries.filter((e) => e.kind === kind).length ?? "…"} {kindLabel}
          </span>
          <button
            type="button"
            className="provider-remove"
            aria-label={`${t("settings.market.remove_source")} · ${source}`}
            data-testid={`market-source-remove-${source}`}
            disabled={busy}
            onClick={() => void saveSources(sources.filter((s) => s !== source))}
          >
            ✕
          </button>
        </div>
      ))}
      <div className="provider-add">
        <input
          data-testid="market-source-input"
          aria-label={t("settings.market.add_source")}
          placeholder={t("settings.market.source_hint")}
          value={newSource}
          onChange={(e) => setNewSource(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void addSource();
          }}
        />
        <button
          type="button"
          data-testid="market-source-add"
          disabled={busy || !newSource.trim()}
          onClick={() => void addSource()}
        >
          {t("settings.market.add_source")}
        </button>
      </div>

      {error && (
        <div role="alert" className="tree-error">
          {error}
        </div>
      )}
      {message && (
        <div role="status" className="muted">
          {message}
        </div>
      )}

      {loading ? (
        <span className="muted">{t("settings.market.loading")}</span>
      ) : (
        Object.entries(manifests).map(([source, view]) => {
          const entries = view.entries.filter((e) => e.kind === kind);
          if (entries.length === 0) return null;
          return (
            <div className="provider-list" key={source} data-testid={`market-list-${source}`}>
              <div className="settings-section-sub">
                {view.name ?? source} · {entries.length} {kindLabel}
              </div>
              {entries.map((entry) => {
                const state = entryState(entry);
                return (
                  <div
                    className="provider-row"
                    key={`${source}/${entry.name}`}
                    data-testid={`market-entry-${entry.name}`}
                  >
                    <code className="provider-name">{entry.name}</code>
                    {entry.version && <span>v{entry.version}</span>}
                    <span className="muted market-desc" title={entry.description}>
                      {entry.description}
                    </span>
                    {state === "installed" ? (
                      <>
                        <span className="muted">✓ {t("settings.market.installed")}</span>
                        <button
                          type="button"
                          className="provider-remove"
                          aria-label={`${t("settings.market.uninstall")} · ${entry.name}`}
                          data-testid={`market-uninstall-${entry.name}`}
                          disabled={busy}
                          onClick={() => void uninstall(entry.name)}
                        >
                          ✕
                        </button>
                      </>
                    ) : (
                      <>
                        <button
                          type="button"
                          data-testid={`market-install-${entry.name}`}
                          disabled={busy}
                          onClick={() => void install(entry)}
                        >
                          {state === "update" ? t("settings.market.update") : t("settings.market.install")}
                        </button>
                        {state === "update" && (
                          <button
                            type="button"
                            className="provider-remove"
                            aria-label={`${t("settings.market.uninstall")} · ${entry.name}`}
                            data-testid={`market-uninstall-${entry.name}`}
                            disabled={busy}
                            onClick={() => void uninstall(entry.name)}
                          >
                            ✕
                          </button>
                        )}
                      </>
                    )}
                  </div>
                );
              })}
            </div>
          );
        })
      )}

      <p className="muted settings-note">{t("settings.market.note")}</p>
    </div>
  );
}
