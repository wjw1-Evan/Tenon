// 技能管理分区（设计方案 §13.4 / v1.130）：合并清单（作用域徽标 + 启停）+
// SKILL.md 源码编辑。全局技能 CRUD 走 /skills；项目技能读写走项目文件 API
//（写守卫 + 脏缓冲协调），删除走文件操作；启停即时 PUT /settings（新会话生效）。
import { useEffect, useState } from "react";
import type { SettingsData, TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

export interface SkillsProjectOption {
  id: string;
  label: string;
}

interface SkillRow {
  name: string;
  display_name: string;
  description: string;
  scope: "global" | "project";
  dir: string;
  enabled: boolean;
}

/** 新建技能的 frontmatter 模板（§13.4：目录名即 id，正文由模型按需加载）。 */
const SKILL_TEMPLATE =
  "---\nname: \ndescription: \n---\n\n<!-- 技能正文：模型经 skill_use 按需加载本文件全文。 -->\n";

const SKILL_NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

function projectSkillPath(name: string) {
  return `.tenon/skills/${name}/SKILL.md`;
}

interface Props {
  api: TenonApi;
  t: Translate;
  projects: SkillsProjectOption[];
  /** 停用名单（settings.skills.disabled，SettingsDialog 持有权威状态）。 */
  disabled: string[];
  onDisabledChange: (next: string[]) => void;
  /** 启停 PUT /settings 成功后回写合并视图（App settings 状态同步）。 */
  onSaved: (s: SettingsData) => void;
}

export function SkillsSettings({ api, t, projects, disabled, onDisabledChange, onSaved }: Props) {
  const [scope, setScope] = useState("");
  const [skills, setSkills] = useState<SkillRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [dirty, setDirty] = useState(false);
  const [newName, setNewName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  async function refresh(target = scope) {
    setLoading(true);
    try {
      const result = await api.listSkills(target || undefined);
      setSkills(result.skills ?? []);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void refresh();
    setSelected(null);
    setContent("");
    setDirty(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, scope]);

  async function toggle(row: SkillRow) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const next = row.enabled
        ? [...disabled, row.name]
        : disabled.filter((n) => n !== row.name);
      const merged = await api.putSettings({ skills: { disabled: next } });
      onDisabledChange(next);
      onSaved(merged);
      setSkills((rows) =>
        rows.map((r) => (r.name === row.name ? { ...r, enabled: !row.enabled } : r))
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function select(row: SkillRow) {
    if (dirty) {
      const discard = window.confirm(t("settings.skills.dirty_confirm"));
      if (!discard) return;
    }
    setError(null);
    setMessage(null);
    try {
      const result = await api.getSkill(row.name, scope || undefined);
      setSelected(row.name);
      setContent(result.content);
      setDirty(false);
    } catch (e) {
      setError(String(e));
    }
  }

  async function saveSelected() {
    if (!selected || busy) return;
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      if (scope) {
        await api.writeFile(scope, projectSkillPath(selected), content);
      } else {
        await api.updateSkill(selected, content);
      }
      setDirty(false);
      setMessage(t("settings.skills.saved"));
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function createSkill() {
    const name = newName.trim();
    if (!name || busy) return;
    if (!SKILL_NAME_PATTERN.test(name)) {
      setError(t("settings.skills.invalid_name"));
      return;
    }
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      await api.createSkill(name, SKILL_TEMPLATE);
      setNewName("");
      await refresh();
      const result = await api.getSkill(name);
      setSelected(name);
      setContent(result.content);
      setDirty(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function removeSkill(row: SkillRow) {
    if (busy) return;
    if (!window.confirm(`${t("settings.skills.delete_confirm")}\n${row.name}`)) return;
    setBusy(true);
    setError(null);
    setMessage(null);
    try {
      if (row.scope === "global") {
        await api.deleteSkill(row.name);
      } else {
        await api.fileOps(scope, [
          { op: "delete", path: projectSkillPath(row.name) },
        ]);
      }
      if (selected === row.name) {
        setSelected(null);
        setContent("");
        setDirty(false);
      }
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="settings-section" data-testid="settings-skills-section">
      <div className="settings-section-title">{t("settings.skills.title")}</div>

      <div className="settings-grid">
        <label htmlFor="skills-scope">{t("settings.skills.scope")}</label>
        <select
          id="skills-scope"
          data-testid="skills-scope"
          value={scope}
          onChange={(e) => setScope(e.target.value)}
        >
          <option value="">{t("settings.skills.scope_global")}</option>
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {t("settings.skills.scope_project")} · {p.label}
            </option>
          ))}
        </select>
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

      <div className="provider-list" data-testid="skills-list">
        {loading ? (
          <span className="muted">{t("settings.skills.loading")}</span>
        ) : skills.length === 0 ? (
          <span className="muted">{t("settings.skills.empty")}</span>
        ) : (
          skills.map((row) => (
            <div
              className={`provider-row${selected === row.name ? " active" : ""}`}
              key={`${row.scope}:${row.name}`}
            >
              <button
                type="button"
                className="provider-name skills-open"
                data-testid={`skills-open-${row.name}`}
                onClick={() => void select(row)}
              >
                {row.display_name}
              </button>
              <span className="muted">{t(`settings.skills.scope_${row.scope}`)}</span>
              <span className="muted skills-desc" title={row.description}>
                {row.description}
              </span>
              <label className="skills-toggle">
                <input
                  type="checkbox"
                  data-testid={`skills-toggle-${row.name}`}
                  aria-label={`${row.name} · ${t("settings.skills.toggle")}`}
                  checked={row.enabled}
                  disabled={busy}
                  onChange={() => void toggle(row)}
                />
                <span>{row.enabled ? t("settings.skills.enabled") : t("settings.skills.off")}</span>
              </label>
              <button
                type="button"
                className="provider-remove"
                aria-label={`${t("settings.skills.delete")} · ${row.name}`}
                data-testid={`skills-delete-${row.name}`}
                disabled={busy}
                onClick={() => void removeSkill(row)}
              >
                ✕
              </button>
            </div>
          ))
        )}
      </div>

      {!scope && (
        <div className="provider-add">
          <input
            data-testid="skills-new-name"
            aria-label={t("settings.skills.name_label")}
            placeholder={t("settings.skills.name_hint")}
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void createSkill();
            }}
          />
          <button
            type="button"
            data-testid="skills-new"
            disabled={busy || !newName.trim()}
            onClick={() => void createSkill()}
          >
            {t("settings.skills.new")}
          </button>
        </div>
      )}

      {selected && (
        <div className="skills-editor" data-testid="skills-editor">
          <div className="settings-section-sub">
            {selected}
            {dirty ? ` · ${t("settings.skills.dirty")}` : ""}
          </div>
          <textarea
            data-testid="skills-content"
            aria-label={`${t("settings.skills.title")} · ${selected}`}
            spellCheck={false}
            value={content}
            onChange={(e) => {
              setContent(e.target.value);
              setDirty(true);
            }}
          />
          <div className="provider-add">
            <button
              type="button"
              data-testid="skills-save"
              disabled={busy || !dirty}
              onClick={() => void saveSelected()}
            >
              {t("settings.skills.save")}
            </button>
          </div>
        </div>
      )}

      <p className="muted settings-note">{t("settings.skills.note")}</p>
    </div>
  );
}
