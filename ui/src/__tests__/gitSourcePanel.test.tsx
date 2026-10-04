// Git source view（§8.1）：branch / changes / commits / inline blame。
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { GitSourcePanel } from "../components/GitSourcePanel";
import type { TenonApi } from "../lib/api";

const gitViewMock = vi.fn();

function api() {
  return { getGitView: gitViewMock } as unknown as TenonApi;
}

beforeEach(() => {
  gitViewMock.mockReset();
  gitViewMock.mockResolvedValue({
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
  });
});

describe("GitSourcePanel", () => {
  it("requests active-file blame and renders branches / changes / commits / blame", async () => {
    const onOpenFile = vi.fn();
    render(
      <GitSourcePanel
        api={api()}
        t={(key) => key}
        projectId="project-a"
        activePath="src/app.ts"
        refreshToken={1}
        onOpenFile={onOpenFile}
      />
    );
    await waitFor(() => expect(gitViewMock).toHaveBeenCalledWith("project-a", "src/app.ts"));
    expect(await screen.findByTestId("source-branch-main")).toHaveTextContent("main");
    expect(screen.getByText("src/app.ts")).toBeInTheDocument();
    expect(screen.getByText("add source view")).toBeInTheDocument();
    fireEvent.click(screen.getByText("export const value = 1;"));
    expect(onOpenFile).toHaveBeenCalledWith("src/app.ts", 7);
  });

  it("renders a non-Git project without change sections", async () => {
    gitViewMock.mockResolvedValue({
      repository: false,
      branch: null,
      branches: [],
      changes: [],
      commits: [],
      blame: null,
    });
    render(
      <GitSourcePanel
        api={api()}
        t={(key) => key}
        projectId="project-a"
        activePath={null}
        refreshToken={1}
        onOpenFile={() => {}}
      />
    );
    expect(await screen.findByText("source.not_git")).toBeInTheDocument();
    expect(screen.queryByTestId("source-changes")).not.toBeInTheDocument();
  });
});
