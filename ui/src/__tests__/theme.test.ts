// 外观档（§7.5）：解析 / 持久化 / 应用到文档根 / 跟随系统监听。
// 测试环境无 localStorage（jsdom 全局被遮蔽）——用 Map 桩替换。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  applyTheme,
  isThemePreference,
  loadThemePreference,
  resolveTheme,
  saveThemePreference,
  THEME_STORAGE_KEY,
  watchSystemTheme,
} from "../lib/theme";

function stubStorage() {
  const backing = new Map<string, string>();
  const store = {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  };
  vi.stubGlobal("localStorage", store);
  return store;
}

describe("theme", () => {
  beforeEach(() => {
    stubStorage();
    document.documentElement.removeAttribute("data-theme");
  });
  afterEach(() => vi.unstubAllGlobals());

  it("resolveTheme: system 跟随系统，显式档直通", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
    expect(resolveTheme("light", true)).toBe("light");
  });

  it("偏好持久化：存取往返，非法值与缺失回退 system", () => {
    expect(loadThemePreference()).toBe("system");
    saveThemePreference("light");
    expect(loadThemePreference()).toBe("light");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("light");
    localStorage.setItem(THEME_STORAGE_KEY, "neon");
    expect(loadThemePreference()).toBe("system");
  });

  it("isThemePreference 仅接受三档", () => {
    expect(isThemePreference("dark")).toBe(true);
    expect(isThemePreference("system")).toBe(true);
    expect(isThemePreference("light")).toBe(true);
    expect(isThemePreference("blue")).toBe(false);
    expect(isThemePreference(null)).toBe(false);
  });

  it("applyTheme 写入 data-theme 并返回解析结果", () => {
    saveThemePreference("light");
    expect(applyTheme(loadThemePreference())).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
    saveThemePreference("dark");
    applyTheme(loadThemePreference());
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("watchSystemTheme：系统切换触发回调，退订后不再触发", () => {
    const listeners: Array<(e: MediaQueryListEvent) => void> = [];
    const mq = {
      addEventListener: (_: string, cb: (e: MediaQueryListEvent) => void) =>
        listeners.push(cb),
      removeEventListener: (_: string, cb: (e: MediaQueryListEvent) => void) => {
        const i = listeners.indexOf(cb);
        if (i >= 0) listeners.splice(i, 1);
      },
    } as unknown as MediaQueryList;
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue(mq));

    const seen: boolean[] = [];
    const unwatch = watchSystemTheme((dark) => seen.push(dark));
    const ev = { matches: false } as unknown as MediaQueryListEvent;
    listeners.forEach((cb) => cb(ev));
    expect(seen).toEqual([false]);
    unwatch();
    listeners.forEach((cb) => cb(ev));
    expect(seen).toEqual([false]);
  });
});
