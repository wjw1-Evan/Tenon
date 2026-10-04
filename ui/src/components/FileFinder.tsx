// Cmd/Ctrl+P fuzzy finder（§7.4）：files / workspace symbols / active-file line。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { LspSymbol, TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface FileHit {
  path: string;
  name: string;
  kind: "dir" | "file";
  score: number;
}

type FinderHit =
  | ({ type: "file" } & FileHit)
  | ({ type: "symbol" } & LspSymbol);

interface Props {
  open: boolean;
  onClose: () => void;
  api: TenonApi;
  t: Translate;
  projectId: string | null;
  projectRoot?: string;
  activePath: string | null;
  onOpen: (path: string, line?: number) => void;
}

function parseQuery(raw: string) {
  const at = raw.lastIndexOf(":");
  if (at <= 0) return { query: raw, line: undefined as number | undefined };
  const line = Number.parseInt(raw.slice(at + 1), 10);
  return Number.isFinite(line) && line > 0
    ? { query: raw.slice(0, at), line }
    : { query: raw, line: undefined };
}

function fileUriToRelative(uri: string): string {
  try {
    const url = new URL(uri);
    let path = decodeURIComponent(url.pathname);
    // Windows file:///C:/path → C:/path
    path = path.replace(/^\/([A-Za-z]:)/, "$1");
    return path.replace(/^\.\//, "");
  } catch {
    return uri;
  }
}

function parseSymbols(value: unknown, projectRoot?: string): LspSymbol[] {
  if (!Array.isArray(value)) return [];
  const symbols: LspSymbol[] = [];
  for (const item of value) {
      const record = item as {
        name?: unknown;
        detail?: unknown;
        location?: {
          uri?: unknown;
          range?: { start?: { line?: unknown; character?: unknown } };
        };
      };
      const name = typeof record.name === "string" ? record.name : "";
      const uri =
        typeof record.location?.uri === "string" ? record.location.uri : "";
      const line = record.location?.range?.start?.line;
      const character = record.location?.range?.start?.character;
      if (!name || !uri) continue;
      let path = fileUriToRelative(uri);
      if (projectRoot && path.startsWith(`${projectRoot}/`)) {
        path = path.slice(projectRoot.length + 1);
      }
      symbols.push({
        name,
        detail: typeof record.detail === "string" ? record.detail : undefined,
        path,
        line: typeof line === "number" ? line + 1 : 1,
        column: typeof character === "number" ? character + 1 : 1,
      });
  }
  return symbols;
}

export function FileFinder({
  open,
  onClose,
  api,
  t,
  projectId,
  projectRoot,
  activePath,
  onOpen,
}: Props) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<FinderHit[]>([]);
  const [activeIndex, setActiveIndex] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const symbolMode = query.startsWith("@");
  const parsed = useMemo(
    () => parseQuery(symbolMode ? query.slice(1) : query),
    [query, symbolMode]
  );

  useEffect(() => {
    if (open) {
      setQuery("");
      setHits([]);
      setActiveIndex(0);
      setError(null);
    }
  }, [open]);

  const search = useCallback(async () => {
    if (!open || !projectId) return;
    setLoading(true);
    setError(null);
    try {
      if (symbolMode) {
        if (!activePath) {
          setError(t("finder.symbol_need_file"));
          setHits([]);
          return;
        }
        const response = await api.lsp({
          project_id: projectId,
          path: activePath,
          action: "workspace_symbol",
          extra: parsed.query,
        });
        const symbols = parseSymbols(response.result, projectRoot).slice(0, 100);
        setHits(symbols.map((symbol) => ({ type: "symbol" as const, ...symbol })));
        setActiveIndex(0);
        return;
      }
      const response = await api.fuzzyFiles(projectId, parsed.query, 100);
      setHits(
        (response.hits ?? []).map((hit) => ({ type: "file" as const, ...hit }))
      );
      setActiveIndex(0);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [activePath, api, open, parsed.query, projectId, symbolMode, t]);

  useEffect(() => {
    if (!open || !projectId) return;
    const timer = window.setTimeout(() => void search(), 120);
    return () => window.clearTimeout(timer);
  }, [open, projectId, search]);

  const selected = hits[activeIndex];
  const choose = (hit?: FinderHit) => {
    if (!hit) return;
    onOpen(hit.path, hit.type === "symbol" ? hit.line : parsed.line);
    onClose();
  };

  useEffect(() => {
    const active = listRef.current?.querySelector<HTMLElement>('[data-active="true"]');
    if (active && typeof active.scrollIntoView === "function") {
      active.scrollIntoView({ block: "nearest" });
    }
  }, [activeIndex, hits]);

  if (!open) return null;
  return (
    <div className="palette-overlay" onClick={onClose} data-testid="file-finder">
      <div className="palette" onClick={(event) => event.stopPropagation()}>
        <input
          autoFocus
          value={query}
          aria-label={t("finder.placeholder")}
          placeholder={t("finder.placeholder")}
          data-testid="file-finder-input"
          onChange={(event) => {
            setQuery(event.target.value);
            setActiveIndex(0);
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape") onClose();
            if (event.key === "ArrowDown") {
              event.preventDefault();
              setActiveIndex((index) => Math.min(index + 1, Math.max(hits.length - 1, 0)));
            }
            if (event.key === "ArrowUp") {
              event.preventDefault();
              setActiveIndex((index) => Math.max(index - 1, 0));
            }
            if (event.key === "Enter") {
              event.preventDefault();
              choose(selected);
            }
          }}
        />
        <div className="finder-status">
          <span data-testid="finder-mode">{symbolMode ? t("finder.mode_symbol") : t("finder.mode_file")}</span>
          {loading ? ` · ${t("finder.loading")}` : error ? ` · ${error}` : ` · ${hits.length} ${t("finder.results")}`}
          {!symbolMode && activePath && parsed.line ? ` · ${t("finder.active_line")}` : ""}
        </div>
        <ul ref={listRef}>
          {hits.map((hit, index) => (
            <li key={`${hit.type}:${hit.path}:${hit.name}:${index}`}>
              <button
                type="button"
                data-active={index === activeIndex}
                onClick={() => choose(hit)}
                onMouseEnter={() => setActiveIndex(index)}
              >
                <span className="finder-path">
                  {hit.type === "symbol" ? `${hit.name} — ${hit.path}` : hit.path}
                </span>
                <span className="finder-meta">
                  {hit.type === "symbol" ? `:${hit.line}` : parsed.line ? `:${parsed.line}` : ""}
                </span>
              </button>
            </li>
          ))}
          {!loading && !error && hits.length === 0 && (
            <li>
              <button type="button" disabled>
                {t("finder.empty")}
              </button>
            </li>
          )}
        </ul>
      </div>
    </div>
  );
}
