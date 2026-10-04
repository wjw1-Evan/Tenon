// 可拖拽分割条（设计方案 §7.2：三区可折叠可调宽）。
import { useCallback, useRef } from "react";

export type ResizeDir = "horizontal" | "vertical";

interface ResizeHandleProps {
  dir: ResizeDir;
  onResize: (deltaPx: number) => void;
  onDoubleClick?: () => void;
  testId?: string;
}

/** 拖拽分割条：鼠标/触摸拖拽回调像素增量（水平 dir → deltaX，垂直 → deltaY）。 */
export function ResizeHandle({ dir, onResize, onDoubleClick, testId }: ResizeHandleProps) {
  const dragging = useRef(false);
  const lastPos = useRef(0);

  const onMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      dragging.current = true;
      lastPos.current = dir === "horizontal" ? e.clientX : e.clientY;

      const onMove = (me: MouseEvent) => {
        if (!dragging.current) return;
        const cur = dir === "horizontal" ? me.clientX : me.clientY;
        const delta = cur - lastPos.current;
        lastPos.current = cur;
        onResize(delta);
      };
      const onUp = () => {
        dragging.current = false;
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [dir, onResize]
  );

  return (
    <div
      className={`resize-handle resize-${dir}`}
      onMouseDown={onMouseDown}
      onDoubleClick={onDoubleClick}
      data-testid={testId ?? "resize-handle"}
      role="separator"
      aria-orientation={dir === "horizontal" ? "vertical" : "horizontal"}
    />
  );
}
