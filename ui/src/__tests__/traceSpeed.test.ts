import { describe, expect, it } from "vitest";

// 从 AgentTracePanel 提取的纯计算逻辑
function computeSpeed(
  prev: { out: number; inp: number; at: number } | null,
  cur: { out: number; inp: number; at: number },
): { outSpeed: number | null; inpSpeed: number | null } {
  if (!prev || cur.at <= prev.at) return { outSpeed: null, inpSpeed: null };
  const dt = (cur.at - prev.at) / 1000;
  const dOut = cur.out - prev.out;
  const dInp = cur.inp - prev.inp;
  return {
    outSpeed: dOut > 0 ? +(dOut / dt).toFixed(1) : dOut < 0 ? null : 0,
    inpSpeed: dInp > 0 ? +(dInp / dt).toFixed(1) : dInp < 0 ? null : 0,
  };
}

describe("token/s 速度计算", () => {
  it("正常速率：增量/时间差（performance.now 毫秒）", () => {
    const r = computeSpeed({ out: 0, inp: 0, at: 0 }, { out: 30, inp: 60, at: 1500 });
    expect(r.outSpeed).toBe(20); // 30 tok / 1.5s = 20 tok/s
    expect(r.inpSpeed).toBe(40);
  });

  it("无增量 → 速度 0", () => {
    const r = computeSpeed({ out: 50, inp: 50, at: 0 }, { out: 50, inp: 50, at: 1500 });
    expect(r.outSpeed).toBe(0);
    expect(r.inpSpeed).toBe(0);
  });

  it("首次采样（无前值）→ null", () => {
    expect(computeSpeed(null, { out: 10, inp: 10, at: 1000 }).outSpeed).toBeNull();
  });

  it("时间倒退 → null（防御）", () => {
    const r = computeSpeed({ out: 10, inp: 10, at: 5000 }, { out: 20, inp: 20, at: 3000 });
    expect(r.outSpeed).toBeNull();
  });
});
