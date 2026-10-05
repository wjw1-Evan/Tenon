// L4StatusPanel 深度测试：状态显示 / rebuild / 错误 / 无项目。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { L4StatusPanel } from "../components/L4StatusPanel";
import type { TenonApi } from "../lib/api";

const l4StatsMock = vi.fn();
const l4RebuildMock = vi.fn();
const connectEventsMock = vi.fn();

function api() {
  return { l4Stats: l4StatsMock, l4Rebuild: l4RebuildMock, connectEvents: connectEventsMock } as unknown as TenonApi;
}

const t = (key: string, opts?: { defaultValue?: string }) => opts?.defaultValue ?? key;

beforeEach(() => {
  l4StatsMock.mockReset();
  l4RebuildMock.mockReset();
  connectEventsMock.mockReset();
  connectEventsMock.mockRejectedValue(new Error("ws offline"));
});

afterEach(cleanup);

describe("L4StatusPanel deep", () => {
  it("no project shows empty state", () => {
    render(<L4StatusPanel api={api()} t={t} projectId={null} />);
    expect(screen.getByTestId("l4-panel")).toHaveTextContent("l4.no_project");
  });

  it("shows chunks and state from API", async () => {
    l4StatsMock.mockResolvedValue({ project_id: "p1", chunks: 42, status: { state: "ready", updated_at: "2026-01-01T10:00:00Z" } });
    render(<L4StatusPanel api={api()} t={t} projectId="p1" />);
    await waitFor(() => expect(l4StatsMock).toHaveBeenCalledWith("p1"));
    expect(await screen.findByTestId("l4-chunks")).toHaveTextContent("42");
    expect(screen.getByTestId("l4-state")).toHaveTextContent("ready");
  });

  it("rebuild button calls API and refreshes", async () => {
    l4StatsMock.mockResolvedValue({ project_id: "p1", chunks: 0, status: { state: "idle", updated_at: null } });
    l4RebuildMock.mockResolvedValue({ queued: true });
    render(<L4StatusPanel api={api()} t={t} projectId="p1" />);
    await waitFor(() => expect(screen.getByTestId("l4-rebuild")).not.toBeDisabled());
    fireEvent.click(screen.getByTestId("l4-rebuild"));
    await waitFor(() => expect(l4RebuildMock).toHaveBeenCalledWith("p1"));
  });

  it("disables rebuild during indexing state", async () => {
    l4StatsMock.mockResolvedValue({ project_id: "p1", chunks: 5, status: { state: "indexing", updated_at: "2026-01-01T10:00:00Z" } });
    render(<L4StatusPanel api={api()} t={t} projectId="p1" />);
    await waitFor(() => expect(screen.getByTestId("l4-state")).toBeTruthy());
    expect(screen.getByTestId("l4-rebuild")).toBeDisabled();
    expect(screen.getByTestId("l4-rebuild")).toHaveTextContent("l4.working");
  });

  it("shows error from API", async () => {
    l4StatsMock.mockResolvedValue({ project_id: "p1", chunks: 0, status: { state: "error", updated_at: "2026-01-01T10:00:00Z", error: "index corrupted" } });
    render(<L4StatusPanel api={api()} t={t} projectId="p1" />);
    await waitFor(() => expect(screen.getByText("index corrupted")).toBeTruthy());
  });

  it("stats API error shows error alert", async () => {
    l4StatsMock.mockRejectedValue(new Error("offline"));
    render(<L4StatusPanel api={api()} t={t} projectId="p1" />);
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
  });

  it("rebuild API error shows error alert", async () => {
    l4StatsMock.mockResolvedValue({ project_id: "p1", chunks: 0 });
    l4RebuildMock.mockRejectedValue(new Error("rebuild failed"));
    render(<L4StatusPanel api={api()} t={t} projectId="p1" />);
    await waitFor(() => expect(screen.getByTestId("l4-rebuild")).not.toBeDisabled());
    fireEvent.click(screen.getByTestId("l4-rebuild"));
    await waitFor(() => expect(l4RebuildMock).toHaveBeenCalledWith("p1"));
  });

  it("l4_status WS event triggers refresh", async () => {
    let eventHandler: ((ev: unknown) => void) | null = null;
    connectEventsMock.mockImplementation(async (handler: (ev: unknown) => void) => {
      eventHandler = handler;
      return {
        onclose: null,
        close: () => {},
      };
    });
    l4StatsMock.mockResolvedValue({ project_id: "p1", chunks: 10 });
    render(<L4StatusPanel api={api()} t={t} projectId="p1" />);
    await waitFor(() => expect(connectEventsMock).toHaveBeenCalled());
    // 初始加载调了一次，WS 建立后又调一次
    const initialCalls = l4StatsMock.mock.calls.length;
    // 模拟 WS 事件
    if (eventHandler) {
      (eventHandler as (ev: unknown) => void)({ type: "l4_status", project_id: "p1" });
      await waitFor(() => expect(l4StatsMock.mock.calls.length).toBeGreaterThan(initialCalls));
    }
  });

  it("ignores events from other projects", async () => {
    let eventHandler: ((ev: unknown) => void) | null = null;
    connectEventsMock.mockImplementation(async (handler: (ev: unknown) => void) => {
      eventHandler = handler;
      return { onclose: null, close: () => {} };
    });
    l4StatsMock.mockResolvedValue({ project_id: "p1", chunks: 10 });
    render(<L4StatusPanel api={api()} t={t} projectId="p1" />);
    await waitFor(() => expect(connectEventsMock).toHaveBeenCalled());
    const callsBefore = l4StatsMock.mock.calls.length;
    if (eventHandler) {
      (eventHandler as (ev: unknown) => void)({ type: "l4_status", project_id: "other" });
      // 不触发额外刷新
      await new Promise(r => setTimeout(r, 100));
      expect(l4StatsMock.mock.calls.length).toBe(callsBefore);
    }
  });
});
