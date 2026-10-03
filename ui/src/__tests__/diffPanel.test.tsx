import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { DiffPanel, diffFromPatchEvent } from "../components/DiffPanel";

describe("diff 面板（M0 交付）", () => {
  it("渲染统一 diff 内容", () => {
    const diff = "--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-old\n+new";
    render(<DiffPanel diff={diff} />);
    expect(screen.getByTestId("diff-panel")).toHaveTextContent("+new");
    expect(screen.getByTestId("diff-panel")).toHaveTextContent("-old");
  });

  it("无改动时显示占位", () => {
    render(<DiffPanel diff={null} />);
    expect(screen.getByTestId("diff-panel")).toHaveTextContent("（无改动）");
  });

  it("从 patch_applied 事件提取 diff", () => {
    const payload = {
      tool: "apply_patch",
      output: { ok: true, content: "已写入 f.txt\n--- a/f.txt\n+++ b/f.txt\n+x" },
    };
    expect(diffFromPatchEvent(payload)).toContain("+x");
    // 非 patch 事件 / 无 diff 内容返回 null
    expect(diffFromPatchEvent({ tool: "grep", output: { ok: true, content: "hits" } })).toBeNull();
    expect(diffFromPatchEvent({})).toBeNull();
  });
});
