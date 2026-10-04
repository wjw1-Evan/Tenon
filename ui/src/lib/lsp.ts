// LSP 响应归一化（§8.3 / §8.5）：供 Monaco 语言注册器复用与纯函数测试。
export interface MonacoRange {
  startLineNumber: number;
  startColumn: number;
  endLineNumber: number;
  endColumn: number;
}

export interface MonacoLocation {
  uri: string;
  range: MonacoRange;
}

export interface LspCompletionItem {
  label: string;
  kind?: number;
  detail?: string;
  documentation?: string;
  insertText?: string;
  sortText?: string;
  filterText?: string;
}

function position(value: unknown, fallback = 0) {
  const record = value as { line?: unknown; character?: unknown };
  const line = Number(record?.line);
  const character = Number(record?.character);
  return {
    line: Number.isFinite(line) && line >= 0 ? line : fallback,
    character: Number.isFinite(character) && character >= 0 ? character : fallback,
  };
}

export function lspRange(value: unknown): MonacoRange {
  const record = value as { start?: unknown; end?: unknown };
  const start = position(record?.start);
  const end = position(record?.end, start.line);
  return {
    startLineNumber: start.line + 1,
    startColumn: start.character + 1,
    endLineNumber: end.line + 1,
    endColumn: end.character + 1,
  };
}

/** 绝对 file URI → Tenon Monaco model path；非 file URI 原样返回。 */
export function lspUriToModelPath(uri: string, projectRoot?: string) {
  try {
    const parsed = new URL(uri);
    if (parsed.protocol !== "file:") return uri;
    let path = decodeURIComponent(parsed.pathname).replace(/^\/([A-Za-z]:)/, "$1");
    if (projectRoot) {
      const root = projectRoot.replace(/\/$/, "");
      if (path === root) return "";
      if (path.startsWith(`${root}/`)) path = path.slice(root.length + 1);
    }
    return path;
  } catch {
    return uri;
  }
}

function locationRecord(value: unknown) {
  const record = value as {
    uri?: unknown;
    targetUri?: unknown;
    targetSelectionRange?: unknown;
    range?: unknown;
    location?: { range?: unknown };
  };
  const uri =
    typeof record.uri === "string"
      ? record.uri
      : typeof record.targetUri === "string"
        ? record.targetUri
        : "";
  const range =
    record.targetSelectionRange ?? record.range ?? record.location?.range ?? null;
  if (!uri || !range) return null;
  return { uri, range };
}

export function parseLspLocations(
  value: unknown,
  projectRoot?: string
): MonacoLocation[] {
  const records = Array.isArray(value)
    ? value
    : value && typeof value === "object" && Array.isArray((value as { locations?: unknown[] }).locations)
      ? (value as { locations: unknown[] }).locations
      : [value];
  return records.flatMap((record) => {
    const location = locationRecord(record);
    if (!location) return [];
    return [{
      uri: lspUriToModelPath(location.uri, projectRoot),
      range: lspRange(location.range),
    }];
  });
}

function markupText(value: unknown): string {
  if (typeof value === "string") return value;
  const record = value as { value?: unknown };
  return typeof record?.value === "string" ? record.value : "";
}

export function parseLspHover(value: unknown): string {
  if (!value) return "";
  const record = value as { contents?: unknown };
  const contents = record.contents;
  if (Array.isArray(contents)) return contents.map(markupText).filter(Boolean).join("\n\n");
  return markupText(contents);
}

export function parseLspCompletions(value: unknown): LspCompletionItem[] {
  const items = Array.isArray(value)
    ? value
    : Array.isArray((value as { items?: unknown[] })?.items)
      ? (value as { items: unknown[] }).items
      : [];
  return items.flatMap((item) => {
    const record = item as {
      label?: unknown;
      kind?: unknown;
      detail?: unknown;
      documentation?: unknown;
      insertText?: unknown;
      sortText?: unknown;
      filterText?: unknown;
      textEdit?: { newText?: unknown };
    };
    const label = typeof record.label === "string" ? record.label : "";
    if (!label) return [];
    const documentation =
      typeof record.documentation === "string"
        ? record.documentation
        : markupText(record.documentation);
    return [{
      label,
      kind: typeof record.kind === "number" ? record.kind : undefined,
      detail: typeof record.detail === "string" ? record.detail : undefined,
      documentation: documentation || undefined,
      insertText:
        typeof record.insertText === "string"
          ? record.insertText
          : typeof record.textEdit?.newText === "string"
            ? record.textEdit.newText
            : label,
      sortText: typeof record.sortText === "string" ? record.sortText : undefined,
      filterText: typeof record.filterText === "string" ? record.filterText : undefined,
    }];
  });
}

export interface MonacoCompletionSuggestion {
  label: string;
  kind: number;
  detail?: string;
  documentation?: string;
  insertText: string;
  sortText?: string;
  filterText?: string;
  range: {
    startLineNumber: number;
    startColumn: number;
    endLineNumber: number;
    endColumn: number;
  };
}

/** LSP item → Monaco suggestion；万行列表时这是呈现预算内的热路径。 */
export function toMonacoCompletionSuggestions(
  items: LspCompletionItem[],
  range: MonacoCompletionSuggestion["range"]
): MonacoCompletionSuggestion[] {
  const suggestions: MonacoCompletionSuggestion[] = [];
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    suggestions.push({
      label: item.label,
      kind: item.kind ?? 0,
      detail: item.detail,
      documentation: item.documentation,
      insertText: item.insertText ?? item.label,
      sortText: item.sortText,
      filterText: item.filterText,
      range,
    });
  }
  return suggestions;
}

export function parseLspSignatureHelp(value: unknown): string {
  if (!value) return "";
  const record = value as {
    signatures?: Array<{
      label?: unknown;
      parameters?: Array<{ label?: unknown }>;
    }>;
    activeSignature?: unknown;
    activeParameter?: unknown;
  };
  const signatures = record.signatures ?? [];
  if (signatures.length === 0) return "";
  const activeSignature = Number(record.activeSignature);
  const signature = signatures[
    Number.isFinite(activeSignature) && activeSignature >= 0 ? activeSignature : 0
  ];
  const label = typeof signature.label === "string" ? signature.label : "";
  if (!label) return "";
  const activeParameter = Number(record.activeParameter);
  const parameter = signature.parameters?.[
    Number.isFinite(activeParameter) && activeParameter >= 0 ? activeParameter : 0
  ];
  const parameterLabel =
    parameter && typeof parameter.label === "string" ? ` · ${parameter.label}` : "";
  return `${label}${parameterLabel}`;
}

export interface LspCodeAction {
  title: string;
  kind?: string;
  workspaceEdit?: unknown;
}

export function parseLspCodeActions(value: unknown): LspCodeAction[] {
  const records = Array.isArray(value)
    ? value
    : Array.isArray((value as { actions?: unknown[] })?.actions)
      ? (value as { actions: unknown[] }).actions
      : [];
  return records.flatMap((item) => {
    const record = item as {
      title?: unknown;
      kind?: unknown;
      edit?: { workspaceEdit?: unknown };
    };
    const title = typeof record.title === "string" ? record.title : "";
    if (!title || !record.edit?.workspaceEdit) return [];
    return [{
      title,
      kind: typeof record.kind === "string" ? record.kind : undefined,
      workspaceEdit: record.edit.workspaceEdit,
    }];
  });
}
