// Evals 报告必须同时呈现五指标与 v1.34 L4 上下文质量门禁；
// v1.167 三态：loading / 错误（inline + 重试）不再吞错。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { EvalsPanel } from "../components/EvalsPanel";
import type { TenonApi } from "../lib/api";
import { __resetToastsForTest } from "../lib/toast";

const t = (key: string) => key;

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

    render(<EvalsPanel api={api} t={t} />);
    await waitFor(() => expect(screen.getByText("M0-baseline-mock")).toBeTruthy());
    expect(screen.getByText("90%")).toBeTruthy();
    expect(screen.getByText("80%")).toBeTruthy();
    expect(screen.getByText("0.821")).toBeTruthy();
    expect(screen.getByTestId("evals-l4-hit-run-1").textContent).toBe("80%");
  });

  it("shows loading state until first load resolves", async () => {
    let resolveRuns: (value: { runs: never[] }) => void = () => {};
    const api = {
      evalRuns: vi.fn().mockImplementation(
        () => new Promise((resolve) => { resolveRuns = resolve; })
      ),
    } as unknown as TenonApi;
    render(<EvalsPanel api={api} t={t} />);
    expect(screen.getByTestId("evals-loading")).toBeTruthy();
    resolveRuns({ runs: [] });
    await waitFor(() => expect(screen.queryByTestId("evals-loading")).toBeNull());
    expect(screen.getByText("evals.empty")).toBeTruthy();
  });

  it("shows inline error with retry on load failure", async () => {
    __resetToastsForTest();
    const api = {
      evalRuns: vi.fn().mockRejectedValueOnce(new Error("API 503: down")),
    } as unknown as TenonApi;
    render(<EvalsPanel api={api} t={t} />);
    expect(await screen.findByTestId("evals-error")).toBeTruthy();
    expect(screen.getByTestId("evals-error").textContent).toContain("API 503: down");

    (api.evalRuns as ReturnType<typeof vi.fn>).mockResolvedValue({ runs: [] });
    fireEvent.click(screen.getByRole("button", { name: "evals.retry" }));
    await waitFor(() => expect(screen.queryByTestId("evals-error")).toBeNull());
    expect(api.evalRuns).toHaveBeenCalledTimes(2);
  });
});
