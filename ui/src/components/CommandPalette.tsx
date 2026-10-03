// 命令面板（设计方案 §7.4：Cmd/Ctrl+Shift+P；无障碍要求：全部命令可达）。
import { useEffect, useMemo, useRef, useState } from "react";

export interface Command {
  id: string;
  label: string;
  run: () => void;
}

interface Props {
  open: boolean;
  onClose: () => void;
  commands: Command[];
}

export function CommandPalette({ open, onClose, commands }: Props) {
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQuery("");
      inputRef.current?.focus();
    }
  }, [open]);

  const filtered = useMemo(() => {
    const q = query.toLowerCase();
    return commands.filter((c) => c.label.toLowerCase().includes(q));
  }, [commands, query]);

  if (!open) return null;
  return (
    <div className="palette-overlay" onClick={onClose} data-testid="command-palette">
      <div className="palette" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          value={query}
          placeholder="Type a command…"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") onClose();
            if (e.key === "Enter" && filtered[0]) {
              filtered[0].run();
              onClose();
            }
          }}
        />
        <ul>
          {filtered.map((c) => (
            <li key={c.id}>
              <button
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
