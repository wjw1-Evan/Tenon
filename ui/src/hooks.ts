// 全局快捷键（设计方案 §7.4 核心集）。
import { useEffect } from "react";

export interface ShortcutHandlers {
  onPalette?: () => void; // Cmd/Ctrl+Shift+P
  onGotoFile?: () => void; // Cmd/Ctrl+P
  onInlineInstruction?: () => void; // Cmd/Ctrl+I 行内 AI 指令（§7.4 / §8.5）
  onPauseOrClose?: () => void; // Esc
  onStop?: () => void; // Cmd/Ctrl+.
  onSidebar?: () => void; // Cmd/Ctrl+B
  onPanel?: () => void; // Cmd/Ctrl+J 底部面板（§7.4）
  onSave?: () => void; // Cmd/Ctrl+S（自动保存下的立即保存，§8.2）
  onSettings?: () => void; // Cmd/Ctrl+,（设置面板，§7.2）
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
      } else if (mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "i") {
        e.preventDefault();
        handlers.onInlineInstruction?.();
      } else if (e.key === "Escape") {
        // 弹层 / 输入框各自处理的 Esc 会 preventDefault：全局暂停代理
        // 不得再触发（关弹层误暂停运行中任务是常见踩踏）
        if (e.defaultPrevented) return;
        handlers.onPauseOrClose?.();
      } else if (mod && e.key === ".") {
        e.preventDefault();
        handlers.onStop?.();
      } else if (mod && !e.shiftKey && e.key.toLowerCase() === "b") {
        e.preventDefault();
        handlers.onSidebar?.();
      } else if (mod && !e.shiftKey && e.key.toLowerCase() === "j") {
        e.preventDefault();
        handlers.onPanel?.();
      } else if (mod && e.key.toLowerCase() === "s") {
        e.preventDefault();
        handlers.onSave?.();
      } else if (mod && e.key === ",") {
        e.preventDefault();
        handlers.onSettings?.();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [handlers]);
}
