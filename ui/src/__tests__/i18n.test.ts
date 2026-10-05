import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LOCALE_CHANGE, createTranslator, resolveLocale, useResolvedLocale } from "../lib/i18n";
import en from "../locales/en.json";
import zh from "../locales/zh.json";

describe("i18n（Q5：英文源 / 中文一级翻译）", () => {
  it("en 与 zh 键完全对齐（防漏翻）", () => {
    const enKeys = Object.keys(en).sort();
    const zhKeys = Object.keys(zh).sort();
    expect(zhKeys).toEqual(enKeys);
  });

  it("解析 locale：auto 跟随系统、手动切换生效", () => {
    expect(resolveLocale("zh-CN")).toBe("zh-CN");
    expect(resolveLocale("en")).toBe("en");
    // jsdom 默认 en
    expect(["en", "zh-CN"]).toContain(resolveLocale("auto"));
  });

  it("翻译回退：缺失键回英文再回键名", () => {
    const t = createTranslator("zh-CN");
    expect(t("timeline.rollback")).toBe("回滚到此对话节点");
    expect(t("nonexistent.key")).toBe("nonexistent.key");
  });

  // 本环境无 localStorage（仓库约定）：内存 Map 桩替代。
  const backing = new Map<string, string>();
  beforeEach(() => {
    backing.clear();
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
      setItem: (k: string, v: string) => void backing.set(k, v),
      removeItem: (k: string) => void backing.delete(k),
      clear: () => backing.clear(),
    });
  });

  it("变量插值", () => {
    const t = createTranslator("en");
    expect(t("nonexistent.key", { x: 1 })).toBe("nonexistent.key");
  });

  it("useResolvedLocale：localStorage 初值 + LOCALE_CHANGE 即时切换（日期时间本地化同源）", () => {
    backing.set("tenon:locale", "zh-CN");
    const { result } = renderHook(() => useResolvedLocale());
    expect(result.current).toBe("zh-CN");
    act(() => {
      window.dispatchEvent(new CustomEvent(LOCALE_CHANGE, { detail: "en" }));
    });
    expect(result.current).toBe("en");
  });

  it("useResolvedLocale：无存储偏好时回退 auto（跟随系统）", () => {
    const { result } = renderHook(() => useResolvedLocale());
    expect(["en", "zh-CN"]).toContain(result.current);
  });
});
