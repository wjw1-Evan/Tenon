// 状态色（设计方案 §7.5 设计系统要点）
export type AgentStateName =
  | "idle"
  | "sensing"
  | "deciding"
  | "executing"
  | "verifying"
  | "fixing"
  | "paused"
  | "awaiting_confirm"
  | "error"
  | "done"
  | "rolled_back";

export const STATE_COLORS: Record<AgentStateName, string> = {
  sensing: "#2f6fed", // 感知（蓝）
  deciding: "#5b6b7a",
  executing: "#d9a514", // 执行（黄）
  verifying: "#7d4fd3", // 验证（紫）
  fixing: "#7d4fd3",
  paused: "#8a8f98",
  awaiting_confirm: "#d9a514", // v2.0 档位确认（design-v2.md §4.1）：执行色系（任务在途、等用户）
  error: "#d43d3d", // 失败（红）
  done: "#2da44e", // 完成（绿）
  rolled_back: "#8a8f98",
  idle: "#8a8f98",
};

// 代理循环正在推进的状态：发送按钮此时显示「停止」（v1.59）。
// awaiting_confirm（v2.0）随 v2.37 档位退役移出运行态；类型与色保留供旧库会话回显。
export const RUNNING_STATES: ReadonlySet<AgentStateName> = new Set([
  "sensing",
  "deciding",
  "executing",
  "verifying",
  "fixing",
]);
