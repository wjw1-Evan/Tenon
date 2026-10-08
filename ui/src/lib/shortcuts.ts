// 快捷键速查表数据（§7.4 / §7.5 v1.167）：组合键显示的单源清单——
// hooks.ts 的按键匹配与此表各自维护（匹配是命令式判定），但组合与人读标签
// 以此表为准；新增全局快捷键必须同步两处 + 本表。

export interface ShortcutSpec {
  /** 稳定 id（测试锚点）。 */
  id: string;
  /** 组合键规范形：mod/shift/alt 修饰 + 主键（小写），如 "mod+shift+p"、"escape"、"mod+/"。 */
  keys: string;
  /** i18n 文案键。 */
  labelKey: string;
}

export interface ShortcutGroupSpec {
  titleKey: string;
  items: ShortcutSpec[];
}

export const SHORTCUT_GROUPS: readonly ShortcutGroupSpec[] = [
  {
    titleKey: "shortcuts.group_global",
    items: [
      { id: "palette", keys: "mod+shift+p", labelKey: "shortcuts.palette" },
      { id: "goto", keys: "mod+p", labelKey: "shortcuts.goto" },
      { id: "settings", keys: "mod+,", labelKey: "shortcuts.settings" },
      { id: "sidebar", keys: "mod+b", labelKey: "shortcuts.sidebar" },
      { id: "panel", keys: "mod+j", labelKey: "shortcuts.panel" },
      { id: "panel_cycle", keys: "mod+alt+j", labelKey: "shortcuts.panel_cycle" },
      { id: "cheatsheet", keys: "mod+/", labelKey: "shortcuts.cheatsheet" },
    ],
  },
  {
    titleKey: "shortcuts.group_task",
    items: [
      { id: "new_task", keys: "mod+alt+n", labelKey: "shortcuts.new_task" },
      { id: "send", keys: "mod+enter", labelKey: "shortcuts.send" },
      { id: "pause", keys: "escape", labelKey: "shortcuts.pause" },
      { id: "stop", keys: "mod+.", labelKey: "shortcuts.stop" },
    ],
  },
  {
    titleKey: "shortcuts.group_editor",
    items: [
      { id: "inline", keys: "mod+i", labelKey: "shortcuts.inline" },
      { id: "save", keys: "mod+s", labelKey: "shortcuts.save" },
      { id: "close_tab", keys: "mod+alt+w", labelKey: "shortcuts.close_tab" },
    ],
  },
];

const IS_MAC =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent);

const MAC_SYMBOLS: Record<string, string> = {
  mod: "⌘",
  shift: "⇧",
  alt: "⌥",
};

const KEY_NAMES: Record<string, string> = {
  escape: "Esc",
  enter: "Enter",
};

/** 组合键规范形 → 平台显示：mac "mod+shift+p" → "⌘⇧P"；其余平台 → "Ctrl+Shift+P"。 */
export function formatCombo(keys: string): string {
  const parts = keys.split("+");
  const key = parts.pop() ?? "";
  const keyName = KEY_NAMES[key] ?? (key.length === 1 ? key.toUpperCase() : key);
  if (IS_MAC) {
    const mods = parts.map((part) => MAC_SYMBOLS[part] ?? part).join("");
    return `${mods}${keyName}`;
  }
  const mods = parts
    .map((part) => (part === "mod" ? "Ctrl" : part === "alt" ? "Alt" : "Shift"))
    .join("+");
  return mods ? `${mods}+${keyName}` : keyName;
}
