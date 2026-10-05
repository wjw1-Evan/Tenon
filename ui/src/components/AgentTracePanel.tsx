// AgentTrace 面板（设计方案 §14.2 / M2 交付）：
// 工具调用明细 / token 与成本 / 风险动作 / token 速度——本地生成、全可查。
import { useEffect, useMemo, useRef, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface TraceEvent {
  id: number;
  seq: number;
  type: string;
  payload: Record<string, unknown>;
}

interface Props {
  api: TenonApi;
  sessionId: string | null;
  t: Translate;
}

/// token 速度采样（两次测量窗口间的增量 / 时间差）。
interface SpeedSample {
  prevOut: number;
  prevInp: number;
  prevAt: number; // performance.now()
}

export function AgentTracePanel({ api, sessionId, t }: Props) {
  const [events, setEvents] = useState<TraceEvent[]>([]);
  const [tokens, setTokens] = useState<{ inp: number; out: number; cost: number }>({
    inp: 0,
    out: 0,
    cost: 0,
  });
  /// 输出 token/s（滚动窗口采样）
  const [outSpeed, setOutSpeed] = useState<number | null>(null);
  /// 输入 token/s
  const [inpSpeed, setInpSpeed] = useState<number | null>(null);
  const speedRef = useRef<SpeedSample | null>(null);

  useEffect(() => {
    if (!sessionId) return;
    let alive = true;
    const load = async () => {
      try {
        const trace = await api.trace(sessionId);
        const costs = await api.costs(sessionId);
        if (!alive) return;
        setEvents((trace.events ?? []) as TraceEvent[]);
        const inp = Number(costs.input_tokens ?? 0);
        const out = Number(costs.output_tokens ?? 0);
        setTokens({ inp, out, cost: Number(costs.cost_usd ?? 0) });

        // token/s：每次轮询计算增量速率（1.5s 窗口）
        const now = performance.now();
        const prev = speedRef.current;
        if (prev && now > prev.prevAt) {
          const dt = (now - prev.prevAt) / 1000;
          const dOut = out - prev.prevOut;
          const dInp = inp - prev.prevInp;
          if (dt > 0) {
            setOutSpeed(dOut > 0 ? +(dOut / dt).toFixed(1) : 0);
            setInpSpeed(dInp > 0 ? +(dInp / dt).toFixed(1) : 0);
          }
        }
        speedRef.current = { prevOut: out, prevInp: inp, prevAt: now };
      } catch {
        // 断线重试
      }
    };
    load();
    const timer = setInterval(load, 1500);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [api, sessionId]);

  const toolCalls = useMemo(
    () =>
      events
        .filter((e) => ["patch_applied", "command_run"].includes(e.type))
        .map((e) => ({
          tool: String((e.payload as { tool?: string }).tool ?? ""),
          ok: Boolean((e.payload as { output?: { ok?: boolean } }).output?.ok ?? true),
        })),
    [events]
  );
  const riskActions = events.filter((e) => e.type === "direct_action");

  return (
    <div className="trace-panel" data-testid="agent-trace">
      <div className="trace-head">{t("trace.title")}</div>
      <div className="trace-metrics" data-testid="trace-metrics">
        <span>{t("trace.tokens_in")} {tokens.inp} tok</span>
        <span>{t("trace.tokens_out")} {tokens.out} tok</span>
        {inpSpeed !== null && inpSpeed > 0 && (
          <span data-testid="inp-speed">{inpSpeed} tok/s ↑</span>
        )}
        {outSpeed !== null && outSpeed > 0 && (
          <span data-testid="out-speed">{outSpeed} tok/s ↓</span>
        )}
        <span>{t("trace.cost")} ${tokens.cost.toFixed(4)}</span>
        <span>{t("trace.risk_actions")} {riskActions.length}</span>
      </div>
      <table className="trace-table">
        <thead>
          <tr>
            <th>seq</th>
            <th>{t("trace.col_type")}</th>
            <th>{t("trace.col_detail")}</th>
          </tr>
        </thead>
        <tbody>
          {events.map((e) => (
            <tr key={e.id} className={`trace-${e.type}`}>
              <td>{e.seq}</td>
              <td>{e.type}</td>
              <td className="trace-detail">
                {e.type === "patch_applied" || e.type === "command_run"
                  ? String((e.payload as { tool?: string }).tool ?? "")
                  : JSON.stringify(e.payload).slice(0, 80)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
