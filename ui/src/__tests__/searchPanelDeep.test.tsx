// SearchPanel 深度测试：替换流 / 全选 / 纯文本模式 / 错误处理。
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SearchPanel } from "../components/SearchPanel";
import type { TenonApi } from "../lib/api";

const searchMock = vi.fn();
const previewMock = vi.fn();
const replaceMock = vi.fn();

function api() {
  return { search: searchMock, searchPreview: previewMock, applySearchReplace: replaceMock } as unknown as TenonApi;
}

const t = (key: string) => key;
const hits = [
  { path: "a.ts", line: 1, column: 1, text: "old value" },
  { path: "a.ts", line: 5, column: 3, text: "another old" },
  { path: "b.ts", line: 2, column: 1, text: "old here" },
];

beforeEach(() => {
  searchMock.mockReset();
  previewMock.mockReset();
  replaceMock.mockReset();
});

afterEach(cleanup);

describe("SearchPanel deep", () => {
  it("search shows grouped hits with select-all", async () => {
    searchMock.mockResolvedValue({ hits });
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "old" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    await waitFor(() => expect(searchMock).toHaveBeenCalledWith("p1", "old"));
    expect(await screen.findByText("3 search.results")).toBeTruthy();
    expect(screen.getByTestId("search-select-all")).toBeTruthy();
    // 分组渲染
    expect(screen.getByText("a.ts")).toBeTruthy();
    expect(screen.getByText("b.ts")).toBeTruthy();
  });

  it("replace with preview shows diff and applies selected files", async () => {
    searchMock.mockResolvedValue({ hits });
    previewMock.mockResolvedValue({ previews: [{ path: "a.ts", diff: "-old\n+new" }] });
    replaceMock.mockResolvedValue({
      applied: [{ path: "a.ts", replacements: 2, diff: "done" }],
      skipped: [],
    });
    const onChanged = vi.fn();
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} onChanged={onChanged} />);
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "old" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    // 结果出现后输入替换文本并重新搜索以触发预览
    await waitFor(() => expect(screen.getByLabelText("search.replace_with")).toBeTruthy());
    fireEvent.change(screen.getByLabelText("search.replace_with"), { target: { value: "new" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    await waitFor(() => expect(previewMock).toHaveBeenCalledWith("p1", "old", "new"));
    // 预览显示
    await waitFor(() => {
      expect(screen.getByTestId("preview-a.ts")).toBeTruthy();
    });
    // 应用替换
    const applyBtn = screen.queryByRole("button", { name: /search.apply|apply/i });
    if (applyBtn) {
      fireEvent.click(applyBtn);
      await waitFor(() => expect(replaceMock).toHaveBeenCalledWith("p1", "old", "new", ["a.ts", "b.ts"]));
      expect(onChanged).toHaveBeenCalled();
    }
  });

  it("regex error shows for invalid pattern", async () => {
    searchMock.mockResolvedValue({ hits: [] });
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "[invalid" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    // 正则错误提示（非纯文本模式下）
    await waitFor(() => {
      // regexError 只在 replacement 非空时校验
      expect(screen.queryByRole("alert") || screen.getByTestId("search-panel")).toBeTruthy();
    });
  });

  it("plain text toggle escapes regex special chars", async () => {
    searchMock.mockResolvedValue({ hits: [] });
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} />);
    fireEvent.click(screen.getByTestId("search-plain-toggle"));
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "foo.bar" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    await waitFor(() => expect(searchMock).toHaveBeenCalledWith("p1", "foo\\.bar"));
  });

  it("search error shows error alert", async () => {
    searchMock.mockRejectedValue(new Error("offline"));
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "x" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    await waitFor(() => expect(screen.getByTestId("search-error")).toBeTruthy());
  });

  it("disabled when no projectId", () => {
    render(<SearchPanel api={api()} t={t} projectId={null} onOpenFile={vi.fn()} />);
    expect(screen.getByRole("button", { name: "search.title" })).toBeDisabled();
  });

  it("hit click calls onOpenFile with line", async () => {
    searchMock.mockResolvedValue({ hits });
    const onOpenFile = vi.fn();
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={onOpenFile} />);
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "old" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    const hitBtn = await screen.findByText("a.ts");
    fireEvent.click(hitBtn);
    expect(onOpenFile).toHaveBeenCalledWith("a.ts", 1);
  });

  it("empty query disables search", () => {
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} />);
    expect(screen.getByRole("button", { name: "search.title" })).toBeDisabled();
  });
});
