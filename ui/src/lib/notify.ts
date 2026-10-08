// OS 级任务终态通知（§7.2 / §7.5 v1.178）：运行→done / error 且窗口失焦时
// 发一条系统通知。桌面壳走 tauri-plugin-notification（v2，经内核 invoke 命令
// 直调、不引 guest-js 包），浏览器走 Web Notification API（首次请求授权、
// denied 永不骚扰、不支持的环境静默）。聚焦时零打扰；跨项目不推送（v1.98
// 边界保留，徽标仍是跨项目 attention 承载）；不设应用内开关——OS 通知设置
// 即权威开关。

import { RUNNING_STATES, type AgentStateName } from "./stateColors";

interface TauriInternals {
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
}

function tauriInvoke(): TauriInternals["invoke"] | null {
  if (typeof window === "undefined") return null;
  const internals = (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ as
    | TauriInternals
    | undefined;
  return internals && typeof internals.invoke === "function" ? internals.invoke : null;
}

/** 转移判定（单点纯函数）：仅「运行态 → done / error」且窗口失焦才通知。 */
export function shouldNotifyOnTransition(
  prev: AgentStateName,
  next: AgentStateName,
  hidden: boolean,
): boolean {
  if (!hidden) return false;
  if (!RUNNING_STATES.has(prev)) return false;
  return next === "done" || next === "error";
}

/** 确保通知权限（幂等）：桌面壳查插件权限，浏览器查 Notification.permission。 */
export async function ensureNotifyPermission(): Promise<boolean> {
  try {
    const invoke = tauriInvoke();
    if (invoke) {
      const granted = await invoke("plugin:notification|is_permission_granted");
      if (granted === true) return true;
      const requested = await invoke("plugin:notification|request_permission");
      return requested === "granted" || requested === true;
    }
    if (typeof Notification === "undefined") return false;
    if (Notification.permission === "granted") return true;
    if (Notification.permission === "denied") return false;
    return (await Notification.requestPermission()) === "granted";
  } catch {
    return false;
  }
}

/** 发送系统通知：权限未授 / 失败静默（通知是尽力而为的提醒，非承诺）。 */
export async function notifyTaskFinished(title: string, body: string): Promise<void> {
  try {
    if (!(await ensureNotifyPermission())) return;
    const invoke = tauriInvoke();
    if (invoke) {
      await invoke("plugin:notification|notify", {
        options: { title, body },
      });
      return;
    }
    if (typeof Notification === "undefined") return;
    new Notification(title, { body });
  } catch {
    // 静默：通知失败不影响任务结果呈现
  }
}
