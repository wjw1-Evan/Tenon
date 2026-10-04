// Cmd/Ctrl+P fuzzy finder（§7.4）：防抖查询 / 文件选择 / :line。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FileFinder } from "../components/FileFinder";

import type { TenonApi } from "../lib/api";

const fuzzyMock = vi.fn();
const lspMock = vi.fn();

function api() {
  return { fuzzyFiles: fuzzyMock, lsp: lspMock } as unknown as TenonApi;
}

describe("FileFinder", () => {
  beforeEach(() => {
    fuzzyMock.mockReset();
    lspMock.mockReset();
  });

  it("防抖后请求 fuzzy endpoint，Enter 打开首个文件", async () => {
    fuzzyMock.mockResolvedValue({
      hits: [
        { path: "src/app.ts", name: "app.ts", kind: "file", git_status: "", score: 100 },
        { path: "src/api.ts", name: "api.ts", kind: "file", git_status: "", score: 80 },
      ],
      query: "api",
    });
    const onClose = vi.fn();
    const onOpen = vi.fn();
    render(
      <FileFinder
        open
        onClose={onClose}
        api={api()}
        t={(key) => key}
        projectId="project-1"
        activePath="src/app.ts"
        onOpen={onOpen}
      />
    );
    fireEvent.change(await screen.findByTestId("file-finder-input"), {
      target: { value: "api" },
    });
    await waitFor(() => expect(fuzzyMock).toHaveBeenCalledWith("project-1", "api", 100));
    fireEvent.keyDown(screen.getByTestId("file-finder-input"), { key: "Enter" });
    expect(onOpen).toHaveBeenCalledWith("src/app.ts", undefined);
    expect(onClose).toHaveBeenCalled();
  });

  it("@ 前缀查询 workspace symbols 并按 LSP location 打开", async () => {
    lspMock.mockResolvedValue({
      result: [
        {
          name: "greet",
          detail: "function greet(name: string): string",
          location: {
            uri: "file:///tmp/project/src/util.ts",
            range: { start: { line: 11, character: 16 } },
          },
        },
      ],
    });
    const onOpen = vi.fn();
    render(
      <FileFinder
        open
        onClose={vi.fn()}
        api={api()}
        t={(key) => key}
        projectId="project-1"
        projectRoot="/tmp/project"
        activePath="src/util.ts"
        onOpen={onOpen}
      />
    );
    const input = await screen.findByTestId("file-finder-input");
    fireEvent.change(input, { target: { value: "@greet" } });
    expect(await screen.findByTestId("finder-mode")).toHaveTextContent("finder.mode_symbol");
    expect(await screen.findByText(/greet/)).toBeInTheDocument();
    expect(lspMock).toHaveBeenCalledWith({
      project_id: "project-1",
      path: "src/util.ts",
      action: "workspace_symbol",
      extra: "greet",
    });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onOpen).toHaveBeenCalledWith("src/util.ts", 12);
  });

  it("解析 :line 并传给打开回调", async () => {
    fuzzyMock.mockResolvedValue({
      hits: [{ path: "README.md", name: "README.md", kind: "file", git_status: "", score: 1 }],
      query: "read",
    });
    const onOpen = vi.fn();
    render(
      <FileFinder
        open
        onClose={vi.fn()}
        api={api()}
        t={(key) => key}
        projectId="project-1"
        activePath="README.md"
        onOpen={onOpen}
      />
    );
    const input = await screen.findByTestId("file-finder-input");
    fireEvent.change(input, { target: { value: "read:42" } });
    await waitFor(() => expect(screen.getByText(":42")).toBeInTheDocument());
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onOpen).toHaveBeenCalledWith("README.md", 42);
  });

  it("空结果可见且不可选择", async () => {
    fuzzyMock.mockResolvedValue({ hits: [], query: "missing" });
    render(
      <FileFinder
        open
        onClose={vi.fn()}
        api={api()}
        t={(key) => key}
        projectId="project-1"
        activePath={null}
        onOpen={vi.fn()}
      />
    );
    fireEvent.change(await screen.findByTestId("file-finder-input"), { target: { value: "missing" } });
    expect(await screen.findByText("finder.empty")).toBeInTheDocument();
  });
});
