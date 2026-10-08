// 快捷键速查表（§7.5 v1.167）：Cmd/Ctrl+/ 或命令面板 help.shortcuts 打开——
// 分组列出全部全局快捷键（组合显示按平台 ⌘/Ctrl 渲染，数据单源 lib/shortcuts.ts）。
// Esc 收起 preventDefault（不穿透全局「暂停代理」，与命令面板同法）。
import { useEffect, useRef } from "react";
import { SHORTCUT_GROUPS, formatCombo } from "../lib/shortcuts";
import type { Translate } from "../lib/i18n";

interface Props {
  open: boolean;
  onClose: () => void;
  t: Translate;
}

export function ShortcutsDialog({ open, onClose, t }: Props) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    ref.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return (
    <div className="palette-overlay" onClick={onClose} data-testid="shortcuts-dialog">
      <div
        className="shortcuts-pane"
        role="dialog"
        aria-modal="true"
        aria-label={t("shortcuts.title")}
        tabIndex={-1}
        ref={ref}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="shortcuts-head">{t("shortcuts.title")}</div>
        {SHORTCUT_GROUPS.map((group) => (
          <div key={group.titleKey} className="shortcuts-group">
            <div className="shortcuts-group-title">{t(group.titleKey)}</div>
            <ul>
              {group.items.map((item) => (
                <li key={item.id} className="shortcuts-row">
                  <span>{t(item.labelKey)}</span>
                  <kbd className="shortcuts-combo">{formatCombo(item.keys)}</kbd>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>
    </div>
  );
}
