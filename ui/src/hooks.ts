// 全局快捷键（设计方案 §7.4 核心集；组合显示单源 lib/shortcuts.ts）。
// v1.167 扩展：Cmd/Ctrl+Alt+N 新任务、Alt+W 关编辑器标签、Alt+J 循环底部
// tab、Cmd/Ctrl+/ 速查表——浏览器保留 Cmd/Ctrl+T/W，故取 Alt 档。
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
  onNewTask?: () => void; // Cmd/Ctrl+Alt+N 新任务（active 项目草稿态，v1.167）
  onCloseTab?: () => void; // Cmd/Ctrl+Alt+W 关闭编辑器活动标签（v1.167）
  onCycleBottomTab?: () => void; // Cmd/Ctrl+Alt+J 循环底部面板 tab（v1.167）
  onShortcuts?: () => void; // Cmd/Ctrl+/ 快捷键速查表（v1.167）
}

export function useShortcuts(handlers: ShortcutHandlers) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();
      if (mod && e.shiftKey && key === "p") {
        e.preventDefault();
        handlers.onPalette?.();
      } else if (mod && !e.shiftKey && !e.altKey && key === "p") {
        e.preventDefault();
        handlers.onGotoFile?.();
      } else if (mod && !e.shiftKey && !e.altKey && key === "i") {
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
      } else if (mod && !e.shiftKey && !e.altKey && key === "b") {
        e.preventDefault();
        handlers.onSidebar?.();
      } else if (mod && e.altKey && !e.shiftKey && key === "j") {
        // Alt 变体先于普通 J 判定（else-if 链），两者互不串扰
        e.preventDefault();
        handlers.onCycleBottomTab?.();
      } else if (mod && !e.shiftKey && key === "j") {
        e.preventDefault();
        handlers.onPanel?.();
      } else if (mod && !e.shiftKey && !e.altKey && key === "s") {
        e.preventDefault();
        handlers.onSave?.();
      } else if (mod && e.key === ",") {
        e.preventDefault();
        handlers.onSettings?.();
      } else if (mod && e.altKey && !e.shiftKey && key === "n") {
        e.preventDefault();
        handlers.onNewTask?.();
      } else if (mod && e.altKey && !e.shiftKey && key === "w") {
        e.preventDefault();
        handlers.onCloseTab?.();
      } else if (mod && !e.shiftKey && !e.altKey && e.key === "/") {
        e.preventDefault();
        handlers.onShortcuts?.();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [handlers]);
}
