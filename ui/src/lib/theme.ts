// 外观档（§7.5）：深色（默认）/ 浅色 / 跟随系统；
// localStorage 记忆 + data-theme 属性驱动 CSS 变量换色。
export type ThemePreference = "system" | "dark" | "light";
export type ResolvedTheme = "dark" | "light";

export const THEME_STORAGE_KEY = "tenon:theme";
/** 外观切换事件（设置控件派发，需要联动处订阅）。 */
export const THEME_CHANGE = "tenon:theme-change";

const VALID: ThemePreference[] = ["system", "dark", "light"];

export function isThemePreference(v: unknown): v is ThemePreference {
  return typeof v === "string" && (VALID as string[]).includes(v);
}

export function loadThemePreference(): ThemePreference {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    return isThemePreference(raw) ? raw : "system";
  } catch {
    return "system";
  }
}

export function saveThemePreference(p: ThemePreference): void {
  try {
    localStorage.setItem(THEME_STORAGE_KEY, p);
  } catch {
    // 存储不可用（隐私模式等）：仅当前会话生效
  }
}

export function resolveTheme(p: ThemePreference, systemDark: boolean): ResolvedTheme {
  if (p === "system") return systemDark ? "dark" : "light";
  return p;
}

export function systemPrefersDark(): boolean {
  return typeof matchMedia === "function"
    ? matchMedia("(prefers-color-scheme: dark)").matches
    : false;
}

/** 应用外观到文档根（data-theme + color-scheme），返回解析后的主题。 */
export function applyTheme(p: ThemePreference): ResolvedTheme {
  const resolved = resolveTheme(p, systemPrefersDark());
  if (typeof document !== "undefined") {
    document.documentElement.dataset.theme = resolved;
  }
  return resolved;
}

/** 跟随系统档：监听系统深浅切换；返回取消监听函数。 */
export function watchSystemTheme(cb: (systemDark: boolean) => void): () => void {
  if (typeof matchMedia !== "function") return () => {};
  const mq = matchMedia("(prefers-color-scheme: dark)");
  const onChange = (e: MediaQueryListEvent) => cb(e.matches);
  mq.addEventListener("change", onChange);
  return () => mq.removeEventListener("change", onChange);
}
