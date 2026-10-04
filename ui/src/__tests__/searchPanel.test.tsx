// 全局搜索 / 替换（§8.1）：hit 跳转、预览、选定应用、dirty buffer 拒绝。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SearchPanel } from "../components/SearchPanel";
import type { TenonApi } from "../lib/api";

const searchMock = vi.fn();
const previewMock = vi.fn();
const applyMock = vi.fn();

function api() {
  return { search: searchMock, searchPreview: previewMock, applySearchReplace: applyMock } as unknown as TenonApi;
}

describe("SearchPanel", () => {
  beforeEach(() => {
    searchMock.mockReset();
    previewMock.mockReset();
    applyMock.mockReset();
  });

  it("搜索 hit 可按行跳转", async () => {
    searchMock.mockResolvedValue({
      hits: [{ path: "src/app.ts", line: 12, column: 3, text: "alpha beta" }],
    });
    const onOpenFile = vi.fn();
    render(
      <SearchPanel api={api()} t={(key) => key} projectId="p1" onOpenFile={onOpenFile} />
    );
    fireEvent.change(screen.getByLabelText("search.query"), { target: { value: "beta" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    expect(await screen.findByText("src/app.ts")).toBeInTheDocument();
    fireEvent.click(screen.getByText("12:3"));
    expect(onOpenFile).toHaveBeenCalledWith("src/app.ts", 12);
  });

  it("替换前展示 diff，应用后回传并请求刷新", async () => {
    searchMock.mockResolvedValue({
      hits: [
        { path: "a.txt", line: 1, column: 1, text: "alpha beta" },
        { path: "b.txt", line: 2, column: 1, text: "beta" },
      ],
    });
    previewMock.mockResolvedValue({
      previews: [
        { path: "a.txt", diff: "-alpha beta\n+alpha BETA" },
        { path: "b.txt", diff: "-beta\n+BETA" },
      ],
    });
    applyMock.mockResolvedValue({
      applied: [{ path: "a.txt", replacements: 1, diff: "-alpha beta\n+alpha BETA" }],
      skipped: [{ path: "b.txt", reason: "dirty buffer" }],
    });
    const onChanged = vi.fn();
    render(
      <SearchPanel
        api={api()}
        t={(key) => key}
        projectId="p1"
        onOpenFile={vi.fn()}
        onChanged={onChanged}
      />
    );
    fireEvent.change(await screen.findByLabelText("search.query"), { target: { value: "beta" } });
    fireEvent.change(screen.getByLabelText("search.replace_with"), { target: { value: "BETA" } });
    fireEvent.click(screen.getByRole("button", { name: "search.title" }));
    await waitFor(() => expect(previewMock).toHaveBeenCalled());
    expect(await screen.findByTestId("preview-a.txt")).toHaveTextContent("+alpha BETA");
    fireEvent.click(await screen.findByTestId("search-apply"));
    await waitFor(() =>
      expect(applyMock).toHaveBeenCalledWith("p1", "beta", "BETA", ["a.txt", "b.txt"])
    );
    expect(onChanged).toHaveBeenCalled();
  });
});
