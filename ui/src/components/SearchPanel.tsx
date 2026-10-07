// 全局搜索 / 替换（§8.1）：rg hits、替换前 diff、选定文件应用、行号跳转。
import { useCallback, useEffect, useMemo, useState } from "react";
import type { SearchHit, TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface Props {
  api: TenonApi;
  t: Translate;
  projectId: string | null;
  onOpenFile: (path: string, line?: number) => void;
  onChanged?: () => void;
}

interface ResultGroup {
  path: string;
  hits: SearchHit[];
}

function groupHits(hits: SearchHit[]): ResultGroup[] {
  const groups = new Map<string, ResultGroup>();
  for (const hit of hits) {
    const group = groups.get(hit.path) ?? { path: hit.path, hits: [] };
    group.hits.push(hit);
    groups.set(hit.path, group);
  }
  return [...groups.values()];
}

export function SearchPanel({ api, t, projectId, onOpenFile, onChanged }: Props) {
  const [query, setQuery] = useState("");
  const [plainText, setPlainText] = useState(false);
  const [replacement, setReplacement] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [previews, setPreviews] = useState<Record<string, string>>({});
  const [selected, setSelected] = useState<Record<string, true>>({});
  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [searched, setSearched] = useState(false);
  const [regexError, setRegexError] = useState<string | null>(null);
  const groups = useMemo(() => groupHits(hits), [hits]);
  const selectedPaths = useMemo(
    () => Object.keys(selected).filter((path) => selected[path]),
    [selected]
  );

  // 切换项目清空结果：旧项目的 hits/selected 若保留，apply() 会用
  // 新 projectId + 旧 selectedPaths 把替换写进新项目的同名文件
  useEffect(() => {
    setHits([]);
    setPreviews({});
    setSelected({});
    setSearched(false);
    setError(null);
  }, [projectId]);

  /** 纯文本模式：转义正则元字符后按字面搜索（对用户隐藏 rg 正则门槛）。 */
  const effectiveQuery = useMemo(
    () =>
      plainText ? query.trim().replace(/[.*+?^${}()|[\]\\]/g, "\\$&") : query.trim(),
    [plainText, query]
  );

  const runSearch = useCallback(async () => {
    if (!projectId || !effectiveQuery) return;
    setLoading(true);
    setError(null);
    setSearched(true);
    try {
      const response = await api.search(projectId, effectiveQuery);
      setHits(response.hits ?? []);
      setSelected(
        Object.fromEntries((response.hits ?? []).map((hit) => [hit.path, true as const]))
      );
      if (replacement) {
        const preview = await api.searchPreview(projectId, effectiveQuery, replacement);
        setPreviews(Object.fromEntries(preview.previews.map((item) => [item.path, item.diff])));
      } else {
        setPreviews({});
      }
    } catch (e) {
      setError(String(e));
  } finally {
    setLoading(false);
  }
  }, [api, effectiveQuery, projectId, replacement]);

  useEffect(() => {
    setRegexError(null);
    // 纯文本模式查询已转义，无需正则校验。
    if (!replacement || plainText) return;
    try {
      new RegExp(query);
    } catch (e) {
      setRegexError(String(e));
    }
  }, [plainText, query, replacement]);

  const apply = useCallback(async () => {
    if (!projectId || selectedPaths.length === 0) return;
    setApplying(true);
    setError(null);
    try {
      const response = await api.applySearchReplace(
        projectId,
        effectiveQuery,
        replacement,
        selectedPaths
      );
      setHits((prev) =>
        prev.filter(
          (hit) => !response.applied.some((item) => item.path === hit.path)
        )
      );
      for (const item of response.applied) setPreviews((prev) => {
        const next = { ...prev };
        delete next[item.path];
        return next;
      });
      setSelected((prev) => {
        const next = { ...prev };
        for (const item of response.applied) delete next[item.path];
        return next;
      });
      onChanged?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setApplying(false);
    }
  }, [api, effectiveQuery, onChanged, projectId, replacement, selectedPaths]);

  return (
    <section className="search-panel" data-testid="search-panel" aria-label={t("search.title")}>
      <form
        className="search-form"
        onSubmit={(event) => {
          event.preventDefault();
          void runSearch();
        }}
      >
        <input
          aria-label={t("search.query")}
          placeholder={plainText ? t("search.query_plain") : t("search.query")}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        {hits.length > 0 && (
          <input
            aria-label={t("search.replace_with")}
            placeholder={t("search.replace_with")}
            value={replacement}
            onChange={(event) => setReplacement(event.target.value)}
          />
        )}
        <label className="search-plain-toggle">
          <input
            type="checkbox"
            checked={plainText}
            onChange={(event) => setPlainText(event.target.checked)}
            data-testid="search-plain-toggle"
          />
          {t("search.plain_text")}
        </label>
        <button type="submit" disabled={loading || !projectId || !query.trim()}>
          {loading ? t("search.searching") : t("search.title")}
        </button>
      </form>
      {regexError && (
        <div className="tree-error" role="alert">
          {regexError}
        </div>
      )}
      {error && (
        <div className="tree-error" role="alert" data-testid="search-error">
          {error}
        </div>
      )}
      {searched && (
        <div className="search-summary">
          {loading ? (
            t("search.searching")
          ) : (
            <>
              <span>
                {hits.length} {t("search.results")}
              </span>
              {hits.length > 0 && (
                <label className="search-select-all">
                  <input
                    type="checkbox"
                    checked={selectedPaths.length === hits.length}
                    onChange={(event) =>
                      setSelected(
                        event.target.checked
                          ? Object.fromEntries(hits.map((hit) => [hit.path, true as const]))
                          : {}
                      )
                    }
                    data-testid="search-select-all"
                  />
                  {t("search.select_all")}
                </label>
              )}
            </>
          )}
        </div>
      )}
      <ul className="search-groups">
        {groups.map((group) => {
          const preview = previews[group.path];
          const allSelected = group.hits.every((hit) => selected[hit.path]);
          return (
            <li key={group.path} className="search-group">
              <div className="search-group-head">
                <label>
                  <input
                    type="checkbox"
                    checked={Boolean(selected[group.path])}
                    onChange={(event) =>
                      setSelected((prev) => {
                        const next = { ...prev };
                        for (const hit of group.hits) {
                          if (event.target.checked) next[hit.path] = true;
                          else delete next[hit.path];
                        }
                        return next;
                      })
                    }
                  />
                  <button
                    type="button"
                    className="search-path"
                    onClick={() => onOpenFile(group.path, group.hits[0]?.line)}
                  >
                    {group.path}
                  </button>
                  <span className="muted">{group.hits.length}</span>
                </label>
              </div>
              <ul>
                {group.hits.map((hit) => (
                  <li key={`${hit.path}:${hit.line}:${hit.column}`}>
                    <button
                      type="button"
                      className="search-hit"
                      onClick={() => onOpenFile(hit.path, hit.line)}
                      title={hit.text}
                    >
                      <span>
                        {hit.line}:{hit.column}
                      </span>
                      <code>{hit.text}</code>
                    </button>
                  </li>
                ))}
              </ul>
              {preview && (
                <>
                  <label className="search-preview-label">
                    <input
                      type="checkbox"
                      checked={allSelected}
                      onChange={(event) =>
                        setSelected((prev) => {
                          const next = { ...prev };
                          for (const hit of group.hits) {
                            if (event.target.checked) next[hit.path] = true;
                            else delete next[hit.path];
                          }
                          return next;
                        })
                      }
                    />
                    {t("search.apply_file")}
                  </label>
                  <pre className="search-preview" data-testid={`preview-${group.path}`}>
                    {preview}
                  </pre>
                </>
              )}
            </li>
          );
        })}
      </ul>
      {searched && replacement && (
        <button
          type="button"
          className="search-apply"
          disabled={applying || selectedPaths.length === 0 || Boolean(regexError)}
          onClick={() => void apply()}
          data-testid="search-apply"
        >
          {applying
            ? t("search.applying")
            : `${t("search.apply_selected")} (${selectedPaths.length})`}
        </button>
      )}
    </section>
  );
}
