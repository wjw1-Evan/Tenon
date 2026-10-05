// 编辑器（设计方案 §8.2 / §7.5）：Monaco 内核；多标签；
// 代理写入行带「AI」角标（AI 修改高亮区，§8.6），用户编辑后解除。
import Editor, { type Monaco } from "@monaco-editor/react";
import { useEffect, useRef, useState } from "react";
import type { editor as monacoEditor } from "monaco-editor";
import type { Translate } from "../lib/i18n";
import type { TenonApi } from "../lib/api";
import {
  lspRange,
  parseLspCompletions,
  parseLspHover,
  parseLspCodeActions,
  parseLspLocations,
  parseLspSignatureHelp,
  toMonacoCompletionSuggestions,
} from "../lib/lsp";
import { recordCompletionPresentation } from "../lib/performance";
import "../monacoSetup";

type DecorationsCollection = { clear: () => void };
type SelectionRange = {
  startLineNumber: number;
  startColumn: number;
  endLineNumber: number;
  endColumn: number;
};
type StandaloneEditor = {
  createDecorationsCollection: (
    ranges: Array<{
      range: { startLineNumber: number; startColumn: number; endLineNumber: number; endColumn: number };
      options: Record<string, unknown>;
    }>
  ) => DecorationsCollection;
  revealLineInCenter: (lineNumber: number) => void;
  focus: () => void;
  onDidChangeCursorSelection: (
    cb: (e: { selection: SelectionRange }) => void
  ) => { dispose: () => void };
  getModel: () => { getValueInRange: (range: SelectionRange) => string } | null;
};

type HighlightToken = {
  kind: string;
  start_line: number;
  start_column: number;
  end_line: number;
  end_column: number;
};

/* Tenon 主题（§7.5 设计令牌同步）：与 styles.css 的 --surface /
   --fg 等变量保持一致；data-theme 切换时联动换主题。 */
const TENON_DARK = {
  base: "vs-dark",
  inherit: true,
  rules: [],
  colors: {
    "editor.background": "#14171F",
    "editor.foreground": "#DFE4EE",
    "editorGutter.background": "#14171F",
    "editorLineNumber.foreground": "#39415A",
    "editorLineNumber.activeForeground": "#8B93A7",
    "editor.lineHighlightBackground": "#1A1E28",
    "editorCursor.foreground": "#5C8AF5",
    "editor.selectionBackground": "#2C4A86",
    "editorIndentGuide.background1": "#232938",
    "editorWhitespace.foreground": "#2A3145",
    "editorWidget.background": "#1D2230",
    "editorWidget.border": "#2B3140",
    "scrollbarSlider.background": "#8B93A71F",
    "scrollbarSlider.hoverBackground": "#8B93A733",
    "scrollbarSlider.activeBackground": "#8B93A747",
  },
} satisfies monacoEditor.IStandaloneThemeData;

const TENON_LIGHT = {
  base: "vs",
  inherit: true,
  rules: [],
  colors: {
    "editor.background": "#FFFFFF",
    "editor.foreground": "#1C2434",
    "editorGutter.background": "#FFFFFF",
    "editorLineNumber.foreground": "#C2C9D6",
    "editorLineNumber.activeForeground": "#5F6B81",
    "editor.lineHighlightBackground": "#F2F4F8",
    "editorCursor.foreground": "#2F6BEC",
    "editor.selectionBackground": "#B9D2FB",
    "editorIndentGuide.background1": "#E8EBF1",
    "editorWhitespace.foreground": "#D8DEE9",
    "editorWidget.background": "#FFFFFF",
    "editorWidget.border": "#D4D9E3",
    "scrollbarSlider.background": "#5F6B811F",
    "scrollbarSlider.hoverBackground": "#5F6B8133",
    "scrollbarSlider.activeBackground": "#5F6B8147",
  },
} satisfies monacoEditor.IStandaloneThemeData;

function defineTenonThemes(monaco: Monaco) {
  monaco.editor.defineTheme("tenon-dark", TENON_DARK);
  monaco.editor.defineTheme("tenon-light", TENON_LIGHT);
}

/** 当前解析主题（data-theme 驱动；跟随系统档由 ThemePicker 改属性后联动）。 */
function useResolvedTheme(): "dark" | "light" {
  const [theme, setTheme] = useState<"dark" | "light">(
    () => (document.documentElement.dataset.theme === "light" ? "light" : "dark")
  );
  useEffect(() => {
    const root = document.documentElement;
    const observer = new MutationObserver(() => {
      setTheme(root.dataset.theme === "light" ? "light" : "dark");
    });
    observer.observe(root, { attributes: true, attributeFilter: ["data-theme"] });
    return () => observer.disconnect();
  }, []);
  return theme;
}

export interface EditorTab {
  path: string;
  content: string;
}

/** 编辑器当前选区（行内指令上下文，§8.5）；text 为空即无选区。 */
export interface EditorSelection {
  path: string;
  startLine: number;
  endLine: number;
  text: string;
}

interface Props {
  t: Translate;
  api: TenonApi;
  projectId: string | null;
  projectRoot?: string;
  sessionId?: string | null;
  /** AI ghost text（P2 实验）：设计要求默认关闭。 */
  inlineCompletionEnabled?: boolean;
  /** watcher 文件事件版本；触发 tree-sitter 高亮刷新。 */
  refreshToken?: number;
  tabs: EditorTab[];
  activePath: string | null;
  onSelect: (path: string) => void;
  onClose: (path: string) => void;
  onChange: (path: string, content: string) => void;
  /** 代理改动行号（行级 AI 角标；用户编辑该文件后解除）。 */
  aiModifiedLines?: Record<string, number[]>;
  /** 有未保存编辑的文件集合（§8.2 自动保存指示）。 */
  unsavedPaths?: Record<string, true>;
  /** 未保存圆点的无障碍文案（中英文案外置）。 */
  unsavedTitle?: string;
  /** fuzzy finder / 诊断跳转（§7.4）：path + 1-based line。 */
  goto?: { path: string; line: number; token: number } | null;
  /** 选区变化（去重后；null = 无选区）。行内指令上下文（§8.5）。 */
  onSelectionChange?: (sel: EditorSelection | null) => void;
  /** 右侧分栏文件（§8.1 多标签 / 分栏）；null = 未分栏。 */
  splitPath?: string | null;
  /** 打开 / 关闭分栏；传 path 即打开指定右侧文件。 */
  onSelectSplit?: (path: string | null) => void;
  /** LSP 写盘前 flush 未保存缓冲。 */
  onFlushFile?: (path: string) => Promise<void> | void;
  /** daemon 已原子应用 WorkspaceEdit；调用方刷新打开文件 / 文件树。 */
  onWorkspaceApplied?: (paths: string[]) => Promise<void> | void;
  /** 命令面板撤销/重做入口（§8.2 v1.75）：挂载绑定 active editor，卸载解绑。 */
  bindEditorApi?: (api: { undo: () => void; redo: () => void } | null) => void;
}

export function EditorPane({
  t,
  api,
  projectId,
  projectRoot,
  sessionId = null,
  inlineCompletionEnabled = false,
  refreshToken = 0,
  tabs,
  activePath,
  onSelect,
  onClose,
  onChange,
  aiModifiedLines,
  unsavedPaths,
  unsavedTitle,
  goto,
  onSelectionChange,
  splitPath = null,
  onSelectSplit,
  onFlushFile,
  onWorkspaceApplied,
  bindEditorApi,
}: Props) {
  const editorRef = useRef<StandaloneEditor | null>(null);
  // bindEditorApi 经 ref 透传：卸载解绑用最新回调，不进 effect 依赖。
  const bindRef = useRef(bindEditorApi);
  bindRef.current = bindEditorApi;
  useEffect(() => () => bindRef.current?.(null), []);
  const decorationsRef = useRef<DecorationsCollection | null>(null);
  const resolvedTheme = useResolvedTheme();
  const activePathRef = useRef(activePath);
  const lastSelSigRef = useRef<string | null>(null);
  const selectionCbRef = useRef(onSelectionChange);
  const selSubRef = useRef<{ dispose: () => void } | null>(null);
  const providersRef = useRef<Array<{ dispose: () => void }>>([]);
  const syntaxDecorationsRef = useRef<DecorationsCollection | null>(null);
  const [syntaxTokens, setSyntaxTokens] = useState<HighlightToken[]>([]);
  const runtimeRef = useRef({
    api,
    projectId,
    projectRoot,
    sessionId,
    onFlushFile,
    onWorkspaceApplied,
    inlineCompletionEnabled,
  });

  useEffect(() => {
    runtimeRef.current = {
      api,
      projectId,
      projectRoot,
      sessionId,
      onFlushFile,
      onWorkspaceApplied,
      inlineCompletionEnabled,
    };
  }, [api, projectId, projectRoot, sessionId, onFlushFile, onWorkspaceApplied]);

  useEffect(() => {
    activePathRef.current = activePath;
  }, [activePath]);
  useEffect(() => {
    selectionCbRef.current = onSelectionChange;
  }, [onSelectionChange]);
  // daemon 侧 tree-sitter token 流（§8.2 / §16）：活动文件切换 / watcher 保存后刷新。
  useEffect(() => {
    if (!projectId || !activePath) {
      setSyntaxTokens([]);
      return;
    }
    let alive = true;
    const timer = window.setTimeout(() => {
      api
        .getHighlights(projectId, activePath)
        .then((result) => {
          if (alive) setSyntaxTokens(result.fallback ? [] : result.tokens ?? []);
        })
        .catch(() => {
          if (alive) setSyntaxTokens([]);
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [api, projectId, activePath, refreshToken]);
  useEffect(
    () => () => {
      selSubRef.current?.dispose();
      selSubRef.current = null;
      providersRef.current.forEach((provider) => provider.dispose());
      providersRef.current = [];
    },
    []
  );

  function modelPath(uri: { path: string }) {
    let path = decodeURIComponent(uri.path).replace(/^\//, "");
    const root = runtimeRef.current.projectRoot?.replace(/\/$/, "");
    if (root && path.startsWith(`${root}/`)) path = path.slice(root.length + 1);
    return path;
  }

  function registerLanguageProviders(monaco: Monaco) {
    const request = async (
      path: string,
      action: string,
      line: number,
      character: number,
      extra?: string
    ) => {
      const runtime = runtimeRef.current;
      if (!runtime.projectId || !path) return null;
      await runtime.onFlushFile?.(path);
      return runtime.api.lsp({
        project_id: runtime.projectId,
        path,
      action: action as "completion",
        line,
        character,
        extra,
      });
    };

    const applyEdit = async (workspaceEdit: unknown, path: string) => {
      const runtime = runtimeRef.current;
      if (!runtime.projectId || !runtime.sessionId) {
        throw new Error("active session required");
      }
      await runtime.onFlushFile?.(path);
      const applied = await runtime.api.applyLspEdit(
        runtime.projectId,
        workspaceEdit,
        runtime.sessionId
      );
      await runtime.onWorkspaceApplied?.(applied.files.map((file) => file.path));
      return applied;
    };

    providersRef.current.push(
      monaco.languages.registerCompletionItemProvider("*", {
        triggerCharacters: [".", "/", "'", '"', "("],
        async provideCompletionItems(model, position) {
          const started = performance.now();
          const path = modelPath(model.uri);
          const result = await request(path, "completion", position.lineNumber - 1, position.column - 1);
          const word = model.getWordUntilPosition(position);
          const range = {
            startLineNumber: position.lineNumber,
            endLineNumber: position.lineNumber,
            startColumn: word.startColumn,
            endColumn: word.endColumn,
          };
          const suggestions = toMonacoCompletionSuggestions(
            parseLspCompletions(result?.result),
            range
          );
          recordCompletionPresentation(performance.now() - started);
          return { suggestions };
        },
      })
    );

    providersRef.current.push(
      monaco.languages.registerHoverProvider("*", {
        async provideHover(model, position, token) {
          const path = modelPath(model.uri);
          const result = await request(path, "hover", position.lineNumber - 1, position.column - 1);
          const value = parseLspHover(result?.result);
          if (token.isCancellationRequested || !value) return { contents: [] };
          return { contents: [{ value }], range: lspRange(null) };
        },
      })
    );

    providersRef.current.push(
      monaco.languages.registerDefinitionProvider("*", {
        async provideDefinition(model, position, token) {
          const path = modelPath(model.uri);
          const result = await request(path, "definition", position.lineNumber - 1, position.column - 1);
          if (token.isCancellationRequested) return [];
          return parseLspLocations(result?.result, runtimeRef.current.projectRoot).map((location) => ({
            uri: monaco.Uri.parse(location.uri),
            range: location.range,
          }));
        },
      })
    );

    providersRef.current.push(
      monaco.languages.registerReferenceProvider("*", {
        async provideReferences(model, position, _context, token) {
          const path = modelPath(model.uri);
          const result = await request(path, "references", position.lineNumber - 1, position.column - 1);
          if (token.isCancellationRequested) return [];
          return parseLspLocations(result?.result, runtimeRef.current.projectRoot)
            .map((location) => ({
              uri: monaco.Uri.parse(location.uri),
              range: location.range,
            }));
        },
      })
    );

    providersRef.current.push(
      monaco.languages.registerSignatureHelpProvider("*", {
        signatureHelpTriggerCharacters: ["(", ","],
        async provideSignatureHelp(model, position, _token, _context) {
          const path = modelPath(model.uri);
          const result = await request(path, "signature_help", position.lineNumber - 1, position.column - 1);
          const value = parseLspSignatureHelp(result?.result);
          if (_token.isCancellationRequested || !value) return null;
          return {
            value: {
              signatures: [{ label: value, parameters: [] }],
              activeSignature: 0,
              activeParameter: 0,
            },
            dispose: () => {},
          };
        },
      })
    );

    providersRef.current.push(
      monaco.languages.registerDocumentFormattingEditProvider("*", {
        async provideDocumentFormattingEdits(model, _token, _options) {
          const path = modelPath(model.uri);
          const result = await request(path, "format", 0, 0);
          const edits = Array.isArray(result?.result) ? result.result : [];
          if (edits.length === 0) return [];
          await applyEdit(
            { changes: { [`file://${runtimeRef.current.projectRoot}/${path}`]: edits } },
            path
          );
          // daemon 已原子写盘；onWorkspaceApplied 会刷新 model value。
          return [];
        },
      })
    );

    providersRef.current.push(
      monaco.languages.registerRenameProvider("*", {
        async provideRenameEdits(model, position, newName, token) {
          const path = modelPath(model.uri);
          const result = await request(
            path,
            "rename",
            position.lineNumber - 1,
            position.column - 1,
            newName
          );
          const workspaceEdit = result?.result;
          if (token.isCancellationRequested || !workspaceEdit) return null;
          await applyEdit(workspaceEdit, path);
          return null;
        },
        resolveRenameLocation() {
          return null;
        },
      })
    );

    const applyCommandId = "tenon.lsp.applyWorkspaceEdit";
    providersRef.current.push(
      monaco.editor.registerCommand(
        applyCommandId,
        async (_accessor, workspaceEdit: unknown, path: string) => {
          await applyEdit(workspaceEdit, path);
        }
      )
    );

    providersRef.current.push(
      monaco.languages.registerCodeActionProvider("*", {
        async provideCodeActions(model, _range, _context, token) {
          const path = modelPath(model.uri);
          const result = await request(path, "codeaction", 0, 0);
          if (token.isCancellationRequested) return { actions: [], dispose: () => {} };
          return {
            actions: parseLspCodeActions(result?.result).map((action) => ({
              title: action.title,
              kind: action.kind ?? "quickfix",
              isPreferred: action.kind === "quickfix",
              command: {
                id: applyCommandId,
                title: action.title,
                arguments: [action.workspaceEdit, path],
              },
            })),
            dispose: () => {},
          };
        },
      })
    );

    providersRef.current.push(
      monaco.languages.registerInlineCompletionsProvider("*", {
        freeInlineCompletions() {},
        async provideInlineCompletions(model, position, _context, token) {
          const runtime = runtimeRef.current;
          const path = modelPath(model.uri);
          if (
            !runtime.inlineCompletionEnabled ||
            !runtime.projectId ||
            !runtime.sessionId ||
            !path ||
            token.isCancellationRequested
          ) {
            return { items: [], enableForwardStability: true };
          }
          // 停顿后再请求，避免每次按键打模型。
          await new Promise((resolve) => window.setTimeout(resolve, 350));
          if (token.isCancellationRequested) return { items: [], enableForwardStability: true };
          const value = model.getValue();
          const offset = model.getOffsetAt(position);
          const prefix = value.slice(Math.max(0, offset - 24_000), offset);
          const suffix = value.slice(offset, offset + 24_000);
          try {
            const response = await runtime.api.inlineComplete({
              project_id: runtime.projectId,
              session_id: runtime.sessionId,
              path,
              language: path.split(".").pop(),
              prefix,
              suffix,
            });
            if (token.isCancellationRequested || !response.completion) {
              return { items: [], enableForwardStability: true };
            }
            let text = response.completion;
            const remainder = value.slice(offset);
            let common = 0;
            while (
              common < text.length &&
              common < remainder.length &&
              text[common] === remainder[common]
            ) {
              common += 1;
            }
            text = text.slice(common);
            if (!text) return { items: [], enableForwardStability: true };
            return {
              items: [{ insertText: text }],
              enableForwardStability: true,
            };
          } catch {
            return { items: [], enableForwardStability: true };
          }
        },
      })
    );
  }

  // AI 角标装饰：行号变化时重建
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor || typeof editor.createDecorationsCollection !== "function") return;
    const lines = activePath ? (aiModifiedLines?.[activePath] ?? []) : [];
    if (lines.length === 0) {
      decorationsRef.current?.clear();
      return;
    }
    const ranges = lines.map((line) => ({
      range: {
        startLineNumber: line,
        startColumn: 1,
        endLineNumber: line,
        endColumn: 1,
      },
      options: {
        isWholeLine: true,
        className: "ai-line",
        glyphMarginClassName: "ai-glyph",
        glyphMarginHoverMessage: { value: t("editor.ai_glyph") },
      },
    }));
    decorationsRef.current?.clear();
    decorationsRef.current = editor.createDecorationsCollection(ranges);
  }, [aiModifiedLines, activePath, t]);
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor || typeof editor.createDecorationsCollection !== "function") return;
    const ranges = syntaxTokens.map((token) => ({
      range: {
        startLineNumber: token.start_line + 1,
        startColumn: token.start_column + 1,
        endLineNumber: token.end_line + 1,
        endColumn: token.end_column + 1,
      },
      options: { inlineClassName: `tenon-hl-${token.kind}` },
    }));
    syntaxDecorationsRef.current?.clear();
    syntaxDecorationsRef.current = editor.createDecorationsCollection(ranges);
  }, [syntaxTokens, activePath]);
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor || !goto || activePath !== goto.path) return;
    editor.revealLineInCenter(goto.line);
    editor.focus();
  }, [goto, activePath]);
  const active = tabs.find((t) => t.path === activePath);
  const splitTab = splitPath ? tabs.find((tab) => tab.path === splitPath) : undefined;
  const splitCandidates = tabs.filter((tab) => tab.path !== activePath);
  const toggleSplit = () => {
    if (!onSelectSplit) return;
    if (splitPath) {
      onSelectSplit(null);
      return;
    }
    onSelectSplit(splitCandidates[0]?.path ?? null);
  };
  return (
    <div className="editor-pane" data-testid="editor-pane">
      <div className="tabs" role="tablist">
        {tabs.map((t) => (
          <span
            key={t.path}
            role="tab"
            aria-selected={t.path === activePath}
            className={`tab ${t.path === activePath ? "active" : ""}`}
            onClick={() => onSelect(t.path)}
          >
            {t.path}
            {unsavedPaths?.[t.path] && (
              <span
                className="unsaved-dot"
                data-testid={`unsaved-${t.path}`}
                title={unsavedTitle}
                aria-label={unsavedTitle}
              >
                ●
              </span>
            )}
            <button
              className="tab-close"
              aria-label={`close ${t.path}`}
              onClick={(ev) => {
                ev.stopPropagation();
                onClose(t.path);
              }}
            >
              ×
            </button>
          </span>
        ))}
        <button
          type="button"
          className={`tab-split ${splitPath ? "active" : ""}`}
          data-testid="editor-split"
          aria-pressed={Boolean(splitPath)}
          disabled={!onSelectSplit || tabs.length < 2 || (!splitPath && splitCandidates.length === 0)}
          title={splitPath ? t("editor.split_close") : t("editor.split_open")}
          onClick={toggleSplit}
        >
          {splitPath ? t("editor.split_close") : t("editor.split_open")}
        </button>
      </div>
      <div className={`editor-body ${splitTab ? "split" : ""}`}>
        {active ? (
          <section className="editor-group">
          <Editor
            height="100%"
            theme={resolvedTheme === "light" ? "tenon-light" : "tenon-dark"}
            beforeMount={defineTenonThemes}
            loading={<div className="editor-loading">{t("editor.loading")}</div>}
            options={{
              // 简洁大方的编辑面（§7.5）：minimap 在 WebView 渲染错位（E2E
              // 实测）且非必需，禁用；其余为阅读体验微调。
              minimap: { enabled: false },
              fontSize: 13,
              lineHeight: 1.7,
              fontLigatures: true,
              smoothScrolling: true,
              cursorBlinking: "smooth",
              cursorSmoothCaretAnimation: "on",
              renderLineHighlight: "all",
              scrollBeyondLastLine: false,
              padding: { top: 8, bottom: 24 },
              stickyScroll: { enabled: false },
            }}
            path={active.path}
            value={active.content}
            onMount={(editor, monacoInstance) => {
              editorRef.current = editor;
              registerLanguageProviders(monacoInstance);
              // 命令面板撤销/重做（§8.2 v1.75）：trigger active editor，不依赖焦点。
              bindEditorApi?.({
                undo: () => editor.trigger("palette", "undo", null),
                redo: () => editor.trigger("palette", "redo", null),
              });
              // 选区上报（§8.5 行内指令上下文）：按 path + 区间签名去重，
              // 光标移动 / 输入不重复触发上层渲染。
              selSubRef.current?.dispose();
              selSubRef.current = editor.onDidChangeCursorSelection((e) => {
                const path = activePathRef.current;
                const sel = e.selection;
                const text =
                  sel.startLineNumber === sel.endLineNumber &&
                  sel.startColumn === sel.endColumn
                    ? ""
                    : (editor.getModel()?.getValueInRange(sel) ?? "");
                const sig = `${path}\u0000${sel.startLineNumber}:${sel.endLineNumber}\u0000${text ? "1" : "0"}`;
                if (sig === lastSelSigRef.current) return;
                lastSelSigRef.current = sig;
                selectionCbRef.current?.(
                  text
                    ? {
                        path: path ?? "",
                        startLine: sel.startLineNumber,
                        endLine: sel.endLineNumber,
                        text,
                      }
                    : null
                );
              });
            }}
            onChange={(v) => {
              // 用户编辑该文件 → 行级 AI 角标解除由上层 onChange 管理（§8.6）
              onChange(active.path, v ?? "");
            }}
          />
          </section>
        ) : (
          <section className="editor-group">
            <div className="muted editor-empty">Tenon</div>
          </section>
        )}
        {splitTab && (
          <section className="editor-group secondary" data-testid="editor-split-group">
            <div className="split-head">
              <label className="muted" htmlFor="split-file-select">{t("editor.split_file")}</label>
              <select
                id="split-file-select"
                data-testid="split-file-select"
                value={splitTab.path}
                onChange={(event) => onSelectSplit?.(event.target.value)}
              >
                {tabs
                  .filter((tab) => tab.path !== activePath)
                  .map((tab) => (
                    <option key={tab.path} value={tab.path}>{tab.path}</option>
                  ))}
              </select>
            </div>
            <Editor
              height="100%"
              theme={resolvedTheme === "light" ? "tenon-light" : "tenon-dark"}
              beforeMount={defineTenonThemes}
              loading={<div className="editor-loading">{t("editor.loading")}</div>}
              options={{
                minimap: { enabled: false },
                fontSize: 13,
                lineHeight: 1.7,
                fontLigatures: true,
                smoothScrolling: true,
                renderLineHighlight: "all",
                scrollBeyondLastLine: false,
                padding: { top: 8, bottom: 24 },
              stickyScroll: { enabled: false },
              }}
              path={splitTab.path}
              value={splitTab.content}
              onChange={(value) => onChange(splitTab.path, value ?? "")}
            />
          </section>
        )}
      </div>
    </div>
  );
}
