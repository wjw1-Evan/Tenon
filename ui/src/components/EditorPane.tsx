// 编辑器（设计方案 §8.2 / §7.5）：Monaco 内核；多标签；
// 代理写入行带「AI」角标（AI 修改高亮区，§8.6），用户编辑后解除。
import Editor, { type Monaco } from "@monaco-editor/react";
import { useEffect, useRef, useState } from "react";

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
} as const;

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
} as const;

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
}

export function EditorPane({
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
}: Props) {
  const editorRef = useRef<StandaloneEditor | null>(null);
  const decorationsRef = useRef<DecorationsCollection | null>(null);
  const resolvedTheme = useResolvedTheme();
  const activePathRef = useRef(activePath);
  const lastSelSigRef = useRef<string | null>(null);
  const selectionCbRef = useRef(onSelectionChange);
  const selSubRef = useRef<{ dispose: () => void } | null>(null);

  useEffect(() => {
    activePathRef.current = activePath;
  }, [activePath]);
  useEffect(() => {
    selectionCbRef.current = onSelectionChange;
  }, [onSelectionChange]);
  useEffect(
    () => () => {
      selSubRef.current?.dispose();
      selSubRef.current = null;
    },
    []
  );

  // AI 角标装饰：行号变化时重建
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor) return;
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
        glyphMarginHoverMessage: { value: "AI 修改区（§8.6）" },
      },
    }));
    decorationsRef.current?.clear();
    decorationsRef.current = editor.createDecorationsCollection(ranges);
  }, [aiModifiedLines, activePath]);
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor || !goto || activePath !== goto.path) return;
    editor.revealLineInCenter(goto.line);
    editor.focus();
  }, [goto, activePath]);
  const active = tabs.find((t) => t.path === activePath);
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
      </div>
      <div className="editor-body">
        {active ? (
          <Editor
            height="100%"
            theme={resolvedTheme === "light" ? "tenon-light" : "tenon-dark"}
            beforeMount={defineTenonThemes}
            loading={<div className="editor-loading">加载中…</div>}
            path={active.path}
            value={active.content}
            onMount={(editor) => {
              editorRef.current = editor;
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
              // 用户编辑该文件 → 行级 AI 角标解除（§8.6）
              if (aiModifiedLines && aiModifiedLines[active.path]?.length) {
                const rest = { ...aiModifiedLines };
                delete rest[active.path];
                // 通过回调清空由上层管理；此处仅透传内容
              }
              onChange(active.path, v ?? "");
            }}
          />
        ) : (
          <div className="muted editor-empty">Tenon</div>
        )}
      </div>
    </div>
  );
}
