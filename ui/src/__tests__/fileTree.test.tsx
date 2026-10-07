// 懒加载层级文件树 + 项目内重命名 / 删除（§8.1；v1.72 起创建由会话大模型决策）。
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
    // v1.142：行尾按钮移除——改名入口为行右键菜单。
    fireEvent.contextMenu(await screen.findByRole("button", { name: "old.ts" }), {
      clientX: 30,
      clientY: 40,
    });
    fireEvent.click(within(screen.getByTestId("tree-context-menu")).getByText("tree.rename"));
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

  // v1.141：行右键菜单——编辑文件名 / 删除（复用 hover 动作同流程）。
  it("右键文件行弹菜单并可进改名弹窗", async () => {
    treeMock.mockResolvedValue({
      entries: [{ path: "old.ts", name: "old.ts", kind: "file", git_status: "" }],
    });
    fileOpsMock.mockResolvedValue({ results: [{ ok: true, outcome: "renamed" }] });
    render(
      <FileTree
        api={api()}
        t={(key) => key}
        projectId="project-1"
        refreshToken={1}
        onOpenFile={() => {}}
      />
    );
    fireEvent.contextMenu(await screen.findByRole("button", { name: "old.ts" }), {
      clientX: 40,
      clientY: 60,
    });
    const menu = screen.getByTestId("tree-context-menu");
    expect(menu).toBeInTheDocument();
    expect(menu.getAttribute("style")).toContain("left: 40px");
    // 「编辑文件名」打开改名弹窗（预填原名）。
    fireEvent.click(within(menu).getByText("tree.rename"));
    expect(screen.queryByTestId("tree-context-menu")).not.toBeInTheDocument();
    const input = document.getElementById("tree-prompt-input") as HTMLInputElement;
    expect(input.value).toBe("old.ts");
  });

  it("右键文件夹行弹菜单并可删除", async () => {
    treeMock.mockResolvedValue({
      entries: [{ path: "src", name: "src", kind: "dir", git_status: "" }],
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
    fireEvent.contextMenu(await screen.findByRole("button", { name: /src/ }), {
      clientX: 20,
      clientY: 30,
    });
    const menu = screen.getByTestId("tree-context-menu");
    fireEvent.click(within(menu).getByText("tree.delete"));
    await waitFor(() =>
      expect(fileOpsMock).toHaveBeenCalledWith("project-1", [{ op: "delete", path: "src" }])
    );
    expect(confirmSpy).toHaveBeenCalledWith("tree.delete_confirm");
    expect(onOperation).toHaveBeenCalledWith({ type: "deleted", path: "src" });
    expect(screen.queryByTestId("tree-context-menu")).not.toBeInTheDocument();
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
    // v1.142：删除入口为行右键菜单。
    fireEvent.contextMenu(await screen.findByRole("button", { name: "old.ts" }), {
      clientX: 30,
      clientY: 40,
    });
    fireEvent.click(within(screen.getByTestId("tree-context-menu")).getByText("tree.delete"));
    await waitFor(() =>
      expect(fileOpsMock).toHaveBeenCalledWith("project-1", [{ op: "delete", path: "old.ts" }])
    );
    expect(onOperation).toHaveBeenCalledWith({ type: "deleted", path: "old.ts" });
    expect(confirmSpy).toHaveBeenCalledWith("tree.delete_confirm");
  });
});
