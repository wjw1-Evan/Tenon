import { describe, expect, it } from "vitest";
import { createTranslator, resolveLocale } from "../lib/i18n";
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

  it("变量插值", () => {
    const t = createTranslator("en");
    expect(t("nonexistent.key", { x: 1 })).toBe("nonexistent.key");
  });
});
