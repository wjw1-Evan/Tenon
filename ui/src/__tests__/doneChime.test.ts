// 任务完成提示音（§7.5 v1.122）：转迁判定矩阵 + WebAudio 双音合成。
// jsdom 无 AudioContext——经 vi.stubGlobal 注入假实现验证振荡器 / 包络接线。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentStateName } from "../lib/stateColors";
import {
  playDoneChime,
  resetChimeForTests,
  shouldChimeOnTransition,
} from "../lib/notifySound";

describe("shouldChimeOnTransition", () => {
  it("运行态转入 done 才响（§9.1 五态逐一覆盖）", () => {
    const running: AgentStateName[] = ["sensing", "deciding", "executing", "verifying", "fixing"];
    for (const prev of running) {
      expect(shouldChimeOnTransition(prev, "done"), prev).toBe(true);
    }
  });

  it("error / paused / 其余去程不响", () => {
    expect(shouldChimeOnTransition("executing", "error")).toBe(false);
    expect(shouldChimeOnTransition("executing", "paused")).toBe(false);
    expect(shouldChimeOnTransition("executing", "sensing")).toBe(false);
    expect(shouldChimeOnTransition("done", "done")).toBe(false);
  });

  it("非运行态进入 done 不响（挂载即 done 的历史会话 / 会话切换重置）", () => {
    expect(shouldChimeOnTransition("idle", "done")).toBe(false);
    expect(shouldChimeOnTransition("paused", "done")).toBe(false);
    expect(shouldChimeOnTransition("error", "done")).toBe(false);
  });
});

// 假 AudioContext：只实现 playDoneChime 用到的面。
interface FakeOsc {
  type: string;
  frequency: { value: number };
  connect: ReturnType<typeof vi.fn>;
  start: ReturnType<typeof vi.fn>;
  stop: ReturnType<typeof vi.fn>;
}

function makeFakeContext() {
  const oscillators: FakeOsc[] = [];
  const gains: {
    gain: Record<string, ReturnType<typeof vi.fn>>;
    connect: ReturnType<typeof vi.fn>;
  }[] = [];
  class FakeContext {
    state = "running";
    currentTime = 0;
    destination = { id: "destination" };
    resume = vi.fn(async () => {});
    createOscillator = vi.fn(() => {
      const osc: FakeOsc = {
        type: "",
        frequency: { value: 0 },
        connect: vi.fn((dest: unknown) => dest),
        start: vi.fn(),
        stop: vi.fn(),
      };
      oscillators.push(osc);
      return osc;
    });
    createGain = vi.fn(() => {
      const g = {
        gain: {
          setValueAtTime: vi.fn(),
          linearRampToValueAtTime: vi.fn(),
          exponentialRampToValueAtTime: vi.fn(),
        },
        connect: vi.fn((dest: unknown) => dest),
      };
      gains.push(g);
      return g;
    });
  }
  return { FakeContext, oscillators, gains };
}

describe("playDoneChime", () => {
  beforeEach(() => {
    resetChimeForTests();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    resetChimeForTests();
  });

  it("上行双音：两枚正弦振荡器 E5→A5，包络接至 destination", async () => {
    const { FakeContext, oscillators, gains } = makeFakeContext();
    vi.stubGlobal("AudioContext", FakeContext);
    await expect(playDoneChime()).resolves.toBe(true);
    expect(oscillators).toHaveLength(2);
    expect(oscillators.map((o) => o.frequency.value)).toEqual([659.25, 880]);
    expect(oscillators.every((o) => o.type === "sine")).toBe(true);
    // osc → gain → destination 链路完整，且各有起止调度
    expect(gains).toHaveLength(2);
    for (const o of oscillators) {
      expect(o.connect).toHaveBeenCalledTimes(1);
      expect(o.start).toHaveBeenCalledTimes(1);
      expect(o.stop).toHaveBeenCalledTimes(1);
    }
  });

  it("suspended（autoplay 未解锁）时静默跳过不出声", async () => {
    const { FakeContext, oscillators } = makeFakeContext();
    class SuspendedContext extends FakeContext {
      constructor() {
        super();
        this.state = "suspended";
      }
    }
    vi.stubGlobal("AudioContext", SuspendedContext);
    await expect(playDoneChime()).resolves.toBe(false);
    expect(oscillators).toHaveLength(0);
  });

  it("无 AudioContext（jsdom 默认）返回 false 不抛错", async () => {
    await expect(playDoneChime()).resolves.toBe(false);
  });
});
