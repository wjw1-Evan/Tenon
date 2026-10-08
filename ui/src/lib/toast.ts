// 全局通知 store（§7.5 v1.167）：应用内右下角 Toast 的模块级发布订阅。
// 无 context / 状态库依赖——任意模块 import 即可发通知，ToastHost 订阅渲染。
// 行为契约：success/info 3s 自动消退；error 留驻、✕ 手关（不接 Esc——Esc 语义
// 归暂停代理，关通知不得误停任务）；同文案 error 5s 去重（自动保存高频失败
// 不刷屏）；最多堆叠 3 条、超出挤掉最旧非 error 条目。

import { useSyncExternalStore } from "react";

export type ToastKind = "success" | "info" | "error";

export interface ToastItem {
  id: number;
  kind: ToastKind;
  message: string;
}

const MAX_STACK = 3;
const AUTO_DISMISS_MS = 3000;
const ERROR_DEDUPE_MS = 5000;

let nextId = 1;
let items: ToastItem[] = [];
const listeners = new Set<() => void>();
/** 同文案 error 的最近弹出时刻（去重窗口）。 */
const lastErrorAt = new Map<string, number>();

function emit() {
  for (const listener of listeners) listener();
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getToasts(): ToastItem[] {
  return items;
}

export function dismiss(id: number): void {
  if (!items.some((toastItem) => toastItem.id === id)) return;
  items = items.filter((toastItem) => toastItem.id !== id);
  emit();
}

function push(kind: ToastKind, message: string): void {
  const trimmed = message.trim();
  if (!trimmed) return;
  if (kind === "error") {
    const now = Date.now();
    const at = lastErrorAt.get(trimmed) ?? 0;
    lastErrorAt.set(trimmed, now);
    if (now - at < ERROR_DEDUPE_MS) return;
  }
  const item: ToastItem = { id: nextId++, kind, message: trimmed };
  let next = [...items];
  if (next.length >= MAX_STACK) {
    // 堆叠上限：优先挤掉最旧非 error；全为 error 才挤最旧 error
    const victim = next.findIndex((toastItem) => toastItem.kind !== "error");
    if (victim === -1) next = next.slice(next.length - MAX_STACK + 1);
    else next.splice(victim, 1);
  }
  next.push(item);
  items = next;
  emit();
  if (kind !== "error") {
    window.setTimeout(() => dismiss(item.id), AUTO_DISMISS_MS);
  }
}

export const toast = {
  success: (message: string) => push("success", message),
  info: (message: string) => push("info", message),
  error: (message: string) => push("error", message),
};

/** ToastHost 订阅钩子（useSyncExternalStore 快照为 items 引用，变更即重渲染）。 */
export function useToasts(): ToastItem[] {
  return useSyncExternalStore(subscribe, getToasts, getToasts);
}

/** 测试钩子：清空条目与去重窗口（不动监听器——宿主可能已挂载）。 */
export function __resetToastsForTest(): void {
  items = [];
  lastErrorAt.clear();
}
