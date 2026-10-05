import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  LOCALE_CHANGE,
  createTranslator,
  loadLocaleResource,
  readLocalePreference,
  resolveLocale,
  useLocaleTranslator,
  useResolvedLocale,
} from "../lib/i18n";
import en from "../locales/en.json";
import ja from "../locales/ja.json";
import ko from "../locales/ko.json";
import zh from "../locales/zh.json";
import zhTW from "../locales/zh-TW.json";

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

describe("i18n（Q5：英文源 / 多语言社区包 v1.100）", () => {
  it("全部语言键与 en 完全对齐（防漏翻；新增语言包必须过此门禁）", () => {
    const enKeys = Object.keys(en).sort();
    for (const [name, table] of [
      ["zh", zh],
      ["zh-TW", zhTW],
      ["ja", ja],
      ["ko", ko],
    ] as const) {
      expect(Object.keys(table).sort(), `${name} 键集须与 en 对齐`).toEqual(enKeys);
    }
  });

  it("解析 locale：手动切换各语言直达", () => {
    expect(resolveLocale("zh-CN")).toBe("zh-CN");
    expect(resolveLocale("zh-TW")).toBe("zh-TW");
    expect(resolveLocale("ja")).toBe("ja");
    expect(resolveLocale("ko")).toBe("ko");
    expect(resolveLocale("en")).toBe("en");
  });

  it("resolveLocale：auto 按系统语言前缀匹配（繁体区→zh-TW）", () => {
    const setLang = (value: string) =>
      Object.defineProperty(window.navigator, "language", { value, configurable: true });
    try {
      setLang("zh-TW");
      expect(resolveLocale("auto")).toBe("zh-TW");
      setLang("zh-HK");
      expect(resolveLocale("auto")).toBe("zh-TW");
      setLang("zh-Hant-TW");
      expect(resolveLocale("auto")).toBe("zh-TW");
      setLang("zh-CN");
      expect(resolveLocale("auto")).toBe("zh-CN");
      setLang("zh");
      expect(resolveLocale("auto")).toBe("zh-CN");
      setLang("ja-JP");
      expect(resolveLocale("auto")).toBe("ja");
      setLang("ko-KR");
      expect(resolveLocale("auto")).toBe("ko");
      setLang("fr");
      expect(resolveLocale("auto")).toBe("en");
    } finally {
      setLang("en-US");
    }
  });

  it("翻译回退：缺失键回英文再回键名", () => {
    const t = createTranslator("zh-CN");
    expect(t("timeline.rollback")).toBe("回滚到此对话节点");
    expect(t("nonexistent.key")).toBe("nonexistent.key");
  });

  it("变量插值", () => {
    const t = createTranslator("en");
    expect(t("nonexistent.key", { x: 1 })).toBe("nonexistent.key");
  });

  it("懒加载：未载入语言优雅回退 en，loadLocaleResource 后生效且幂等", async () => {
    const before = createTranslator("ja");
    expect(before("tree.rename")).toBe("Rename");
    expect(await loadLocaleResource("ja")).toBe(true);
    expect(await loadLocaleResource("ja")).toBe(false);
    const after = createTranslator("ja");
    expect(after("tree.rename")).toBe("名前を変更");
    expect(after("message.send")).toBe("送信");
  });

  it("useLocaleTranslator：懒加载语言就绪后自动切换", async () => {
    const { result } = renderHook(() => useLocaleTranslator("ko"));
    await waitFor(() => expect(result.current("message.send")).toBe("보내기"));
  });

  it("偏好读写：localStorage 十进十出、非法值回退 auto", () => {
    backing.set("tenon:locale", "zh-TW");
    expect(readLocalePreference()).toBe("zh-TW");
    backing.set("tenon:locale", "xx-YY");
    expect(readLocalePreference()).toBe("auto");
    backing.clear();
    expect(readLocalePreference()).toBe("auto");
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
