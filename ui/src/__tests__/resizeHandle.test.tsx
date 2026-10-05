// 拖拽分割条测试：mousedown 启动拖拽 / mousemove 回调增量 / mouseup 停止。
import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ResizeHandle } from "../components/ResizeHandle";

describe("ResizeHandle", () => {
  it("renders with correct orientation and testid", () => {
    const { getByTestId } = render(<ResizeHandle dir="horizontal" onResize={vi.fn()} />);
    const el = getByTestId("resize-handle");
    expect(el.getAttribute("aria-orientation")).toBe("vertical");
    expect(el.className).toContain("resize-horizontal");
  });

  it("calls onResize with pixel delta during horizontal drag", () => {
    const onResize = vi.fn();
    const { getByTestId } = render(<ResizeHandle dir="horizontal" onResize={onResize} />);
    const el = getByTestId("resize-handle");

    fireEvent.mouseDown(el, { clientX: 100, clientY: 0 });
    fireEvent.mouseMove(window, { clientX: 140, clientY: 0 });
    expect(onResize).toHaveBeenCalledWith(40);
    fireEvent.mouseMove(window, { clientX: 120, clientY: 0 });
    expect(onResize).toHaveBeenCalledWith(-20);

    fireEvent.mouseUp(window);
    // mouseup 后不再回调
    fireEvent.mouseMove(window, { clientX: 200, clientY: 0 });
    expect(onResize).toHaveBeenCalledTimes(2);
  });

  it("calls onResize with vertical delta during vertical drag", () => {
    const onResize = vi.fn();
    const { getByTestId } = render(<ResizeHandle dir="vertical" onResize={onResize} />);
    const el = getByTestId("resize-handle");
    expect(el.getAttribute("aria-orientation")).toBe("horizontal");

    fireEvent.mouseDown(el, { clientX: 0, clientY: 50 });
    fireEvent.mouseMove(window, { clientX: 0, clientY: 80 });
    expect(onResize).toHaveBeenCalledWith(30);
    fireEvent.mouseUp(window);
  });

  it("supports custom testId and double-click", () => {
    const onDoubleClick = vi.fn();
    const { getByTestId } = render(
      <ResizeHandle dir="horizontal" onResize={vi.fn()} onDoubleClick={onDoubleClick} testId="split-left" />
    );
    fireEvent.dblClick(getByTestId("split-left"));
    expect(onDoubleClick).toHaveBeenCalledTimes(1);
  });
});
