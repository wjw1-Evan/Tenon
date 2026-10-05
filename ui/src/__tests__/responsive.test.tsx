// 视口自适应（v1.74 §7.2）：三档判定 / 渲染期 clamp / narrow 浮层互斥与遮罩收起 / 跨档恢复
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";
import {
  bandOf,
  effectiveBottom,
  effectiveFloatWidth,
  effectiveLeft,
  effectiveRight,
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
    // 右栏 ≤34vw、下限 280
    expect(effectiveRight(420, 1280)).toBe(420);
    expect(effectiveRight(420, 1000)).toBe(340);
    expect(effectiveRight(300, 700)).toBe(280);
    // 底栏 ≤40vh、下限 100
    expect(effectiveBottom(180, 800)).toBe(180);
    expect(effectiveBottom(300, 600)).toBe(240);
    expect(effectiveBottom(80, 400)).toBe(80);
    // 浮层宽 ≤82vw、下限 220
    expect(effectiveFloatWidth(420, 1280)).toBe(420);
    expect(effectiveFloatWidth(600, 500)).toBe(410);
    expect(effectiveFloatWidth(120, 600)).toBe(220);
  });
});

describe("三档布局（§7.2 v1.78）", () => {
  it("宽屏：线程是主区，源码区整体停靠右区（v1.108），关闭即隐藏", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    const workspace = screen.getByTestId("workspace");
    expect(workspace.getAttribute("data-band")).toBe("wide");
    expect(workspace.className).not.toContain("compact");
    expect(screen.queryByTestId("float-backdrop")).toBeNull();
    expect(screen.queryByTestId("editor-float-toggle")).toBeNull();
    expect(document.querySelector(".zone-thread")).not.toBeNull();
    // v1.108：源码区默认开启 → 右区停靠（源码树 + 空编辑器），无需先开 tab。
    expect(document.querySelector(".source-dock")).not.toBeNull();
    expect(document.querySelector(".zone-center")).not.toBeNull();

    // 关闭源码区 → 右区整体隐藏，右缘细条常驻恢复。
    fireEvent.click(screen.getByTestId("source-collapse"));
    await waitFor(() => expect(document.querySelector(".zone-center")).toBeNull());
    fireEvent.click(screen.getByTestId("source-open"));
    await waitFor(() => expect(document.querySelector(".source-dock")).not.toBeNull());
  });

  it("宽屏源码区整体开关（v1.108）：有打开 tab 也整体隐藏，重开即恢复 tab", async () => {
    // 项目打开成功 + ui-state 恢复一个 tab：完整链路。
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
          body = { tabs: ["a.txt"], activePath: "a.txt" };
        } else if (url.includes("/file?")) {
          body = { path: "a.txt", content: "hello", total_bytes: 5 };
        } else if (url.includes("/tree")) {
          body = { entries: [] };
        } else if (url.includes("/session")) {
          body = { session_id: "s1", project_id: "p1" };
        }
        return Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));
      })
    );
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/repo" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    // ui-state 恢复的 tab 使编辑器有内容，源码区停靠。
    await waitFor(() => expect(screen.getByTestId("editor-pane")).toBeTruthy());

    // 关闭源码区：已打开 tab 不阻止整体隐藏，右缘细条常驻。
    fireEvent.click(screen.getByTestId("source-collapse"));
    await waitFor(() => expect(document.querySelector(".zone-center")).toBeNull());
    expect(screen.getByTestId("source-open")).toBeTruthy();

    // 细条重开：tab 状态保留、编辑器恢复。
    fireEvent.click(screen.getByTestId("source-open"));
    await waitFor(() => expect(screen.getByTestId("editor-pane")).toBeTruthy());
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
    backing.setItem("tenon:rightWidth", "500");
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
      const center = document.querySelector<HTMLElement>(".zone-center");
      expect(left?.style.width).toBe("240px");
      expect(thread?.style.flex).toBe("1 1 0%");
      // v1.107：源码树停靠右区，中屏按 clamp 收敛（500 → 34vw=340）。
      expect(center?.style.width).toBe("340px");
    });
    setViewport(1280);
    await waitFor(() => {
      const left = document.querySelector<HTMLElement>(".zone-left");
      expect(left?.style.width).toBe("400px");
    });
    expect(localStorage.getItem("tenon:leftWidth")).toBe("400");
    expect(localStorage.getItem("tenon:rightWidth")).toBe("500");
  });

  it("窄屏编辑器审查窗格浮层（v1.78）：有 tab 时顶栏钮唤出，遮罩收起", async () => {
    // 项目打开成功 + ui-state 恢复一个 tab：编辑器浮层链路完整。
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
          body = { tabs: ["a.txt"], activePath: "a.txt" };
        } else if (url.includes("/file?")) {
          body = { path: "a.txt", content: "hello", total_bytes: 5 };
        } else if (url.includes("/tree")) {
          body = { entries: [] };
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

    // 有打开 tab：切换钮渲染；点击唤出编辑器浮层 + 遮罩；线程仍在流主区。
    const toggle = await screen.findByTestId("editor-float-toggle");
    fireEvent.click(toggle);
    await waitFor(() =>
      expect(document.querySelector(".zone-center")?.className).toContain("zone-float")
    );
    expect(screen.getByTestId("float-backdrop")).toBeTruthy();
    expect(screen.getByTestId("task-input-box")).toBeTruthy();

    // 遮罩点击收起。
    fireEvent.click(screen.getByTestId("float-backdrop"));
    await waitFor(() => expect(document.querySelector(".zone-center")).toBeNull());
    await waitFor(() => expect(screen.queryByTestId("float-backdrop")).toBeNull());
  });
});
