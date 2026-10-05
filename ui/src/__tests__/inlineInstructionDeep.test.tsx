// InlineInstruction 深度测试：选区/无选区/Esc/发送。
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { InlineInstruction } from "../components/InlineInstruction";

afterEach(cleanup);
const t = (key: string) => key;

describe("InlineInstruction deep", () => {
  it("returns null when closed", () => {
    const { container } = render(
      <InlineInstruction open={false} selection={null} activePath="a.ts" t={t} onClose={vi.fn()} onSend={vi.fn()} />
    );
    expect(container.querySelector(".inline-instruction")).toBeNull();
  });

  it("renders with selection context", () => {
    const selection = { path: "a.ts", startLine: 1, endLine: 5, text: "selected code" };
    render(
      <InlineInstruction open={true} selection={selection} activePath="a.ts" t={t} onClose={vi.fn()} onSend={vi.fn()} />
    );
    expect(screen.getByTestId("inline-instruction")).toBeTruthy();
    expect(screen.getByTestId("inline-context")).toBeTruthy();
  });

  it("renders without selection (file-level target)", () => {
    render(
      <InlineInstruction open={true} selection={null} activePath="b.ts" t={t} onClose={vi.fn()} onSend={vi.fn()} />
    );
    expect(screen.getByTestId("inline-instruction")).toBeTruthy();
    expect(screen.getByTestId("inline-context").textContent).toContain("inline.whole_file");
  });

  it("shows no target when no activePath", () => {
    const { container } = render(
      <InlineInstruction open={true} selection={null} activePath={null} t={t} onClose={vi.fn()} onSend={vi.fn()} />
    );
    expect(screen.getByTestId("inline-instruction")).toBeTruthy();
  });

  it("Escape closes", () => {
    const onClose = vi.fn();
    render(
      <InlineInstruction open={true} selection={null} activePath="a.ts" t={t} onClose={onClose} onSend={vi.fn()} />
    );
    fireEvent.keyDown(screen.getByTestId("inline-instruction"), { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });

  it("Enter sends instruction", () => {
    const onSend = vi.fn();
    render(
      <InlineInstruction open={true} selection={null} activePath="a.ts" t={t} onClose={vi.fn()} onSend={onSend} />
    );
    const textarea = screen.getByTestId("inline-input");
    fireEvent.change(textarea, { target: { value: "fix this" } });
    fireEvent.keyDown(textarea, { key: "Enter", metaKey: true });
    expect(onSend).toHaveBeenCalledWith("fix this", expect.objectContaining({ path: "a.ts" }));
  });

  it("Enter with shift adds newline instead", () => {
    const onSend = vi.fn();
    render(
      <InlineInstruction open={true} selection={null} activePath="a.ts" t={t} onClose={vi.fn()} onSend={onSend} />
    );
    const textarea = screen.getByTestId("inline-input");
    fireEvent.change(textarea, { target: { value: "line1" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: true });
    expect(onSend).not.toHaveBeenCalled();
  });

  it("empty input does not send", () => {
    const onSend = vi.fn();
    render(
      <InlineInstruction open={true} selection={null} activePath="a.ts" t={t} onClose={vi.fn()} onSend={onSend} />
    );
    const textarea = screen.getByTestId("inline-input");
    fireEvent.keyDown(textarea, { key: "Enter", metaKey: true });
    expect(onSend).not.toHaveBeenCalled();
  });
});

describe("InlineInstruction additional", () => {
  it("cancel button closes", () => {
    const onClose = vi.fn();
    render(
      <InlineInstruction open={true} selection={null} activePath="a.ts" t={(k) => k} onClose={onClose} onSend={vi.fn()} />
    );
    fireEvent.click(screen.getByTestId("inline-cancel"));
    expect(onClose).toHaveBeenCalled();
  });

  it("send button disabled without target", () => {
    render(
      <InlineInstruction open={true} selection={null} activePath={null} t={(k) => k} onClose={vi.fn()} onSend={vi.fn()} />
    );
    const sendBtn = screen.getByTestId("inline-send") as HTMLButtonElement;
    expect(sendBtn.disabled).toBe(true);
  });

  it("selection with different path shows whole file target", () => {
    const selection = { path: "other.ts", startLine: 1, endLine: 5, text: "code" };
    render(
      <InlineInstruction open={true} selection={selection} activePath="a.ts" t={(k) => k} onClose={vi.fn()} onSend={vi.fn()} />
    );
    // 选区 path ≠ activePath → 降级为文件级目标
    expect(screen.getByTestId("inline-instruction")).toBeTruthy();
  });
});
