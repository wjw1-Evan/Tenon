// feature flag 基建（v2.29，design.md §7.0 / design-v2.md §9「流程门禁」）：
// 新形态一律注册后挂载——值存 daemon ui_prefs `flag.<key>`（跨启动 / 跨端权威，
// 与 sound.done 同轨道），localStorage 快路径防闪烁；一键回退无需发版。
import { useCallback, useEffect, useState } from "react";

export interface FeatureFlagDef {
  key: string;
  default: boolean;
  desc: string;
}

/** 单一注册表：布局级 / 方向性新形态在此登记（流程门禁的挂载前提）。 */
export const FEATURE_FLAGS: readonly FeatureFlagDef[] = [
  {
    key: "reviewPane",
    default: true,
    desc: "常驻审查面（v2.5）——关闭回 v1 无审查面形态",
  },
];

const LS_PREFIX = "tenon:flag:";

/** 快路径初值（防闪烁）：localStorage 优先，缺省回注册表默认。 */
export function flagDefault(key: string): boolean {
  const def = FEATURE_FLAGS.find((f) => f.key === key);
  if (!def) return false;
  try {
    const raw = localStorage.getItem(LS_PREFIX + key);
    return raw === null ? def.default : raw === "on";
  } catch {
    return def.default;
  }
}

/** daemon 权威值解析（ui_prefs `flag.<key>`：\"on\"/\"off\"；缺席 = 默认）。 */
export function flagFromPrefs(
  key: string,
  prefs: Record<string, string>,
): boolean {
  const raw = prefs[`flag.${key}`];
  if (raw === "on") return true;
  if (raw === "off") return false;
  const def = FEATURE_FLAGS.find((f) => f.key === key);
  return def?.default ?? false;
}

export function flagToPrefsEntry(key: string, value: boolean) {
  return { [`flag.${key}`]: value ? "on" : "off" } as Record<string, string>;
}

export interface FeatureFlagsApi {
  /** 当前值（已融合 daemon 权威与本地快路径）。 */
  enabled: (key: string) => boolean;
  /** 开关并双写（daemon 权威 + localStorage 快路径）。 */
  toggle: (key: string) => void;
}

/** App 级 hook：`initial` 为启动时拉取的 ui_prefs 快照。 */
export function useFeatureFlags(
  initial: Record<string, string>,
  persist: (prefs: Record<string, string>) => void,
): FeatureFlagsApi {
  const [values, setValues] = useState<Record<string, boolean>>(() => {
    const out: Record<string, boolean> = {};
    for (const f of FEATURE_FLAGS) out[f.key] = flagFromPrefs(f.key, initial);
    return out;
  });
  // 忽略 persist 引用变化——toggle 闭包用最新 values
  useEffect(() => {}, [persist]);
  const enabled = useCallback(
    (key: string) => values[key] ?? flagDefault(key),
    [values],
  );
  const toggle = useCallback(
    (key: string) => {
      setValues((prev) => {
        const next = { ...prev, [key]: !(prev[key] ?? flagDefault(key)) };
        try {
          localStorage.setItem(
            LS_PREFIX + key,
            next[key] ? "on" : "off",
          );
        } catch {
          // 存储不可用：仅当前会话生效
        }
        persist(flagToPrefsEntry(key, next[key]));
        return next;
      });
    },
    [persist],
  );
  return { enabled, toggle };
}
