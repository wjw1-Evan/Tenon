// performance.ts / theme.ts 补全测试。
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  percentile,
  recordWorkspaceInputReady,
  resetColdStartTelemetry,
  coldStartStatus,
  markWorkspaceInputReady,
  recordCompletionPresentation,
  resetCompletionTelemetry,
} from "../lib/performance";
import {
  isThemePreference,
  loadThemePreference,
  saveThemePreference,
  resolveTheme,
  applyTheme,
  systemPrefersDark,
  watchSystemTheme,
} from "../lib/theme";

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });
  resetColdStartTelemetry();
  resetCompletionTelemetry();
});

describe("performance deep", () => {
  it("percentile handles edge cases", () => {
    expect(percentile([], 0.5)).toBeUndefined();
    expect(percentile([NaN, Infinity], 0.5)).toBeUndefined();
    expect(percentile([10], 0.5)).toBe(10);
    expect(percentile([1, 2, 3, 4, 5], 0.5)).toBe(3);
    expect(percentile([4, 1, 3, 2], 0.5)).toBe(2);
  });

  it("recordWorkspaceInputReady stores and caps at limit", () => {
    for (let i = 0; i < 15; i++) {
      recordWorkspaceInputReady(i * 10);
    }
    const status = coldStartStatus();
    expect(status.samples!.length).toBe(10);
    expect(status.ready).toBe(true);
    expect(status.duration_ms).toBe(140);
  });

  it("recordWorkspaceInputReady handles negative and non-finite", () => {
    recordWorkspaceInputReady(-5);
    expect(coldStartStatus().duration_ms).toBe(0);
    resetColdStartTelemetry();
    recordWorkspaceInputReady(NaN);
    expect(coldStartStatus().ready).toBe(true);
  });

  it("markWorkspaceInputReady is a no-op after ready", async () => {
    recordWorkspaceInputReady(100);
    const before = coldStartStatus().duration_ms;
    markWorkspaceInputReady();
    await new Promise(r => setTimeout(r, 50));
    expect(coldStartStatus().duration_ms).toBe(before);
  });

  it("coldStartStatus empty returns not ready", () => {
    expect(coldStartStatus().ready).toBe(false);
    expect(coldStartStatus().duration_ms).toBeUndefined();
  });

  it("recordCompletionPresentation tracks p50 and caps", () => {
    for (let i = 1; i <= 25; i++) {
      recordCompletionPresentation(i);
    }
    const status = window.__TENON_COMPLETION_PERF__;
    expect(status!.samples.length).toBe(20);
    expect(status!.latest_ms).toBe(25);
    expect(status!.p50_ms).toBe(15);
  });

  it("recordCompletionPresentation handles invalid input", () => {
    recordCompletionPresentation(-5);
    expect(window.__TENON_COMPLETION_PERF__!.latest_ms).toBe(0);
  });
});

describe("theme deep", () => {
  it("isThemePreference validates", () => {
    expect(isThemePreference("dark")).toBe(true);
    expect(isThemePreference("light")).toBe(true);
    expect(isThemePreference("system")).toBe(true);
    expect(isThemePreference("auto")).toBe(false);
    expect(isThemePreference(42)).toBe(false);
    expect(isThemePreference(null)).toBe(false);
  });

  it("loadThemePreference defaults to system on invalid", () => {
    expect(loadThemePreference()).toBe("system");
    localStorage.setItem("tenon:theme", "invalid");
    expect(loadThemePreference()).toBe("system");
    localStorage.setItem("tenon:theme", "light");
    expect(loadThemePreference()).toBe("light");
  });

  it("saveThemePreference persists", () => {
    saveThemePreference("dark");
    expect(loadThemePreference()).toBe("dark");
  });

  it("resolveTheme resolves system and explicit", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
    expect(resolveTheme("light", true)).toBe("light");
  });

  it("applyTheme sets document attribute", () => {
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({ matches: true }));
    const resolved = applyTheme("system");
    expect(resolved).toBe("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("systemPrefersDark falls back to false without matchMedia", () => {
    vi.stubGlobal("matchMedia", undefined);
    expect(systemPrefersDark()).toBe(false);
  });

  it("watchSystemTheme subscribes and unsubscribes", () => {
    const listeners: Array<(e: MediaQueryListEvent) => void> = [];
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({
      matches: false,
      addEventListener: (_: string, cb: (e: MediaQueryListEvent) => void) => listeners.push(cb),
      removeEventListener: (_: string, cb: (e: MediaQueryListEvent) => void) => {
        const idx = listeners.indexOf(cb);
        if (idx >= 0) listeners.splice(idx, 1);
      },
    }));
    const cb = vi.fn();
    const unwatch = watchSystemTheme(cb);
    expect(listeners.length).toBe(1);
    // 模拟系统切换
    listeners[0]({ matches: true } as MediaQueryListEvent);
    expect(cb).toHaveBeenCalledWith(true);
    unwatch();
    expect(listeners.length).toBe(0);
  });

  it("watchSystemTheme returns noop without matchMedia", () => {
    vi.stubGlobal("matchMedia", undefined);
    const unwatch = watchSystemTheme(vi.fn());
    expect(() => unwatch()).not.toThrow();
  });
});


describe("theme + performance final", () => {
  it("theme systemPrefersDark with mock matchMedia", () => {
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({ matches: true }));
    expect(systemPrefersDark()).toBe(true);
    vi.unstubAllGlobals();
  });

  it("theme applyTheme with light on system dark", () => {
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({ matches: true }));
    expect(applyTheme("light")).toBe("light");
    vi.unstubAllGlobals();
  });
});
