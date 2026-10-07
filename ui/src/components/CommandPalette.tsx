// 命令面板（设计方案 §7.4：Cmd/Ctrl+Shift+P；无障碍要求：全部命令键盘可达）。
import { useEffect, useMemo, useRef, useState } from "react";
import type { Translate } from "../lib/i18n";

export interface Command {
  id: string;
  label: string;
  run: () => void;
}

interface Props {
  open: boolean;
  onClose: () => void;
  commands: Command[];
  t: Translate;
}

export function CommandPalette({ open, onClose, commands, t }: Props) {
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);

  useEffect(() => {
    if (open) {
      setQuery("");
      setActiveIndex(0);
      inputRef.current?.focus();
    }
  }, [open]);

  useEffect(() => setActiveIndex(0), [query]);

  const filtered = useMemo(() => {
    const q = query.toLowerCase();
    return commands.filter((c) => c.label.toLowerCase().includes(q));
  }, [commands, query]);

  if (!open) return null;
  const runAt = (index: number) => {
    const command = filtered[index];
    if (!command) return;
    command.run();
    onClose();
  };
  return (
    <div className="palette-overlay" onClick={onClose} data-testid="command-palette">
      <div className="palette" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          value={query}
          placeholder={t("command.placeholder")}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              onClose();
            }
            if (e.key === "Enter") {
              e.preventDefault();
              runAt(activeIndex);
            }
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setActiveIndex((index) => Math.min(index + 1, Math.max(filtered.length - 1, 0)));
            }
            if (e.key === "ArrowUp") {
              e.preventDefault();
              setActiveIndex((index) => Math.max(index - 1, 0));
            }
          }}
        />
        <ul ref={listRef}>
          {filtered.map((c, index) => (
            <li key={c.id} className={index === activeIndex ? "active" : ""}>
              <button
                aria-current={index === activeIndex}
                onClick={() => {
                  c.run();
                  onClose();
                }}
              >
                {c.label}
              </button>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
