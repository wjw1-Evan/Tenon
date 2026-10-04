// 行内 AI 指令（设计方案 §7.4 Cmd/Ctrl+I / §8.5 共生集成点「行内指令」，S2 / T8）：
// 选中代码（无选区退化为整文件）→ 自然语言改写指令 → 经当前 project_id 会话注入；
// 就地 diff 由既有 AI 行角标与 watcher 回读呈现（§8.1 / §8.6）。
import { useEffect, useRef, useState } from "react";
import type { EditorSelection } from "./EditorPane";
import type { Translate } from "../lib/i18n";

export interface InlineTarget {
  path: string;
  startLine?: number;
  endLine?: number;
  excerpt?: string;
}

/** 把选区上下文 + 用户指令组装成会话任务文本（T8：选中区外零改动）。 */
export function buildInlineTask(instruction: string, target: InlineTarget): string {
  const range =
    target.startLine !== undefined && target.endLine !== undefined
      ? `${target.startLine}-${target.endLine} 行`
      : "整个文件";
  const excerpt = target.excerpt?.trim()
    ? `选中代码：\n\`\`\`\n${target.excerpt.trim().slice(0, 1200)}\n\`\`\`\n`
    : "";
  return [
    `行内指令：在 ${target.path} 的${range}内，${instruction.trim()}`,
    excerpt,
    "约束：只修改上述目标区域，选中区外零改动；完成后确认无新诊断或回归。",
  ]
    .filter(Boolean)
    .join("\n");
}

interface Props {
  open: boolean;
  /** 当前选区；null = 整文件模式。 */
  selection: EditorSelection | null;
  activePath: string | null;
  t: Translate;
  onClose: () => void;
  onSend: (instruction: string, target: InlineTarget) => void;
}

export function InlineInstruction({ open, selection, activePath, t, onClose, onSend }: Props) {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const [value, setValue] = useState("");

  useEffect(() => {
    if (open) {
      setValue("");
      window.setTimeout(() => inputRef.current?.focus(), 0);
    }
  }, [open]);

  if (!open) return null;
  const target: InlineTarget | null = activePath
    ? selection && selection.path === activePath
      ? {
          path: activePath,
          startLine: selection.startLine,
          endLine: selection.endLine,
          excerpt: selection.text,
        }
      : { path: activePath }
    : null;

  const submit = () => {
    const trimmed = value.trim();
    if (!trimmed || !target) return;
    onSend(trimmed, target);
    onClose();
  };

  return (
    <div className="inline-overlay" onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div
        className="inline-card"
        role="dialog"
        aria-modal="true"
        aria-label={t("inline.title")}
        data-testid="inline-instruction"
      >
        <div className="inline-head">
          <strong>{t("inline.title")}</strong>
          <span className="inline-context" data-testid="inline-context">
            {target
              ? target.startLine !== undefined
                ? t("inline.range")
                    .replace("{path}", target.path)
                    .replace("{start}", String(target.startLine))
                    .replace("{end}", String(target.endLine ?? target.startLine))
                : t("inline.whole_file").replace("{path}", target.path)
              : t("inline.no_file")}
          </span>
        </div>
        <textarea
          ref={inputRef}
          rows={3}
          value={value}
          placeholder={t("inline.placeholder")}
          aria-label={t("inline.title")}
          data-testid="inline-input"
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
              e.preventDefault();
              submit();
            }
          }}
        />
        <div className="inline-actions">
          <button type="button" onClick={onClose} data-testid="inline-cancel">
            {t("inline.cancel")}
          </button>
          <button
            type="button"
            className="primary"
            onClick={submit}
            disabled={!target}
            data-testid="inline-send"
          >
            {t("inline.send")}
          </button>
        </div>
      </div>
    </div>
  );
}
