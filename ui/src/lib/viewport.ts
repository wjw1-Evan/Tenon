// 视口自适应（§7.2 v1.74）：档位判定与尺寸 clamp 纯函数 + resize 订阅。
// clamp 只作用于渲染，不回写用户记忆尺寸与项目 ui-state（§7.2 项目状态持久化）。
import { useEffect, useState } from "react";

export type ViewportBand = "wide" | "middle" | "narrow";

/** 档位边界：narrow < 800 ≤ middle < 1180 ≤ wide */
export const NARROW_MAX = 800;
export const MIDDLE_MIN = 1180;

export function bandOf(width: number): ViewportBand {
  if (width < NARROW_MAX) return "narrow";
  if (width < MIDDLE_MIN) return "middle";
  return "wide";
}

/** 左栏渲染宽：middle/narrow 收敛至 ≤24vw，宽屏原样。 */
export function effectiveLeft(width: number, vw: number): number {
  if (vw >= MIDDLE_MIN) return width;
  return Math.min(width, Math.max(160, Math.round(vw * 0.24)));
}

/** 右栏渲染宽：middle/narrow 收敛至 ≤34vw，宽屏原样。 */
export function effectiveRight(width: number, vw: number): number {
  if (vw >= MIDDLE_MIN) return width;
  return Math.min(width, Math.max(280, Math.round(vw * 0.34)));
}

/** 底栏渲染高：middle/narrow 收敛至 ≤40vh，宽屏原样。 */
export function effectiveBottom(height: number, vh: number): number {
  if (vh >= MIDDLE_MIN) return height;
  return Math.min(height, Math.max(100, Math.round(vh * 0.4)));
}

/** 浮层渲染宽（narrow）：用户记忆宽与 82vw 取小，下限 220px。 */
export function effectiveFloatWidth(width: number, vw: number): number {
  return Math.min(Math.max(width, 220), Math.round(vw * 0.82));
}

/** 订阅视口尺寸；jsdom 测试经 mock innerWidth/innerHeight + resize 事件驱动。 */
export function useViewport(): { width: number; height: number; band: ViewportBand } {
  const [size, setSize] = useState(() => ({
    width: typeof window === "undefined" ? 1440 : window.innerWidth,
    height: typeof window === "undefined" ? 900 : window.innerHeight,
  }));
  useEffect(() => {
    const onResize = () =>
      setSize({ width: window.innerWidth, height: window.innerHeight });
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return { ...size, band: bandOf(size.width) };
}
