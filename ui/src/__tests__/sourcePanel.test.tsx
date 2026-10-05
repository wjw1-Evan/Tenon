// 右区源码树（§7.2 v1.107）：文件浏览与 git 状态同树、「全部 / 仅变更」过滤、
// 「源码控制」折叠组（分支 / 提交 / blame）；v1.107 自底部 GitSourcePanel 并入。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SourcePanel } from "../components/SourcePanel";
import type { TenonApi } from "../lib/api";

const gitViewMock = vi.fn();
const treeMock = vi.fn();

function api() {
  return { getGitView: gitViewMock, tree: treeMock } as unknown as TenonApi;
}

function gitView() {
  return {
    repository: true,
    branch: "main",
    branches: [{ name: "main", current: true, commit: "abc1234" }],
    changes: [{ path: "src/app.ts", index_status: " ", worktree_status: "M" }],
    commits: [{
      id: "abcdef1234567890",
      short_id: "abcdef1",
      summary: "add source view",
      author: "Dev",
      email: "dev@tenon.local",
      timestamp: 1,
    }],
    blame: {
      path: "src/app.ts",
      lines: [{
        line: 7,
        commit: "abcdef1234567890",
        author: "Dev",
        email: "dev@tenon.local",
        timestamp: 1,
        summary: "add source view",
        content: "export const value = 1;",
      }],
    },
  };
}

beforeEach(() => {
  gitViewMock.mockReset();
  gitViewMock.mockImplementation(() => Promise.resolve(gitView()));
  treeMock.mockReset();
  treeMock.mockResolvedValue({
    entries: [{ path: "src/app.ts", name: "app.ts", kind: "file", git_status: "M" }],
  });
});

describe("SourcePanel（右区合并源码树）", () => {
  it("renders the file tree in all mode and switches to the changes list", async () => {
    const onOpenFile = vi.fn();
    render(
      <SourcePanel
        api={api()}
        t={(key) => key}
        projectId="project-a"
        refreshToken={1}
        activePath="src/app.ts"
        onOpenFile={onOpenFile}
        onFileTreeChange={() => {}}
        onCollapse={() => {}}
      />
    );
    // 全部：文件树渲染（懒加载根层请求 + git 状态装饰交由 FileTree）。
    expect(await screen.findByTestId("file-tree")).toBeInTheDocument();
    expect(screen.getByTestId("source-filter-all").className).toContain("active");

    // 仅变更：working changes 平铺，点击打开文件。
    fireEvent.click(screen.getByTestId("source-filter-changes"));
    expect(await screen.findByTestId("source-changes")).toBeInTheDocument();
    expect(screen.queryByTestId("file-tree")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("src/app.ts"));
    expect(onOpenFile).toHaveBeenCalledWith("src/app.ts");
  });

  it("expands the source control group with branches / commits / blame", async () => {
    const onOpenFile = vi.fn();
    render(
      <SourcePanel
        api={api()}
        t={(key) => key}
        projectId="project-a"
        refreshToken={1}
        activePath="src/app.ts"
        onOpenFile={onOpenFile}
        onFileTreeChange={() => {}}
        onCollapse={() => {}}
      />
    );
    fireEvent.click(await screen.findByTestId("source-control-toggle"));
    expect(await screen.findByTestId("source-branch-main")).toHaveTextContent("main");
    expect(screen.getByText("add source view")).toBeInTheDocument();
    fireEvent.click(screen.getByText("export const value = 1;"));
    expect(onOpenFile).toHaveBeenCalledWith("src/app.ts", 7);
  });

  it("collapses via the header button and renders non-Git projects without sections", async () => {
    const onCollapse = vi.fn();
    const { rerender } = render(
      <SourcePanel
        api={api()}
        t={(key) => key}
        projectId="project-a"
        refreshToken={1}
        activePath={null}
        onOpenFile={() => {}}
        onFileTreeChange={() => {}}
        onCollapse={onCollapse}
      />
    );
    fireEvent.click(screen.getByTestId("source-collapse"));
    expect(onCollapse).toHaveBeenCalledOnce();

    gitViewMock.mockImplementation(() =>
      Promise.resolve({
        repository: false,
        branch: null,
        branches: [],
        changes: [],
        commits: [],
        blame: null,
      })
    );
    rerender(
      <SourcePanel
        api={api()}
        t={(key) => key}
        projectId="project-a"
        refreshToken={2}
        activePath={null}
        onOpenFile={() => {}}
        onFileTreeChange={() => {}}
        onCollapse={onCollapse}
      />
    );
    await waitFor(() => expect(screen.getByText("source.not_git_hint")).toBeInTheDocument());
    expect(screen.queryByTestId("source-filter")).not.toBeInTheDocument();
    expect(screen.queryByTestId("source-branches")).not.toBeInTheDocument();
  });
});
