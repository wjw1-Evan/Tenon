// L4 索引诊断（§10.1 / v1.35）：WS 推送、错误显示、手动 rebuild。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { L4StatusPanel } from "../components/L4StatusPanel";
import type { TenonApi } from "../lib/api";

const statsMock = vi.fn();
const rebuildMock = vi.fn();
let statusListener: ((event: unknown) => void) | null = null;

function api() {
  return {
    l4Stats: statsMock,
    l4Rebuild: rebuildMock,
    connectEvents: vi.fn().mockImplementation(async (listener: (event: unknown) => void) => {
      statusListener = listener;
      return { close: vi.fn(), onclose: null };
    }),
  } as unknown as TenonApi;
}


function ready(chunks: number) {
  return {
    project_id: "p1",
    chunks,
    status: { state: "ready", updated_at: "2026-10-04T00:00:01Z", error: null },
  };
}

function queued(chunks: number) {
  return {
    project_id: "p1",
    chunks,
    status: { state: "queued", updated_at: "2026-10-04T00:00:00Z", error: null },
  };
}

function failed(chunks: number) {
  return {
    project_id: "p1",
    chunks,
    status: { state: "failed", updated_at: "2026-10-04T00:00:00Z", error: "scan failed" },
  };
}

describe("L4StatusPanel", () => {
  beforeEach(() => {
    statsMock.mockReset();
    rebuildMock.mockReset();
    statusListener = null;
  });

  it("渲染切片数与 ready 状态", async () => {
    statsMock.mockImplementation(async () => ready(7));
    render(<L4StatusPanel api={api()} t={(key) => key} projectId="p1" />);
    expect(await screen.findByTestId("l4-state")).toHaveTextContent("ready");
    expect(screen.getByTestId("l4-chunks")).toHaveTextContent("chunks: 7");
  });

  it("failed 状态展示错误，重建成功后刷新", async () => {
    const responses = [
      failed(0),
      failed(0),
      ready(3),
    ];
    statsMock.mockImplementation(async () => responses.shift() ?? ready(3));
    rebuildMock.mockResolvedValue({ queued: true, project_id: "p1" });
    render(<L4StatusPanel api={api()} t={(key) => key} projectId="p1" />);
    expect(await screen.findByText("scan failed")).toBeInTheDocument();
    fireEvent.click(await screen.findByTestId("l4-rebuild"));
    await waitFor(() => expect(rebuildMock).toHaveBeenCalledWith("p1"));
    await waitFor(() => expect(screen.getByTestId("l4-state")).toHaveTextContent("ready"));
    expect(screen.getByTestId("l4-chunks")).toHaveTextContent("chunks: 3");
  });

  it("refreshes from authenticated l4_status WS events without polling", async () => {
    const responses = [
      queued(1),
      queued(1),
      ready(4),
    ];
    statsMock.mockImplementation(async () => responses.shift() ?? ready(4));
    render(<L4StatusPanel api={api()} t={(key) => key} projectId="p1" />);
    expect(await screen.findByTestId("l4-state")).toHaveTextContent("queued");

    statusListener?.({ type: "l4_status", project_id: "p1", state: "ready" });
    await waitFor(() => expect(screen.getByTestId("l4-state")).toHaveTextContent("ready"));
    expect(screen.getByTestId("l4-chunks")).toHaveTextContent("chunks: 4");
  });
});
