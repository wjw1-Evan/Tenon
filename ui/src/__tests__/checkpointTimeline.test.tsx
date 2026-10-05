// CheckpointTimeline 测试：加载 / 回滚 / 撤销回滚 / 空态。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CheckpointTimeline } from "../components/CheckpointTimeline";
import type { TenonApi } from "../lib/api";

const checkpointsMock = vi.fn();
const rollbackMock = vi.fn();
const controlMock = vi.fn();

function api() {
  return {
    checkpoints: checkpointsMock,
    rollbackCheckpoint: rollbackMock,
    control: controlMock,
  } as unknown as TenonApi;
}

const t = (key: string) => key;

beforeEach(() => {
  checkpointsMock.mockReset();
  rollbackMock.mockReset();
  controlMock.mockReset();
});

afterEach(cleanup);

const cps = [
  { id: "cp-1", tree: "aaaaaaaa1234567890", files: ["a.ts", "b.ts"], created_at: "2026-01-01T10:00:00Z" },
  { id: "cp-2", tree: "bbbbbbbb1234567890", files: [], created_at: "2026-01-01T11:00:00Z" },
];

describe("CheckpointTimeline", () => {
  it("renders empty state when sessionId is null", () => {
    render(<CheckpointTimeline api={api()} t={t} sessionId={null} />);
    expect(screen.getByTestId("timeline")).toBeTruthy();
    expect(screen.getByText("timeline.empty")).toBeTruthy();
    expect(checkpointsMock).not.toHaveBeenCalled();
  });

  it("loads and renders checkpoints in reverse order", async () => {
    checkpointsMock.mockResolvedValue({ checkpoints: cps });
    render(<CheckpointTimeline api={api()} t={t} sessionId="s1" />);
    await waitFor(() => expect(checkpointsMock).toHaveBeenCalledWith("s1"));
    // 最新的在前
    const nodes = document.querySelectorAll(".timeline-node");
    expect(nodes.length).toBe(2);
    expect(nodes[0].querySelector(".tree-oid")?.textContent).toBe("bbbbbbbb");
    // 空文件列表显示 "—"
    expect(nodes[0].querySelector(".files")?.textContent).toBe("—");
  });

  it("rollback button calls rollbackCheckpoint", async () => {
    checkpointsMock.mockResolvedValue({ checkpoints: cps });
    rollbackMock.mockResolvedValue({ rolled_back: ["a.ts"] });
    render(<CheckpointTimeline api={api()} t={t} sessionId="s1" />);
    await waitFor(() => expect(screen.getAllByRole("button", { name: "timeline.rollback" }).length).toBe(2));
    fireEvent.click(screen.getAllByRole("button", { name: "timeline.rollback" })[0]);
    await waitFor(() => expect(rollbackMock).toHaveBeenCalledWith("cp-2"));
  });

  it("unrevert button calls session control", async () => {
    checkpointsMock.mockResolvedValue({ checkpoints: [] });
    controlMock.mockResolvedValue({ ok: true });
    render(<CheckpointTimeline api={api()} t={t} sessionId="s1" />);
    await waitFor(() => expect(screen.getByRole("button", { name: "timeline.unrevert" })).not.toBeDisabled());
    fireEvent.click(screen.getByRole("button", { name: "timeline.unrevert" }));
    await waitFor(() => expect(controlMock).toHaveBeenCalledWith("s1", "unrollback"));
  });

  it("handles API errors gracefully", async () => {
    checkpointsMock.mockRejectedValue(new Error("offline"));
    render(<CheckpointTimeline api={api()} t={t} sessionId="s1" />);
    // 不崩溃、显示空态
    await waitFor(() => expect(checkpointsMock).toHaveBeenCalled());
    expect(screen.getByText("timeline.empty")).toBeTruthy();
  });
});
