// 行内 AI 指令（§7.4 Cmd+I / §8.5 共生集成点，S2 / T8）：
// 选区上下文组装、注入任务文本约束、弹卡交互与无文件守卫。
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { InlineInstruction, buildInlineTask } from "../components/InlineInstruction";

const t = (key: string) =>
  // 占位符模板需可替换，其余键原样返回
  key === "inline.range" ? "{path} lines {start}-{end}" : key;

describe("buildInlineTask", () => {
  it("选区模式：带文件、行区间、选区摘要与零改动约束（T8）", () => {
    const text = buildInlineTask("改写为 async 并补错误处理", {
      path: "src/auth.ts",
      startLine: 12,
      endLine: 48,
      excerpt: "function load() { return fetch('/x'); }",
    });
    expect(text).toContain("src/auth.ts 的12-48 行内");
    expect(text).toContain("改写为 async 并补错误处理");
    expect(text).toContain("function load()");
    expect(text).toContain("选中区外零改动");
    expect(text).toContain("无新诊断");
  });

  it("无选区退化为整文件模式，不含选区摘要", () => {
    const text = buildInlineTask("补充类型标注", { path: "src/util.ts" });
    expect(text).toContain("src/util.ts 的整个文件内");
    expect(text).not.toContain("选中代码");
    expect(text).toContain("选中区外零改动");
  });
});

describe("InlineInstruction", () => {
  it("渲染选区上下文，提交回调带指令与目标，Esc 关闭", () => {
    const onSend = vi.fn();
    const onClose = vi.fn();
    const { container } = render(
      <InlineInstruction
        open
        selection={{ path: "src/auth.ts", startLine: 3, endLine: 7, text: "let x = 1;" }}
        activePath="src/auth.ts"
        t={t}
        onClose={onClose}
        onSend={onSend}
      />
    );
    expect(screen.getByTestId("inline-context").textContent).toContain("src/auth.ts");

    fireEvent.change(screen.getByTestId("inline-input"), {
      target: { value: "改为 const" },
    });
    fireEvent.click(screen.getByTestId("inline-send"));
    expect(onSend).toHaveBeenCalledWith("改为 const", {
      path: "src/auth.ts",
      startLine: 3,
      endLine: 7,
      excerpt: "let x = 1;",
    });
    expect(onClose).toHaveBeenCalled();

    fireEvent.keyDown(container.firstChild as HTMLElement, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("无活动文件时守卫：提示打开文件且发送禁用", () => {
    const onSend = vi.fn();
    render(
      <InlineInstruction
        open
        selection={null}
        activePath={null}
        t={t}
        onClose={vi.fn()}
        onSend={onSend}
      />
    );
    expect(screen.getByTestId("inline-context").textContent).toContain("inline.no_file");
    expect((screen.getByTestId("inline-send") as HTMLButtonElement).disabled).toBe(true);
  });

  it("open=false 不渲染", () => {
    render(
      <InlineInstruction
        open={false}
        selection={null}
        activePath="src/a.ts"
        t={t}
        onClose={vi.fn()}
        onSend={vi.fn()}
      />
    );
    expect(screen.queryByTestId("inline-instruction")).toBeNull();
  });
});
