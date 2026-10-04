// 懒加载层级文件树 + 项目内重命名 / 删除（§8.1；v1.72 起创建由会话大模型决策）。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FileTree } from "../components/FileTree";
import type { TenonApi } from "../lib/api";

const treeMock = vi.fn();
const fileOpsMock = vi.fn();

function api() {
  return { tree: treeMock, fileOps: fileOpsMock } as unknown as TenonApi;
}

describe("FileTree", () => {
  beforeEach(() => {
    treeMock.mockReset();
    fileOpsMock.mockReset();
  });

  it("refreshToken 变化时重新拉取 watcher 后的文件列表", async () => {
    treeMock.mockResolvedValue({
      entries: [{ path: "a.txt", name: "a.txt", kind: "file", git_status: "modified" }],
    });
    const view = render(
      <FileTree
        api={api()}
        t={(key) => key}
        projectId="project-1"
        refreshToken={1}
        onOpenFile={() => {}}
      />
    );
    expect(await screen.findByText("a.txt")).toBeInTheDocument();
    treeMock.mockClear();
    view.rerender(
      <FileTree
        api={api()}
        t={(key) => key}
        projectId="project-1"
        refreshToken={2}
        onOpenFile={() => {}}
      />
    );
    await waitFor(() => expect(treeMock).toHaveBeenCalledWith("project-1", ""));
  });

  it("目录点击懒加载子级并在事件版本变化后刷新", async () => {
    treeMock.mockImplementation((_projectId: string, path: string) => {
      if (path === "") {
        return Promise.resolve({
          entries: [{ path: "src", name: "src", kind: "dir", git_status: "" }],
        });
      }
      return Promise.resolve({
        entries: [{ path: "src/main.ts", name: "main.ts", kind: "file", git_status: "modified" }],
      });
    });
    render(
      <FileTree
        api={api()}
        t={(key) => key}
        projectId="project-1"
        refreshToken={1}
        onOpenFile={() => {}}
      />
    );
    fireEvent.click(await screen.findByRole("button", { name: /src/ }));
    expect(await screen.findByText("main.ts")).toBeInTheDocument();
    expect(treeMock).toHaveBeenCalledWith("project-1", "src");
  });

  it("重命名走 project-scoped file ops 并回传新旧路径", async () => {
    treeMock.mockResolvedValue({
      entries: [{ path: "old.ts", name: "old.ts", kind: "file", git_status: "" }],
    });
    fileOpsMock.mockResolvedValue({ results: [{ ok: true, outcome: "renamed" }] });
    const onOperation = vi.fn();
    render(
      <FileTree
        api={api()}
        t={(key) => key}
        projectId="project-1"
        refreshToken={1}
        onOpenFile={() => {}}
        onOperation={onOperation}
      />
    );
    fireEvent.click(await screen.findByTitle("tree.rename"));
    const input = document.getElementById("tree-prompt-input") as HTMLInputElement;
    expect(input.value).toBe("old.ts");
    fireEvent.change(input, { target: { value: "new.ts" } });
    fireEvent.click(screen.getByRole("button", { name: "tree.save" }));
    await waitFor(() =>
      expect(fileOpsMock).toHaveBeenCalledWith("project-1", [
        { op: "rename", from: "old.ts", to: "new.ts" },
      ])
    );
    expect(onOperation).toHaveBeenCalledWith({
      type: "renamed",
      from: "old.ts",
      to: "new.ts",
      kind: "file",
    });
  });

  it("删除需确认并回传路径", async () => {
    treeMock.mockResolvedValue({
      entries: [{ path: "old.ts", name: "old.ts", kind: "file", git_status: "" }],
    });
    fileOpsMock.mockResolvedValue({ results: [{ ok: true, outcome: "deleted" }] });
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    const onOperation = vi.fn();
    render(
      <FileTree
        api={api()}
        t={(key) => key}
        projectId="project-1"
        refreshToken={1}
        onOpenFile={() => {}}
        onOperation={onOperation}
      />
    );
    fireEvent.click(await screen.findByTitle("tree.delete"));
    await waitFor(() =>
      expect(fileOpsMock).toHaveBeenCalledWith("project-1", [{ op: "delete", path: "old.ts" }])
    );
    expect(onOperation).toHaveBeenCalledWith({ type: "deleted", path: "old.ts" });
    expect(confirmSpy).toHaveBeenCalledWith("delete old.ts?");
  });
});
