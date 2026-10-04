// 文件树拖拽移动（§8.1）：drag data → /file/ops move → renamed 回调。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FileTree } from "../components/FileTree";
import type { TenonApi } from "../lib/api";

const treeMock = vi.fn();
const fileOpsMock = vi.fn();

function api() {
  return { tree: treeMock, fileOps: fileOpsMock } as unknown as TenonApi;
}

describe("FileTree drag move", () => {
  beforeEach(() => {
    treeMock.mockReset();
    fileOpsMock.mockReset();
  });

  it("file 可拖入目录并调用 move", async () => {
    treeMock.mockImplementation((_project: string, path: string) => {
      if (path === "") {
        return Promise.resolve({
          entries: [
            { path: "src", name: "src", kind: "dir", git_status: "" },
            { path: "old.ts", name: "old.ts", kind: "file", git_status: "" },
          ],
        });
      }
      return Promise.resolve({ entries: [] });
    });
    fileOpsMock.mockResolvedValue({ results: [{ ok: true, outcome: "moved" }] });
    const onOperation = vi.fn();
    render(
      <FileTree
        api={api()}
        t={(key) => key}
        projectId="p1"
        refreshToken={1}
        onOpenFile={() => {}}
        onOperation={onOperation}
      />
    );
    expect(await screen.findByText("old.ts")).toBeInTheDocument();

    // 触发 HTML5 drag data；jsdom 不完整实现 DataTransfer。
    const data = {
      getData: (type: string) =>
        type === "application/x-tenon-path" ? "old.ts" : "",
    };
    fireEvent.dragStart(screen.getByText("old.ts"), { dataTransfer: data });
    const dirButton = screen.getByRole("button", { name: /src/ });
    fireEvent.dragOver(dirButton, { dataTransfer: data });
    fireEvent.drop(dirButton.closest(".tree-row")!, { dataTransfer: data });

    await waitFor(() =>
      expect(fileOpsMock).toHaveBeenCalledWith("p1", [
        { op: "move", from: "old.ts", to: "src/old.ts" },
      ])
    );
    expect(onOperation).toHaveBeenCalledWith({
      type: "renamed",
      from: "old.ts",
      to: "src/old.ts",
      kind: "file",
    });
  });

  it("根 drop 将嵌套文件移动到根", async () => {
    treeMock.mockImplementation((_project: string, path: string) => {
      if (path === "") {
        return Promise.resolve({
          entries: [{ path: "src", name: "src", kind: "dir", git_status: "" }],
        });
      }
      return Promise.resolve({
        entries: [{ path: "src/nested.ts", name: "nested.ts", kind: "file", git_status: "" }],
      });
    });
    fileOpsMock.mockResolvedValue({ results: [{ ok: true, outcome: "moved" }] });
    const onOperation = vi.fn();
    render(
      <FileTree
        api={api()}
        t={(key) => key}
        projectId="p1"
        refreshToken={1}
        onOpenFile={() => {}}
        onOperation={onOperation}
      />
    );
    fireEvent.click(await screen.findByRole("button", { name: /src/ }));
    const source = await screen.findByText("nested.ts");
    const data = {
      getData: (type: string) =>
        type === "application/x-tenon-path" ? "src/nested.ts" : "",
    };
    fireEvent.dragStart(source, { dataTransfer: data });
    const tree = screen.getByTestId("file-tree");
    fireEvent.dragOver(tree, { dataTransfer: data });
    fireEvent.drop(tree, { dataTransfer: data });
    await waitFor(() =>
      expect(fileOpsMock).toHaveBeenCalledWith("p1", [
        { op: "move", from: "src/nested.ts", to: "nested.ts" },
      ])
    );
  });
});
