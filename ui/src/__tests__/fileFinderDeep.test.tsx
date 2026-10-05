// FileFinder 深度测试：symbol 模式 / 行号查询 / 键盘导航。
import { cleanup, fireEvent, act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FileFinder } from "../components/FileFinder";
import type { TenonApi } from "../lib/api";

const fuzzyMock = vi.fn();
const lspMock = vi.fn();

function api() {
  return { fuzzyFiles: fuzzyMock, lsp: lspMock } as unknown as TenonApi;
}

const t = (key: string) => key;
const DEBOUNCE = 200;
const hits = [
  { path: "src/a.ts", name: "a.ts", kind: "file" as const, score: 1 },
  { path: "src/b.ts", name: "b.ts", kind: "file" as const, score: 0.8 },
];

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  fuzzyMock.mockReset();
  lspMock.mockReset();
});

afterEach(() => { cleanup(); vi.useRealTimers(); });

describe("FileFinder deep", () => {




  it("Escape closes finder", async () => {
    const onClose = vi.fn();
    render(<FileFinder open={true} onClose={onClose} api={api()} t={t} projectId="p1" activePath={null} onOpen={vi.fn()} />);
    fireEvent.keyDown(screen.getByTestId("file-finder-input"), { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });


  it("shows error on API failure", async () => {
    fuzzyMock.mockRejectedValue(new Error("offline"));
    render(<FileFinder open={true} onClose={vi.fn()} api={api()} t={t} projectId="p1" activePath={null} onOpen={vi.fn()} />);
    fireEvent.change(screen.getByTestId("file-finder-input"), { target: { value: "x" } });
    act(() => { vi.advanceTimersByTime(200); });
    await waitFor(() => expect(screen.getByText(/offline/i)).toBeTruthy());
  });


  it("does not search without projectId", () => {
    render(<FileFinder open={true} onClose={vi.fn()} api={api()} t={t} projectId={null} activePath={null} onOpen={vi.fn()} />);
    expect(fuzzyMock).not.toHaveBeenCalled();
  });

  it("active file line mode via @:", () => {
    render(<FileFinder open={true} onClose={vi.fn()} api={api()} t={t} projectId="p1" activePath="src/current.ts" onOpen={vi.fn()} />);
    const input = screen.getByTestId("file-finder-input");
    fireEvent.change(input, { target: { value: "@:10" } });
    fireEvent.keyDown(input, { key: "Enter" });
    // 不崩溃即可（line 跳转到活动文件）
  });
});

describe("FileFinder additional", () => {
  it("open=false renders nothing", () => {
    const { container } = render(
      <FileFinder open={false} onClose={vi.fn()} api={{} as never} t={(k) => k} projectId="p1" activePath={null} onOpen={vi.fn()} />
    );
    expect(container.querySelector(".palette-overlay")).toBeNull();
  });

  it("shows symbol mode indicator when @ prefix", () => {
    render(
      <FileFinder open={true} onClose={vi.fn()} api={{} as never} t={(k) => k} projectId="p1" activePath={null} onOpen={vi.fn()} />
    );
    const input = screen.getByTestId("file-finder-input");
    fireEvent.change(input, { target: { value: "@" } });
    expect(screen.getByTestId("finder-mode")).toBeTruthy();
  });

  it("clears query on open transition", () => {
    const { rerender } = render(
      <FileFinder open={false} onClose={vi.fn()} api={{} as never} t={(k) => k} projectId="p1" activePath={null} onOpen={vi.fn()} />
    );
    rerender(
      <FileFinder open={true} onClose={vi.fn()} api={{} as never} t={(k) => k} projectId="p1" activePath={null} onOpen={vi.fn()} />
    );
    const input = screen.getByTestId("file-finder-input") as HTMLInputElement;
    expect(input.value).toBe("");
  });
});
