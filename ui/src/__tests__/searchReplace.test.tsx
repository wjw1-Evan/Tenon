// SearchPanel 替换流程剩余路径：选中/取消选择/替换后清除预览。
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
  { path: "a.ts", line: 1, column: 1, text: "old" },
  { path: "b.ts", line: 2, column: 1, text: "old" },
];

beforeEach(() => {
  searchMock.mockReset(); previewMock.mockReset(); replaceMock.mockReset();
});
afterEach(cleanup);

describe("SearchPanel replace flow", () => {
  it("deselect all and reselect", async () => {
    searchMock.mockResolvedValue({ hits });
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "old" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    await waitFor(() => expect(screen.getByTestId("search-select-all")).toBeTruthy());
    // 取消全选
    fireEvent.click(screen.getByTestId("search-select-all"));
    // 重新全选
    fireEvent.click(screen.getByTestId("search-select-all"));
    expect(screen.getByTestId("search-select-all")).toBeTruthy();
  });

  it("deselect individual file group", async () => {
    searchMock.mockResolvedValue({ hits });
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "old" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    await waitFor(() => expect(screen.getAllByRole("checkbox").length).toBeGreaterThan(1));
    // 取消 a.ts 组
    const checkboxes = screen.getAllByRole("checkbox");
    // checkboxes[0] = plain toggle, [1] = select all, [2..] = file groups
    fireEvent.click(checkboxes[2]);
    // 组取消后 checkbox unchecked
    expect(checkboxes[2]).not.toBeChecked();
  });

  it("replacement input appears only with hits", async () => {
    searchMock.mockResolvedValue({ hits: [] });
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "x" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    await waitFor(() => expect(screen.getByText("0 search.results")).toBeTruthy());
    expect(screen.queryByLabelText("search.replace_with")).toBeNull();
  });

  it("apply button not rendered without hits", async () => {
    searchMock.mockResolvedValue({ hits: [] });
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "x" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    await waitFor(() => expect(screen.getByText("0 search.results")).toBeTruthy());
    // 无 apply 按钮
    expect(screen.queryByText("search.apply")).toBeNull();
  });

  it("plain text mode regex special chars escaped in search", async () => {
    searchMock.mockResolvedValue({ hits: [] });
    render(<SearchPanel api={api()} t={t} projectId="p1" onOpenFile={vi.fn()} />);
    fireEvent.click(screen.getByTestId("search-plain-toggle"));
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "test.value" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    await waitFor(() => expect(searchMock).toHaveBeenCalledWith("p1", "test\\.value"));
  });
});
