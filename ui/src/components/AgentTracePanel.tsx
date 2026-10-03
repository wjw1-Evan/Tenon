// AgentTrace 面板（设计方案 §14.2 / M2 交付）：
// 工具调用明细 / token 与成本 / 审批记录——本地生成、全可查。
import { useEffect, useMemo, useState } from "react";
import type { TenonApi } from "../lib/api";

interface TraceEvent {
  id: number;
  seq: number;
  type: string;
  payload: Record<string, unknown>;
}

interface Props {
  api: TenonApi;
  sessionId: string | null;
}

export function AgentTracePanel({ api, sessionId }: Props) {
  const [events, setEvents] = useState<TraceEvent[]>([]);
  const [tokens, setTokens] = useState<{ inp: number; out: number; cost: number }>({
    inp: 0,
    out: 0,
    cost: 0,
  });

  useEffect(() => {
    if (!sessionId) return;
    let alive = true;
    const load = async () => {
      try {
        const trace = await api.trace(sessionId);
        const costs = await api.costs(sessionId);
        if (!alive) return;
        setEvents((trace.events ?? []) as TraceEvent[]);
        setTokens({
          inp: Number(costs.input_tokens ?? 0),
          out: Number(costs.output_tokens ?? 0),
          cost: Number(costs.cost_usd ?? 0),
        });
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
  const approvals = events.filter((e) => e.type === "approval_request");

  return (
    <div className="trace-panel" data-testid="agent-trace">
      <div className="trace-head">AgentTrace（本地审计）</div>
      <div className="trace-metrics" data-testid="trace-metrics">
        <span>输入 {tokens.inp} tok</span>
        <span>输出 {tokens.out} tok</span>
        <span>成本 ${tokens.cost.toFixed(4)}</span>
        <span>审批 {approvals.length} 次</span>
      </div>
      <table className="trace-table">
        <thead>
          <tr>
            <th>seq</th>
            <th>类型</th>
            <th>详情</th>
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
