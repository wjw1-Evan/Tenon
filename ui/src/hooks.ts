// 全局快捷键（设计方案 §7.4 核心集）。
import { useEffect } from "react";

export interface ShortcutHandlers {
  onPalette?: () => void; // Cmd/Ctrl+Shift+P
  onGotoFile?: () => void; // Cmd/Ctrl+P
  onPauseOrClose?: () => void; // Esc
  onStop?: () => void; // Cmd/Ctrl+.
  onTimeline?: () => void; // Cmd/Ctrl+Alt+Z
  onSidebar?: () => void; // Cmd/Ctrl+B
}

export function useShortcuts(handlers: ShortcutHandlers) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.shiftKey && e.key.toLowerCase() === "p") {
        e.preventDefault();
        handlers.onPalette?.();
      } else if (mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "p") {
        e.preventDefault();
        handlers.onGotoFile?.();
      } else if (e.key === "Escape") {
        handlers.onPauseOrClose?.();
      } else if (mod && e.key === ".") {
        e.preventDefault();
        handlers.onStop?.();
      } else if (mod && e.altKey && e.key.toLowerCase() === "z") {
        e.preventDefault();
        handlers.onTimeline?.();
      } else if (mod && !e.shiftKey && e.key.toLowerCase() === "b") {
        e.preventDefault();
        handlers.onSidebar?.();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [handlers]);
}
