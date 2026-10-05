// Evals 报告必须同时呈现五指标与 v1.34 L4 上下文质量门禁。
import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { EvalsPanel } from "../components/EvalsPanel";
import type { TenonApi } from "../lib/api";

describe("EvalsPanel", () => {
  it("renders persisted L4 recall gate metrics from daemon reports", async () => {
    const api = {
      evalRuns: vi.fn().mockResolvedValue({
        runs: [
          {
            id: "run-1",
            target: "M0-baseline-mock",
            verdict: "pass",
            created_at: "2026-10-04T08:00:00Z",
            metrics_json: {
              pass_rate: 0.9,
              total_tokens: 1234,
              total_steps: 22,
              total_risk_actions: 1,
              security_violations: 0,
              l4_recall_hit_rate: 0.8,
              l4_average_score: 0.821,
            },
          },
        ],
      }),
    } as unknown as TenonApi;

    render(<EvalsPanel api={api} />);
    await waitFor(() => expect(screen.getByText("M0-baseline-mock")).toBeTruthy());
    expect(screen.getByText("90%")).toBeTruthy();
    expect(screen.getByText("80%")).toBeTruthy();
    expect(screen.getByText("0.821")).toBeTruthy();
    expect(screen.getByTestId("evals-l4-hit-run-1").textContent).toBe("80%");
  });
});
