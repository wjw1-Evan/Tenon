import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  coldStartStatus,
  markWorkspaceInputReady,
  percentile,
  recordWorkspaceInputReady,
  resetColdStartTelemetry,
} from "../lib/performance";

describe("cold start telemetry", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    const backing = new Map<string, string>();
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
      getItem: (key: string) => backing.get(key) ?? null,
      setItem: (key: string, value: string) => void backing.set(key, value),
      removeItem: (key: string) => void backing.delete(key),
      clear: () => backing.clear(),
      },
    });
    delete window.__TENON_COLD_START__;
    resetColdStartTelemetry();
  });

  it("keeps a bounded local sample set and reports the upper median", () => {
    for (const duration of [900, 120, 400, 1_800, 250, 700, 100]) {
      recordWorkspaceInputReady(duration);
    }
    const status = coldStartStatus();
    expect(status.samples).toHaveLength(7);
    expect(percentile(status.samples!.map((sample) => sample.duration_ms), 0.5)).toBe(400);
    expect(status.duration_ms).toBe(100);
    expect(window.__TENON_COLD_START__).toEqual(status);
  });

  it("marks input ready after the second frame using the navigation clock", async () => {
    let now = 100;
    vi.stubGlobal("performance", { now: () => now });
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      now += 25;
      callback(now);
      return now;
    });
    markWorkspaceInputReady();
    await new Promise((resolve) => setTimeout(resolve, 0));
    const status = window.__TENON_COLD_START__;
    expect(status?.ready).toBe(true);
    expect(status?.duration_ms).toBe(150);
    expect(coldStartStatus().samples).toHaveLength(1);
  });
});

import { recordCompletionPresentation, type CompletionPerfStatus } from "../lib/performance";

describe("completion telemetry", () => {
  it("keeps bounded local samples and reports upper median", () => {
    delete window.__TENON_COMPLETION_PERF__;
    localStorage.removeItem("tenon:performance:completion");
    for (const duration of [120, 40, 80, 20, 60]) {
      recordCompletionPresentation(duration);
    }
    const status = window.__TENON_COMPLETION_PERF__ as CompletionPerfStatus | undefined;
    expect(status?.samples).toHaveLength(5);
    expect(status?.p50_ms).toBe(60);
    expect(status?.latest_ms).toBe(60);
  });
});
