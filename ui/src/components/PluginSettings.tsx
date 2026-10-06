// MCP 插件管理分区（设计方案 §13.5 / §7.5，v1.145 重构）：已装列表（settings
// mcp.servers：启停 / 删除 / 来源徽标）+ 手动添加 + 市场子视图。启停与删除经
// PUT /settings mcp.servers 整体替换（新会话生效）；命令启动器白名单由 daemon
// 校验。旧官方 registry 检索安装界面随通道退役（§13.5）。
import { useState } from "react";
import type { McpServerData, SettingsData, TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { MarketBrowser, type InstalledMarketEntry } from "./MarketBrowser";

/** 市场同款启动器白名单（安装期校验在 daemon；此处仅供手动添加提示）。 */
const LAUNCHERS = ["npx", "uvx", "bunx", "node", "python", "python3", "docker"];

interface Props {
  api: TenonApi;
  t: Translate;
  /** 当前 mcp.servers（settings 合并视图；SettingsDialog 持有权威状态）。 */
  servers: Record<string, McpServerData>;
  /** 保存成功后回写合并视图（App settings 状态同步）。 */
  onSaved: (s: SettingsData) => void;
}

export function PluginSettings({ api, t, servers, onSaved }: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [newCommand, setNewCommand] = useState("npx");
  const [newArgs, setNewArgs] = useState("");
  const [newNet, setNewNet] = useState(false);

  async function putServers(next: Record<string, McpServerData>) {
    setBusy(true);
    setError(null);
    try {
      const merged = await api.putSettings({ mcp: { servers: next } });
      onSaved(merged);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function toggle(name: string) {
    if (busy) return;
    const cur = servers[name];
    if (!cur) return;
    await putServers({
      ...servers,
      [name]: { ...cur, enabled: !(cur.enabled ?? true) },
    });
  }

  async function remove(name: string) {
    if (busy) return;
    if (!window.confirm(`${t("settings.plugins.remove_confirm")}\n${name}`)) return;
    const next = { ...servers };
    delete next[name];
    await putServers(next);
  }

  async function addManual() {
    if (busy) return;
    const name = newName.trim();
    const command = newCommand.trim();
    if (!name || !command) return;
    if (servers[name]) {
      setError(t("settings.plugins.name_taken"));
      return;
    }
    const args = newArgs
      .trim()
      .split(/\s+/)
      .filter(Boolean);
    const entry: McpServerData = {
      command,
      args,
      enabled: true,
      permissions: newNet ? ["net:*"] : [],
      source: null,
    };
    setNewName("");
    setNewArgs("");
    setNewNet(false);
    await putServers({ ...servers, [name]: entry });
  }

  const installedForMarket: Record<string, InstalledMarketEntry> = Object.fromEntries(
    Object.entries(servers).map(([name, s]) => [
      name,
      { source: s.source ?? null, version: s.version ?? null },
    ])
  );

  return (
    <div className="settings-section" data-testid="settings-plugins-section">
      <div className="settings-section-title">{t("settings.plugins")}</div>

      {error && (
        <div role="alert" className="tree-error">
          {error}
        </div>
      )}

      <div className="provider-list" data-testid="mcp-servers-list">
        <div className="settings-section-sub">{t("settings.plugins.mcp_list")}</div>
        {Object.keys(servers).length === 0 ? (
          <span className="muted">{t("settings.plugins.empty")}</span>
        ) : (
          Object.entries(servers).map(([name, s]) => (
            <div className="provider-row" key={name} data-testid={`mcp-server-${name}`}>
              <code className="provider-name">{name}</code>
              <code className="muted mcp-command" title={[s.command, ...(s.args ?? [])].join(" ")}>
                {[s.command, ...(s.args ?? [])].join(" ")}
              </code>
              {s.source && <span className="muted">{s.source}</span>}
              <label className="skills-toggle">
                <input
                  type="checkbox"
                  data-testid={`mcp-toggle-${name}`}
                  aria-label={`${name} · ${t("settings.plugins.toggle")}`}
                  checked={s.enabled ?? true}
                  disabled={busy}
                  onChange={() => void toggle(name)}
                />
                <span>{(s.enabled ?? true) ? t("settings.plugins.on") : t("settings.plugins.off")}</span>
              </label>
              <button
                type="button"
                className="provider-remove"
                aria-label={`${t("settings.plugins.remove")} · ${name}`}
                data-testid={`mcp-remove-${name}`}
                disabled={busy}
                onClick={() => void remove(name)}
              >
                ✕
              </button>
            </div>
          ))
        )}
      </div>

      <div className="settings-section-sub">{t("settings.plugins.manual_add")}</div>
      <div className="provider-add">
        <input
          data-testid="mcp-new-name"
          aria-label={t("settings.plugins.name_label")}
          placeholder={t("settings.plugins.name_hint")}
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
        />
        <input
          data-testid="mcp-new-command"
          aria-label={t("settings.plugins.command_label")}
          placeholder={LAUNCHERS.join(" / ")}
          value={newCommand}
          list="mcp-launchers"
          onChange={(e) => setNewCommand(e.target.value)}
        />
        <datalist id="mcp-launchers">
          {LAUNCHERS.map((l) => (
            <option key={l} value={l} />
          ))}
        </datalist>
        <input
          data-testid="mcp-new-args"
          aria-label={t("settings.plugins.args_label")}
          placeholder={t("settings.plugins.args_hint")}
          value={newArgs}
          onChange={(e) => setNewArgs(e.target.value)}
        />
        <label className="skills-toggle">
          <input
            type="checkbox"
            data-testid="mcp-new-net"
            checked={newNet}
            onChange={(e) => setNewNet(e.target.checked)}
          />
          <span>{t("settings.plugins.net")}</span>
        </label>
        <button
          type="button"
          data-testid="mcp-add"
          disabled={busy || !newName.trim() || !newCommand.trim()}
          onClick={() => void addManual()}
        >
          {t("settings.plugins.add")}
        </button>
      </div>
      <p className="muted settings-note">{t("settings.plugins.note")}</p>

      <div className="settings-section-sub">{t("settings.market.title")}</div>
      <MarketBrowser
        api={api}
        t={t}
        kind="mcp"
        installed={installedForMarket}
        onChanged={() => {
          // 市场安装/卸载经 daemon 写 settings——重拉合并视图校准已装态。
          void api
            .getSettings()
            .then(onSaved)
            .catch(() => {});
        }}
      />
    </div>
  );
}
