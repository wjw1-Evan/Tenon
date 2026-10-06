// 视口自适应（v1.74 §7.2）：三档判定 / 渲染期 clamp / narrow 浮层互斥与遮罩收起 / 跨档恢复
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import App from "../App";
import {
  bandOf,
  effectiveBottom,
  effectiveFloatWidth,
  effectiveLeft,
} from "../lib/viewport";

beforeEach(() => {
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });
  vi.stubGlobal("innerWidth", 1280);
  vi.stubGlobal("innerHeight", 800);
  // 旧基底 App 打开未信任项目会弹 TOFU confirm（v1.67 起静默）；桩掉保持两基底可跑
  vi.stubGlobal("confirm", () => true);
  vi.stubGlobal(
    "fetch",
    vi.fn().mockImplementation((input: RequestInfo | URL) => {
      const url = String(input);
      let body: unknown = {};
      if (url.includes("/pairing")) {
        body = { port: 9876, token: "dev" };
      } else if (url.includes("/projects") && !url.includes("open")) {
        body = { projects: [] };
      } else if (url.includes("/settings")) {
        body = {
          session: {
            mode: "interactive",
            first_edit_buffer_ms: 2000,

          },
          exec: { command_timeout_s: 120 },
        };
      } else if (url.includes("/portfolio-tasks")) {
        body = { tasks: [] };
      }
      return Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));
    })
  );
});

function setViewport(width: number, height = 800) {
  vi.stubGlobal("innerWidth", width);
  vi.stubGlobal("innerHeight", height);
  fireEvent.resize(window);
}

describe("档位与 clamp 纯函数（lib/viewport）", () => {
  it("bandOf 边界：<800 narrow，800–1179 middle，≥1180 wide", () => {
    expect(bandOf(799)).toBe("narrow");
    expect(bandOf(800)).toBe("middle");
    expect(bandOf(1179)).toBe("middle");
    expect(bandOf(1180)).toBe("wide");
  });

  it("clamp 只收敛不放大，宽屏原样", () => {
    // 左栏 ≤24vw、下限 160
    expect(effectiveLeft(220, 1280)).toBe(220);
    expect(effectiveLeft(400, 1000)).toBe(240);
    expect(effectiveLeft(400, 600)).toBe(160);
    // 底栏 ≤40vh、下限 100（v1.110 移除右栏）
    expect(effectiveBottom(180, 800)).toBe(180);
    expect(effectiveBottom(300, 600)).toBe(240);
    expect(effectiveBottom(80, 400)).toBe(80);
    // 浮层宽 ≤82vw、下限 220
    expect(effectiveFloatWidth(420, 1280)).toBe(420);
    expect(effectiveFloatWidth(600, 500)).toBe(410);
    expect(effectiveFloatWidth(120, 600)).toBe(220);
  });
});

describe("三档布局（§7.2 v1.110）", () => {
  it("宽屏：线程满宽主区，右区退场，编辑器浮层默认关闭", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    const workspace = screen.getByTestId("workspace");
    expect(workspace.getAttribute("data-band")).toBe("wide");
    expect(workspace.className).not.toContain("compact");
    expect(screen.queryByTestId("float-backdrop")).toBeNull();
    // v1.110：右区（源码树 / 审查窗格）与顶栏编辑器切换钮全部退场。
    expect(document.querySelector(".zone-thread")).not.toBeNull();
    expect(document.querySelector(".zone-center")).toBeNull();
    expect(document.querySelector(".source-dock")).toBeNull();
    expect(screen.queryByTestId("editor-float-toggle")).toBeNull();
    expect(screen.queryByTestId("editor-overlay")).toBeNull();
  });

  it("源码工作台弹出：线程主区不动，✕ 收回对话（v1.138）", async () => {
    // 项目打开成功 + 文件树含 a.txt：点「源码」弹工作台，对话保持可用。
    vi.stubGlobal(
      "fetch",
      vi.fn().mockImplementation((input: RequestInfo | URL) => {
        const url = String(input);
        let body: unknown = {};
        if (url.includes("/pairing")) {
          body = { port: 9876, token: "dev" };
        } else if (url.includes("/ws-ticket")) {
          body = { ticket: "t", expires_in_s: 60 };
        } else if (url.includes("/projects/open")) {
          body = { id: "p1", path: "/tmp/repo", display_name: "repo", trusted: true };
        } else if (url.includes("/projects")) {
          body = {
            projects: [
              {
                id: "p1",
                path: "/tmp/repo",
                display_name: "repo",
                trusted: true,
                sessions: [],
                active_sessions: 0,
                dirty_buffers: 0,
                usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
              },
            ],
          };
        } else if (url.includes("/ui-state")) {
          body = {};
        } else if (url.includes("/file?")) {
          body = { path: "a.txt", content: "hello", total_bytes: 5 };
        } else if (url.includes("/tree")) {
          body = {
            entries: [{ path: "a.txt", name: "a.txt", kind: "file", git_status: "" }],
          };
        } else if (url.includes("/session")) {
          body = { session_id: "s1", project_id: "p1" };
        }
        return Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));
      })
    );
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/repo" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    await waitFor(() => expect(screen.getByTestId("chat-list-p1")).toBeTruthy());
    expect(screen.queryByTestId("source-workbench")).toBeNull();

    // 点「源码」：工作台弹出（文件树 + 内嵌编辑器），线程主区不动（不被替换 / 不隐藏）。
    fireEvent.click(screen.getByTestId("pe-source-p1"));
    await waitFor(() => expect(screen.getByTestId("source-workbench")).toBeTruthy());
    expect(screen.getByTestId("file-tree")).toBeTruthy();
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
    expect(document.querySelector<HTMLElement>(".zone-thread")?.style.display).toBe("");
    // 会话级状态不落盘（v1.137 的 tenon:peView 持久化已撤）。
    expect(localStorage.getItem("tenon:peView")).toBeNull();

    // 单击文件：tab 落在工作台内嵌编辑器（无独立浮层）。
    fireEvent.click(within(screen.getByTestId("file-tree")).getByText("a.txt"));
    await waitFor(() => expect(screen.getByTestId("editor-pane")).toBeTruthy());
    expect(screen.queryByTestId("editor-overlay")).toBeNull();

    // ✕ 收回：弹出层退场，对话原样可见。
    fireEvent.click(screen.getByTestId("source-workbench-close"));
    await waitFor(() => expect(screen.queryByTestId("source-workbench")).toBeNull());
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
  });

  it("窄屏无线程浮层：线程留在文档流，侧栏按需唤出", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    expect(document.querySelector(".zone-thread")).not.toBeNull();

    setViewport(700);
    await waitFor(() =>
      expect(screen.getByTestId("workspace").getAttribute("data-band")).toBe("narrow")
    );
    await waitFor(() => expect(screen.getByTestId("workspace").className).toContain("compact"));
    // 无打开文件时没有编辑器浮层 / 顶栏审查入口；线程持续可见。
    expect(screen.queryByTestId("float-backdrop")).toBeNull();
    expect(screen.queryByTestId("editor-float-toggle")).toBeNull();
    expect(screen.getByTestId("task-input-box")).toBeTruthy();

    // rail 点「项目」：侧栏浮层开；同视图再点收起。
    fireEvent.click(screen.getByTestId("rail-projects"));
    await waitFor(() => expect(document.querySelector(".zone-left")).not.toBeNull());
    expect(screen.getByTestId("float-backdrop")).toBeTruthy();

    fireEvent.click(screen.getByTestId("rail-projects"));
    await waitFor(() => expect(document.querySelector(".zone-left")).toBeNull());
    await waitFor(() => expect(screen.queryByTestId("float-backdrop")).toBeNull());
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
  });

  it("中屏渲染期 clamp：侧栏收敛且不改写记忆值，线程保持弹性", async () => {
    const backing = localStorage as unknown as { setItem: (k: string, v: string) => void };
    backing.setItem("tenon:leftWidth", "400");
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    expect(screen.getByTestId("workspace").getAttribute("data-band")).toBe("wide");
    setViewport(1000);
    await waitFor(() =>
      expect(screen.getByTestId("workspace").getAttribute("data-band")).toBe("middle")
    );
    await waitFor(() => {
      const left = document.querySelector<HTMLElement>(".zone-left");
      const thread = document.querySelector<HTMLElement>(".zone-thread");
      expect(left?.style.width).toBe("240px");
      expect(thread?.style.flex).toBe("1 1 0%");
      // v1.110：右区退场，线程满宽（无 zone-center）。
      expect(document.querySelector(".zone-center")).toBeNull();
    });
    setViewport(1280);
    await waitFor(() => {
      const left = document.querySelector<HTMLElement>(".zone-left");
      expect(left?.style.width).toBe("400px");
    });
    expect(localStorage.getItem("tenon:leftWidth")).toBe("400");
  });

  it("窄屏：源码工作台同样弹出，线程与侧栏浮层不受影响（v1.138）", async () => {
    // 项目打开成功 + 文件树含 a.txt：窄屏下工作台仍为弹出层（与视口无关）。
    vi.stubGlobal(
      "fetch",
      vi.fn().mockImplementation((input: RequestInfo | URL) => {
        const url = String(input);
        let body: unknown = {};
        if (url.includes("/pairing")) {
          body = { port: 9876, token: "dev" };
        } else if (url.includes("/ws-ticket")) {
          body = { ticket: "t", expires_in_s: 60 };
        } else if (url.includes("/projects/open")) {
          body = { id: "p1", path: "/tmp/repo", display_name: "repo", trusted: true };
        } else if (url.includes("/projects")) {
          body = {
            projects: [
              {
                id: "p1",
                path: "/tmp/repo",
                display_name: "repo",
                trusted: true,
                sessions: [],
                active_sessions: 0,
                dirty_buffers: 0,
                usage: { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
              },
            ],
          };
        } else if (url.includes("/ui-state")) {
          body = {};
        } else if (url.includes("/file?")) {
          body = { path: "a.txt", content: "hello", total_bytes: 5 };
        } else if (url.includes("/tree")) {
          body = {
            entries: [{ path: "a.txt", name: "a.txt", kind: "file", git_status: "" }],
          };
        } else if (url.includes("/session")) {
          body = { session_id: "s1", project_id: "p1" };
        }
        return Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));
      })
    );
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/repo" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    setViewport(700);
    await waitFor(() =>
      expect(screen.getByTestId("workspace").getAttribute("data-band")).toBe("narrow")
    );

    // 侧栏浮层唤出 → 点「源码」：工作台弹出，线程留在文档流（输入框仍在）。
    fireEvent.click(screen.getByTestId("rail-projects"));
    await waitFor(() => expect(screen.getByTestId("float-backdrop")).toBeTruthy());
    fireEvent.click(screen.getByTestId("pe-source-p1"));
    await waitFor(() => expect(screen.getByTestId("source-workbench")).toBeTruthy());
    expect(screen.getByTestId("file-tree")).toBeTruthy();
    expect(screen.getByTestId("task-input-box")).toBeTruthy();

    // 单击文件落内嵌编辑器（无独立浮层）；✕ 收回后侧栏浮层与遮罩不受影响。
    fireEvent.click(within(screen.getByTestId("file-tree")).getByText("a.txt"));
    await waitFor(() => expect(screen.getByTestId("editor-pane")).toBeTruthy());
    expect(screen.queryByTestId("editor-overlay")).toBeNull();
    fireEvent.click(screen.getByTestId("source-workbench-close"));
    await waitFor(() => expect(screen.queryByTestId("source-workbench")).toBeNull());
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
    expect(screen.getByTestId("float-backdrop")).toBeTruthy();
  });
});
