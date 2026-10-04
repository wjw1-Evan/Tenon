// 自动保存调度器（§8.2）：去抖写盘 / flush 立即保存 / 失败保留待重试。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createAutoSaver } from "../lib/autosave";

describe("autosave", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("编辑停顿 delayMs 后写盘一次；连续编辑只保留最后一次", async () => {
    const saved: Array<[string, string]> = [];
    const saver = createAutoSaver(async (p, c) => void saved.push([p, c]), 1000);

    saver.schedule("a.ts", "v1");
    vi.advanceTimersByTime(600);
    saver.schedule("a.ts", "v2"); // 去抖窗口内再次编辑
    vi.advanceTimersByTime(600); // 距 v1 已 1200ms，但距 v2 仅 600ms
    expect(saved).toEqual([]);

    await vi.advanceTimersByTimeAsync(400);
    expect(saved).toEqual([["a.ts", "v2"]]);
    saver.dispose();
  });

  it("flush 立即写盘并取消去抖，不重复保存", async () => {
    const saved: Array<[string, string]> = [];
    const saver = createAutoSaver(async (p, c) => void saved.push([p, c]), 1000);

    saver.schedule("b.ts", "hello");
    const flush = saver.flush("b.ts");
    vi.advanceTimersByTime(0); // 让 flush 内 await 队列跑完
    await flush;
    expect(saved).toEqual([["b.ts", "hello"]]);

    await vi.advanceTimersByTimeAsync(5000);
    expect(saved).toEqual([["b.ts", "hello"]]); // 去抖计时已取消
  });

  it("flush 无待保存内容时为空操作", async () => {
    const saved: Array<[string, string]> = [];
    const saver = createAutoSaver(async (p, c) => void saved.push([p, c]), 1000);
    await saver.flush("none.ts");
    expect(saved).toEqual([]);
  });

  it("写盘失败不抛出、内容不丢：下次编辑重新调度", async () => {
    let fail = true;
    const saved: Array<[string, string]> = [];
    const saver = createAutoSaver(async (p, c) => {
      if (fail) throw new Error("disk full");
      saved.push([p, c]);
    }, 500);

    saver.schedule("c.ts", "v1");
    await vi.advanceTimersByTimeAsync(500); // 失败被吞掉
    expect(saved).toEqual([]);
    expect(saver.pending("c.ts")).toBe(false);

    fail = false;
    saver.schedule("c.ts", "v2"); // 用户继续编辑 → 重试
    await vi.advanceTimersByTimeAsync(500);
    expect(saved).toEqual([["c.ts", "v2"]]);
    expect(saver.pending("c.ts")).toBe(false);
    saver.dispose();
  });

  it("dispose 取消全部计时器", async () => {
    const saved: Array<[string, string]> = [];
    const saver = createAutoSaver(async (p, c) => void saved.push([p, c]), 1000);
    saver.schedule("d.ts", "x");
    saver.schedule("e.ts", "y");
    saver.dispose();
    await vi.advanceTimersByTimeAsync(5000);
    expect(saved).toEqual([]);
    expect(saver.pending("d.ts")).toBe(false);
  });
});
