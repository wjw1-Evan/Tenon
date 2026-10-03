// i18n（Q5）：英文为源语言（source of truth），中文一级翻译；
// 默认跟随系统，可手动切换；文案全部外置（§4.2）。
import en from "../locales/en.json";
import zh from "../locales/zh.json";

export type Locale = "auto" | "en" | "zh-CN";

const RESOURCES: Record<string, Record<string, string>> = {
  en: en as Record<string, string>,
  "zh-CN": zh as Record<string, string>,
};

export function resolveLocale(preference: Locale): string {
  if (preference !== "auto") return preference;
  if (typeof navigator === "undefined") return "en";
  const lang = navigator.language || "en";
  return lang.toLowerCase().startsWith("zh") ? "zh-CN" : "en";
}

export type Translate = (key: string, vars?: Record<string, string | number>) => string;

export function createTranslator(preference: Locale): Translate {
  const locale = resolveLocale(preference);
  const table = RESOURCES[locale] ?? RESOURCES.en;
  return (key, vars) => {
    let text = table[key] ?? (RESOURCES.en as Record<string, string>)[key] ?? key;
    if (vars) {
      for (const [k, v] of Object.entries(vars)) {
        text = text.replaceAll(`{${k}}`, String(v));
      }
    }
    return text;
  };
}

/** 语言切换事件（设置面板派发，App 订阅重渲染）。 */
export const LOCALE_CHANGE = "tenon:locale-change";
