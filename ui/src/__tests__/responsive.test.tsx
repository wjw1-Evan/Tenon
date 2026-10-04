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
            approval_timeout_s: 120,
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

describe("三档布局（§7.2 v1.74）", () => {
  it("宽屏：四区并排，无浮层与顶栏切换钮", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    const workspace = screen.getByTestId("workspace");
    expect(workspace.getAttribute("data-band")).toBe("wide");
    expect(workspace.className).not.toContain("compact");
    expect(screen.queryByTestId("float-backdrop")).toBeNull();
    expect(screen.queryByTestId("agent-float-toggle")).toBeNull();
    expect(document.querySelector(".zone-left")).not.toBeNull();
  });

  it("跨入窄屏：代理面板转浮层默认展开，侧栏收起，分屏把手隐藏", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    expect(document.querySelector(".zone-left")).not.toBeNull();

    setViewport(700);
    await waitFor(() =>
      expect(screen.getByTestId("workspace").getAttribute("data-band")).toBe("narrow")
    );
    // 转档由 effect 落浮层默认态（第二跳 commit），等它落地
    await waitFor(() => expect(screen.getByTestId("float-backdrop")).toBeTruthy());
    const workspace = screen.getByTestId("workspace");
    expect(workspace.className).toContain("compact");
    expect(screen.getByTestId("agent-float-toggle")).toBeTruthy();
    // 代理浮层默认展开且带浮层类；侧栏不占位
    expect(screen.getByTestId("task-input-box")).toBeTruthy();
    expect(document.querySelector(".zone-right")?.className).toContain("zone-float");
    expect(document.querySelector(".zone-left")).toBeNull();
  });

  it("rail 切侧栏浮层（与代理互斥），遮罩点击收起，顶栏钮唤回代理", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    setViewport(700);
    await waitFor(() => expect(screen.getByTestId("float-backdrop")).toBeTruthy());

    // rail 点「项目」：侧栏浮层开、代理浮层收（互斥）
    fireEvent.click(screen.getByTestId("rail-projects"));
    await waitFor(() => expect(document.querySelector(".zone-left")).not.toBeNull());
    await waitFor(() => expect(screen.queryByTestId("task-input-box")).toBeNull());
    expect(screen.getByTestId("float-backdrop")).toBeTruthy();

    // 同视图再点：收起（无浮层）
    fireEvent.click(screen.getByTestId("rail-projects"));
    await waitFor(() => expect(document.querySelector(".zone-left")).toBeNull());
    await waitFor(() => expect(screen.queryByTestId("float-backdrop")).toBeNull());

    // 遮罩收起后经顶栏钮唤回代理浮层
    fireEvent.click(screen.getByTestId("agent-float-toggle"));
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    fireEvent.click(screen.getByTestId("float-backdrop"));
    await waitFor(() => expect(screen.queryByTestId("task-input-box")).toBeNull());
    await waitFor(() => expect(screen.queryByTestId("float-backdrop")).toBeNull());
  });

  it("中屏渲染期 clamp：宽度按视口收敛且不改写记忆值", async () => {
    const backing = localStorage as unknown as { setItem: (k: string, v: string) => void };
    backing.setItem("tenon:leftWidth", "400");
    backing.setItem("tenon:rightWidth", "500");
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => expect(screen.getByTestId("task-input-box")).toBeTruthy());
    expect(screen.getByTestId("workspace").getAttribute("data-band")).toBe("wide");
    // 1000px 视口：左 ≤240、右 ≤340
    setViewport(1000);
    await waitFor(() =>
      expect(screen.getByTestId("workspace").getAttribute("data-band")).toBe("middle")
    );
    await waitFor(() => {
      const left = document.querySelector<HTMLElement>(".zone-left");
      const right = document.querySelector<HTMLElement>(".zone-right");
      expect(left?.style.width).toBe("240px");
      // v1.64：无打开文件时代理区是弹性主区，不使用右栏记忆宽。
      expect(right?.style.width).toBe("");
      expect(right?.style.flex).toBe("1 1 0%");
    });
    // 记忆值不被改写：回宽屏原样恢复
    setViewport(1280);
    await waitFor(() => {
      const left = document.querySelector<HTMLElement>(".zone-left");
      const right = document.querySelector<HTMLElement>(".zone-right");
      expect(left?.style.width).toBe("400px");
      expect(right?.style.width).toBe("");
    });
    expect(localStorage.getItem("tenon:leftWidth")).toBe("400");
    expect(localStorage.getItem("tenon:rightWidth")).toBe("500");
  });
});
