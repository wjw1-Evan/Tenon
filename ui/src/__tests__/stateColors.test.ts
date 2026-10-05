import { describe, expect, it } from "vitest";
import { STATE_COLORS } from "../lib/stateColors";

describe("状态色（§7.5 设计系统）", () => {
  it("五种语义色齐全且互异", () => {
    // 感知蓝 / 执行黄 / 验证紫 / 失败红 / 完成绿
    expect(STATE_COLORS.sensing).toBe("#2f6fed");
    expect(STATE_COLORS.executing).toBe("#d9a514");
    expect(STATE_COLORS.verifying).toBe("#7d4fd3");
    expect(STATE_COLORS.error).toBe("#d43d3d");
    expect(STATE_COLORS.done).toBe("#2da44e");
    // 六种语义色互异（中性态可共用灰）
    const semantic = [
      STATE_COLORS.sensing,
      STATE_COLORS.executing,
      STATE_COLORS.verifying,
      STATE_COLORS.error,
      STATE_COLORS.done,
    ];
    expect(new Set(semantic).size).toBe(5);
  });

  it("全部状态都有色", () => {
    for (const s of [
      "idle", "sensing", "deciding", "executing", "verifying", "fixing",
"paused", "error", "done", "rolled_back",
    ] as const) {
      expect(STATE_COLORS[s]).toMatch(/^#[0-9a-f]{6}$/);
    }
  });
});
