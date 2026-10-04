// 编辑器（设计方案 §8.2 / §7.5）：Monaco 内核；多标签；
// 代理写入行带「AI」角标（AI 修改高亮区，§8.6），用户编辑后解除。
import Editor from "@monaco-editor/react";
import { useEffect, useRef } from "react";

type DecorationsCollection = { clear: () => void };
type StandaloneEditor = {
  createDecorationsCollection: (
    ranges: Array<{
      range: { startLineNumber: number; startColumn: number; endLineNumber: number; endColumn: number };
      options: Record<string, unknown>;
    }>
  ) => DecorationsCollection;
};

export interface EditorTab {
  path: string;
  content: string;
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
}: Props) {
  const editorRef = useRef<StandaloneEditor | null>(null);
  const decorationsRef = useRef<DecorationsCollection | null>(null);

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
            theme="vs-dark"
            path={active.path}
            value={active.content}
            onMount={(editor) => {
              editorRef.current = editor;
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
