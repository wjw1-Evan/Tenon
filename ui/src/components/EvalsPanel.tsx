// AI Evals 报告可视化（设计方案 §18.3 / M3）：
// 基线指标（通过率 / 成本 / 步数 / 风险动作数 / 安全违规）+ L4 上下文质量。
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
            <th>风险动作</th>
            <th>违规</th>
            <th>L4 命中</th>
            <th>L4 均分</th>
          </tr>
        </thead>
        <tbody>
          {runs.map((r) => {
            // tenon-evals 落盘文件使用 five_metrics 嵌套；daemon store 报告为顶层。
            const metrics = r.metrics_json as {
              five_metrics?: Record<string, number>;
              pass_rate?: number;
              total_tokens?: number;
              total_steps?: number;
              total_risk_actions?: number;
              security_violations?: number;
              l4_recall_hit_rate?: number;
              l4_average_score?: number;
            };
            const fm = metrics.five_metrics ?? metrics;
            return (
              <tr key={r.id} data-verdict={r.verdict}>
                <td>{r.target}</td>
                <td>{r.created_at.slice(0, 19)}</td>
                <td>{r.verdict}</td>
                <td>{fm ? `${Math.round((fm.pass_rate ?? 0) * 100)}%` : "—"}</td>
                <td>{fm?.total_tokens ?? "—"}</td>
                <td>{fm?.total_steps ?? "—"}</td>
                <td>{fm?.total_risk_actions ?? "—"}</td>
                <td>{fm?.security_violations ?? "—"}</td>
                <td data-testid={`evals-l4-hit-${r.id}`}>
                  {typeof fm?.l4_recall_hit_rate === "number"
                    ? `${Math.round(fm.l4_recall_hit_rate * 100)}%`
                    : "—"}
                </td>
                <td>
                  {typeof fm?.l4_average_score === "number"
                    ? fm.l4_average_score.toFixed(3)
                    : "—"}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
