// 活动文件诊断必须走 project-scoped LSP，并能把诊断转成一键修复任务（T4）。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DiagnosticsPanel } from "../components/DiagnosticsPanel";
import type { TenonApi } from "../lib/api";

const lspMock = vi.fn();

function api() {
  return { lsp: lspMock } as unknown as TenonApi;
}

beforeEach(() => {
  lspMock.mockReset();
});

describe("DiagnosticsPanel", () => {
  it("normalizes LSP diagnostics and converts a row into an AI fix task", async () => {
    const onFix = vi.fn();
    const onOpenFile = vi.fn();
    lspMock.mockResolvedValue({
      result: {
        items: [{
          range: { start: { line: 11, character: 4 } },
          severity: 1,
          source: "ts",
          code: 2322,
          message: "Type 'string' is not assignable to type 'number'.",
        }],
      },
    });

    render(
      <DiagnosticsPanel
        api={api()}
        t={(key) => key}
        projectId="project-a"
        path="src/app.ts"
        refreshToken={1}
        sessionId="session-1"
        onOpenFile={onOpenFile}
        onFix={onFix}
      />
    );

    await waitFor(() => expect(lspMock).toHaveBeenCalledWith({
      project_id: "project-a",
      path: "src/app.ts",
      action: "diagnostics",
    }));
    expect(await screen.findByText("Type 'string' is not assignable to type 'number'.")).toBeTruthy();
    expect(screen.getByTestId("diagnostic-summary")).toHaveTextContent("1");

    fireEvent.click(screen.getByRole("button", { name: "src/app.ts:12:5" }));
    expect(onOpenFile).toHaveBeenCalledWith("src/app.ts", 12);

    fireEvent.click(screen.getByTestId("diagnostic-fix-0"));
    expect(onFix).toHaveBeenCalledWith(expect.objectContaining({
      path: "src/app.ts",
      line: 12,
      column: 5,
      severity: 1,
      message: "Type 'string' is not assignable to type 'number'.",
    }));
  });

  it("refreshes diagnostics on request and reports unavailable language services", async () => {
    lspMock.mockRejectedValueOnce(new Error("language server unavailable"));
    render(
      <DiagnosticsPanel
        api={api()}
        t={(key) => key}
        projectId="project-a"
        path="src/app.ts"
        refreshToken={1}
        sessionId={null}
        onOpenFile={() => {}}
        onFix={() => {}}
      />
    );
    expect(await screen.findByText(/language server unavailable/)).toBeInTheDocument();

    lspMock.mockResolvedValue({ result: { items: [] } });
    fireEvent.click(screen.getByTestId("diagnostic-refresh"));
    expect(await screen.findByTestId("diagnostics-clean")).toBeInTheDocument();
    expect(lspMock).toHaveBeenCalledTimes(2);
  });
});

describe("DiagnosticsPanel 补充", () => {
  it("no language pack error shows degraded message", async () => {
    lspMock.mockRejectedValue(new Error("语言包不可用：typescript"));
    const { getByTestId } = render(
      <DiagnosticsPanel
        api={api()}
        t={(key) => key}
        projectId="p1"
        path="a.ts"
        refreshToken={0}
        sessionId={null}
        onOpenFile={vi.fn()}
        onFix={vi.fn()}
      />
    );
    await waitFor(() => {
      expect(getByTestId("diagnostics-no-language-pack")).toBeTruthy();
    });
  });

  it("severities render error/warning/info labels", async () => {
    lspMock.mockResolvedValue({
      result: {
        items: [
          { range: { start: { line: 0, character: 0 } }, severity: 1, message: "err" },
          { range: { start: { line: 1, character: 0 } }, severity: 2, message: "warn" },
          { range: { start: { line: 2, character: 0 } }, severity: 3, message: "info" },
        ],
      },
    });
    render(
      <DiagnosticsPanel
        api={api()}
        t={(key) => key}
        projectId="p1"
        path="a.ts"
        refreshToken={0}
        sessionId={null}
        onOpenFile={vi.fn()}
        onFix={vi.fn()}
      />
    );
    await waitFor(() => {
      const summary = document.querySelector('[data-testid="diagnostic-summary"]');
      expect(summary?.textContent).toContain("diagnostic.error");
      expect(summary?.textContent).toContain("diagnostic.warning");
    });
  });

  it("non-path renders no_file message", () => {
    render(
      <DiagnosticsPanel
        api={api()}
        t={(key) => key}
        projectId={null}
        path={null}
        refreshToken={0}
        sessionId={null}
        onOpenFile={vi.fn()}
        onFix={vi.fn()}
      />
    );
    expect(screen.getByText("diagnostic.no_file")).toBeTruthy();
  });
});
