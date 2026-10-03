// AI Evals 报告可视化（设计方案 §18.3 / M3）：
// 五指标（通过率 / 成本 / 步数 / 审批数 / 安全违规）+ 对比。
import { useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";

interface EvalRun {
  id: string;
  target: string;
  metrics_json: Record<string, unknown>;
  verdict: string;
  created_at: string;
}

export function EvalsPanel({ api }: { api: TenonApi }) {
  const [runs, setRuns] = useState<EvalRun[]>([]);

  useEffect(() => {
    api.evalRuns().then((r) => setRuns(r.runs ?? [])).catch(() => {});
  }, [api]);

  return (
    <div className="evals-panel" data-testid="evals-panel">
      <div className="evals-head">AI Evals 报告（本地生成）</div>
      {runs.length === 0 && <div className="muted">暂无报告（tenon-evals 运行后生成）</div>}
      <table>
        <thead>
          <tr>
            <th>target</th>
            <th>时间</th>
            <th>判定</th>
            <th>通过率</th>
            <th>tokens</th>
            <th>steps</th>
            <th>审批</th>
            <th>违规</th>
          </tr>
        </thead>
        <tbody>
          {runs.map((r) => {
            const fm = (r.metrics_json as { five_metrics?: Record<string, number> })
              .five_metrics;
            return (
              <tr key={r.id} data-verdict={r.verdict}>
                <td>{r.target}</td>
                <td>{r.created_at.slice(0, 19)}</td>
                <td>{r.verdict}</td>
                <td>{fm ? `${Math.round((fm.pass_rate ?? 0) * 100)}%` : "—"}</td>
                <td>{fm?.total_tokens ?? "—"}</td>
                <td>{fm?.total_steps ?? "—"}</td>
                <td>{fm?.total_approvals ?? "—"}</td>
                <td>{fm?.security_violations ?? "—"}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
