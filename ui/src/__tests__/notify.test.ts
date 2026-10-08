// OS 级任务终态通知（§7.2 / §7.5 v1.178）：转移判定矩阵 + 双通道分支。
// 本环境 jsdom 无 localStorage / Notification，依仓库惯例 vi.stubGlobal 桩。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ensureNotifyPermission,
  notifyTaskFinished,
  shouldNotifyOnTransition,
} from "../lib/notify";
import type { AgentStateName } from "../lib/stateColors";

describe("shouldNotifyOnTransition（v1.178 §7.2 转移判定）", () => {
  const cases: Array<[AgentStateName, AgentStateName, boolean, boolean]> = [
    // prev, next, hidden, 期望
    ["executing", "done", true, true],
    ["verifying", "error", true, true],
    ["sensing", "done", true, true],
    ["executing", "done", false, false], // 聚焦零打扰
    ["executing", "error", false, false],
    ["idle", "done", true, false], // 非运行态起点（挂载即终态不通知）
    ["paused", "done", true, false],
    ["done", "done", true, false], // 同态不通知
    ["executing", "paused", true, false], // 暂停不是终态
    ["executing", "idle", true, false],
  ];
  for (const [prev, next, hidden, expected] of cases) {
    it(`${prev}→${next} hidden=${hidden} → ${expected}`, () => {
      expect(shouldNotifyOnTransition(prev, next, hidden)).toBe(expected);
    });
  }
});

describe("notify 通道分支（v1.178）", () => {
  beforeEach(() => {
    vi.stubGlobal("document", { hidden: true });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("桌面壳：权限已授 → 经 plugin:notification|notify 发送", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce(true) // is_permission_granted
      .mockResolvedValueOnce(undefined); // notify
    vi.stubGlobal("__TAURI_INTERNALS__", { invoke });
    await notifyTaskFinished("Tenon", "任务完成");
    expect(invoke).toHaveBeenCalledWith("plugin:notification|is_permission_granted");
    expect(invoke).toHaveBeenCalledWith("plugin:notification|notify", {
      options: { title: "Tenon", body: "任务完成" },
    });
  });

  it("桌面壳：权限未授 → 请求授权后发送；拒绝即不打扰", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce("granted")
      .mockResolvedValueOnce(undefined);
    vi.stubGlobal("__TAURI_INTERNALS__", { invoke });
    await notifyTaskFinished("Tenon", "任务完成");
    expect(invoke).toHaveBeenCalledWith("plugin:notification|request_permission");
    expect(invoke).toHaveBeenCalledWith("plugin:notification|notify", {
      options: { title: "Tenon", body: "任务完成" },
    });

    const deniedInvoke = vi
      .fn()
      .mockResolvedValueOnce(false)
      .mockResolvedValueOnce("denied");
    vi.stubGlobal("__TAURI_INTERNALS__", { invoke: deniedInvoke });
    await notifyTaskFinished("Tenon", "任务完成");
    expect(deniedInvoke).toHaveBeenCalledTimes(2);
    expect(deniedInvoke.mock.calls.some(([c]) => c === "plugin:notification|notify")).toBe(false);
  });

  it("浏览器：Notification 可用且授权 → new Notification；不支持 → 静默", async () => {
    class FakeNotification {
      static permission = "default";
      constructor(
        public title: string,
        public options?: NotificationOptions,
      ) {}
      static requestPermission = vi.fn().mockResolvedValue("granted");
    }
    vi.stubGlobal("Notification", FakeNotification);
    await notifyTaskFinished("web", "完成");
    expect(FakeNotification.requestPermission).toHaveBeenCalled();
    const instance = new (FakeNotification as unknown as new (
      t: string,
      o?: NotificationOptions,
    ) => FakeNotification)("web", { body: "probe" });
    expect(instance.title).toBe("web");

    vi.stubGlobal("Notification", undefined);
    await expect(notifyTaskFinished("web", "完成")).resolves.toBeUndefined();
  });
});
