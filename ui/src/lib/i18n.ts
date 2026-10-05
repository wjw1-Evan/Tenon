// i18n（Q5 / v1.100）：英文为源语言（source of truth），中文一级翻译静态随包（首屏即需），
// 社区语言包（繁體中文 / 日本語 / 한국어）按需懒加载；默认跟随系统，可手动切换；
// 文案全部外置（§4.2）。新增语言 = locales/<tag>.json（键与 en 对齐，测试门禁）+ LOCALES 注册一行。
import { useEffect, useMemo, useState } from "react";
import en from "../locales/en.json";
import zh from "../locales/zh.json";

export type Locale = "auto" | "en" | "zh-CN" | "zh-TW" | "ja" | "ko";

/** 语言注册表：顶栏选择器按此渲染（nativeName 不经 t()，语言名以母语显示是 i18n 惯例）。 */
export const LOCALES: ReadonlyArray<{ tag: Exclude<Locale, "auto">; nativeName: string }> = [
  { tag: "en", nativeName: "English" },
  { tag: "zh-CN", nativeName: "简体中文" },
  { tag: "zh-TW", nativeName: "繁體中文" },
  { tag: "ja", nativeName: "日本語" },
  { tag: "ko", nativeName: "한국어" },
];

export function isLocale(value: unknown): value is Locale {
  return value === "auto" || LOCALES.some((l) => l.tag === value);
}

/** 静态随包资源（en / zh-CN）；其余语言经 LOADER 懒加载后并入 RESOURCES。 */
const STATIC: Record<string, Record<string, string>> = {
  en: en as Record<string, string>,
  "zh-CN": zh as Record<string, string>,
};

const LOADER: Partial<Record<string, () => Promise<{ default: Record<string, string> }>>> = {
  "zh-TW": () => import("../locales/zh-TW.json"),
  ja: () => import("../locales/ja.json"),
  ko: () => import("../locales/ko.json"),
};

const RESOURCES: Record<string, Record<string, string>> = { ...STATIC };

export const LOCALE_STORAGE_KEY = "tenon:locale";

export function readLocalePreference(): Locale {
  try {
    const raw = localStorage.getItem(LOCALE_STORAGE_KEY);
    return isLocale(raw) ? raw : "auto";
  } catch {
    return "auto";
  }
}

export function saveLocalePreference(preference: Locale): void {
  try {
    localStorage.setItem(LOCALE_STORAGE_KEY, preference);
  } catch {
    // 存储不可用：仅当前会话生效
  }
}

/** auto → 系统语言：繁体区（zh-Hant / TW / HK / MO）→ zh-TW，其余中文 → zh-CN，ja / ko 按前缀。 */
export function resolveLocale(preference: Locale): string {
  if (preference !== "auto") return preference;
  if (typeof navigator === "undefined") return "en";
  const lang = (navigator.language || "en").toLowerCase();
  if (lang.startsWith("zh")) {
    return /zh[_-](hant|tw|hk|mo)/.test(lang) ? "zh-TW" : "zh-CN";
  }
  if (lang.startsWith("ja")) return "ja";
  if (lang.startsWith("ko")) return "ko";
  return "en";
}

/** 懒加载语言资源；静态或已加载返回 false（无需重渲染），首次载入返回 true。幂等。 */
export async function loadLocaleResource(tag: string): Promise<boolean> {
  if (RESOURCES[tag]) return false;
  const loader = LOADER[tag];
  if (!loader) return false;
  const mod = await loader();
  RESOURCES[tag] = mod.default as Record<string, string>;
  return true;
}

export type Translate = (key: string, vars?: Record<string, string | number>) => string;

export function createTranslator(preference: Locale): Translate {
  const locale = resolveLocale(preference);
  const table = RESOURCES[locale] ?? RESOURCES.en;
  return (key, vars) => {
    let text = table[key] ?? RESOURCES.en[key] ?? key;
    if (vars) {
      for (const [k, v] of Object.entries(vars)) {
        text = text.replaceAll(`{${k}}`, String(v));
      }
    }
    return text;
  };
}

/** App 级翻译器：懒加载语言首次就绪后触发重渲染完成切换（此前优雅回退英文）。 */
export function useLocaleTranslator(preference: Locale): Translate {
  const [loadedTick, setLoadedTick] = useState(0);
  const resolved = resolveLocale(preference);
  useEffect(() => {
    let alive = true;
    void loadLocaleResource(resolved).then((loaded) => {
      if (loaded && alive) setLoadedTick((n) => n + 1);
    });
    return () => {
      alive = false;
    };
  }, [resolved]);
  return useMemo(() => createTranslator(preference), [preference, loadedTick]);
}

/** 语言切换事件（顶栏 / 设置面板派发，App 订阅重渲染）。 */
export const LOCALE_CHANGE = "tenon:locale-change";

/** 解析后的 BCP-47 语言标签（"en" | "zh-CN" | "zh-TW" | "ja" | "ko"），供日期时间本地化（§4.2）。
 *  与 App 同源：localStorage 初值 + LOCALE_CHANGE 订阅，应用内切换语言时时间格式随之切换。 */
export function useResolvedLocale(): string {
  const [preference, setPreference] = useState<Locale>(readLocalePreference);
  useEffect(() => {
    const onLocaleChange = (event: Event) => setPreference((event as CustomEvent).detail as Locale);
    window.addEventListener(LOCALE_CHANGE, onLocaleChange);
    return () => window.removeEventListener(LOCALE_CHANGE, onLocaleChange);
  }, []);
  return resolveLocale(preference);
}
