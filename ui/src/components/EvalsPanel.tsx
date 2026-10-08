// AI Evals 报告可视化（设计方案 §18.3 / M3）：
// 基线指标（通过率 / 成本 / 步数 / 风险动作数 / 安全违规）+ L4 上下文质量。
// v1.167 三态补齐：loading / 错误（inline + 重试）不再吞错；表格 caption + th scope。
import { useCallback, useEffect, useState } from "react";
import type { TenonApi } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { toast } from "../lib/toast";

interface EvalRun {
  id: string;
  target: string;
  metrics_json: Record<string, unknown>;
  verdict: string;
  created_at: string;
}

interface Props {
  api: TenonApi;
  t: Translate;
}

export function EvalsPanel({ api, t }: Props) {
  const [runs, setRuns] = useState<EvalRun[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    // 已有数据时的静默刷新不回 loading 态（避免重进 tab 闪烁）
    api
      .evalRuns()
      .then((r) => {
        setRuns(r.runs ?? []);
        setError(null);
      })
      .catch((e) => {
        setError(String(e));
        // 拉取失败（v1.167）：不再吞错——inline 错误态 + 重试钮，toast 同步提示
        toast.error(String(e));
      });
  }, [api]);

  useEffect(() => {
    load();
  }, [load]);

  return (
    <div className="evals-panel" data-testid="evals-panel">
      <div className="evals-head">{t("evals.title")}</div>
      {runs === null && !error && (
        <div className="muted" role="status" data-testid="evals-loading">
          {t("evals.loading")}
        </div>
      )}
      {error && (
        <div className="tree-error" role="alert" data-testid="evals-error">
          {error}{" "}
          <button type="button" onClick={load}>
            {t("evals.retry")}
          </button>
        </div>
      )}
      {runs !== null && runs.length === 0 && <div className="muted">{t("evals.empty")}</div>}
      {runs !== null && (
        <table aria-label={t("evals.title")}>
          <caption className="sr-only">{t("evals.caption")}</caption>
          <thead>
            <tr>
              <th scope="col">{t("evals.col_target")}</th>
              <th scope="col">{t("evals.col_time")}</th>
              <th scope="col">{t("evals.col_verdict")}</th>
              <th scope="col">{t("evals.col_pass_rate")}</th>
              <th scope="col">{t("evals.col_tokens")}</th>
              <th scope="col">{t("evals.col_steps")}</th>
              <th scope="col">{t("evals.col_risk_actions")}</th>
              <th scope="col">{t("evals.col_violations")}</th>
              <th scope="col">{t("evals.col_l4_hit")}</th>
              <th scope="col">{t("evals.col_l4_score")}</th>
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
      )}
    </div>
  );
}
