import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ApprovalCard, pendingApprovalFromEvents } from "../components/ApprovalCard";
import { createTranslator } from "../lib/i18n";
import type { TenonApi } from "../lib/api";

const t = createTranslator("en");

describe("审批卡片（§7.3）", () => {
  it("渲染级别、动作与三键（允许一次 / 本会话 / 拒绝）", () => {
    const api = { decideApproval: vi.fn().mockResolvedValue({}) } as unknown as TenonApi;
    render(
      <ApprovalCard
        api={api}
        t={t}
        approval={{ approvalId: "a1", tool: "git_commit", level: "d", summary: "git 提交：msg" }}
      />
    );
    expect(screen.getByTestId("approval-card")).toHaveAttribute("data-level", "d");
    expect(screen.getByText("git_commit")).toBeInTheDocument();
    expect(screen.getByTestId("approve-once")).toHaveTextContent("Allow once");
    expect(screen.getByTestId("approve-session")).toHaveTextContent("Allow for session");
    expect(screen.getByTestId("approve-deny")).toHaveTextContent("Deny");
  });

  it("点击派发审批决策", async () => {
    const api = { decideApproval: vi.fn().mockResolvedValue({}) } as unknown as TenonApi;
    const onDecided = vi.fn();
    render(
      <ApprovalCard
        api={api}
        t={t}
        approval={{ approvalId: "a1", tool: "http_fetch", level: "c", summary: "出网抓取 example.com" }}
        onDecided={onDecided}
      />
    );
    fireEvent.click(screen.getByTestId("approve-once"));
    await waitFor(() => expect(api.decideApproval).toHaveBeenCalledWith("a1", "once"));
    expect(onDecided).toHaveBeenCalledWith("once");
  });

  it("pendingApprovalFromEvents 提取最近审批卡", () => {
    const events = [
      { type: "user_input", payload: {} },
      { type: "approval_request", payload: { approval_id: "a9", tool: "git_push", level: "d", summary: "push" } },
    ];
    const info = pendingApprovalFromEvents(events);
    expect(info?.approvalId).toBe("a9");
    expect(info?.level).toBe("d");
    expect(pendingApprovalFromEvents([{ type: "user_input", payload: {} }])).toBeNull();
  });

  it("pendingApprovalFromEvents 决策已落 trace 则不复活旧审批卡（回放回归）", () => {
    const events = [
      { type: "user_input", payload: {} },
      { type: "approval_request", payload: { approval_id: "a1", tool: "run_build", level: "b", summary: "build" } },
      { type: "patch_applied", payload: {} },
      { type: "approval_decision", payload: { approval_id: "a1", decision: "once" } },
      { type: "done", payload: {} },
    ];
    expect(pendingApprovalFromEvents(events)).toBeNull();
    // 多轮审批：最后一个请求未决策 → 待审批
    const events2 = [
      { type: "approval_request", payload: { approval_id: "a1", tool: "run_build", level: "b", summary: "" } },
      { type: "approval_decision", payload: { approval_id: "a1", decision: "once" } },
      { type: "approval_request", payload: { approval_id: "a2", tool: "apply_patch", level: "b", summary: "patch" } },
    ];
    expect(pendingApprovalFromEvents(events2)?.approvalId).toBe("a2");
  });
});
