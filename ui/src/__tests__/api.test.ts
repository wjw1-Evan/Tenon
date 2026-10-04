import { afterEach, describe, expect, it, vi } from "vitest";
import { TenonApi } from "../lib/api";

const fetchMock = vi.fn();
vi.stubGlobal("fetch", fetchMock);
// WebSocket mock（connectEvents 用）
class FakeWebSocket {
  // 测试桩：记录 send 的帧

  static last: FakeWebSocket | null = null;
  url: string;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((m: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(url: string) {
    this.url = url;
    FakeWebSocket.last = this;
    queueMicrotask(() => this.onopen?.());
  }
  send(data: string) {
    this.sent.push(data);
    queueMicrotask(() => this.onmessage?.({ data: "auth ok" }));
    queueMicrotask(() => this.onmessage?.({ data: JSON.stringify({ type: "user_input" }) }));
  }
}
vi.stubGlobal("WebSocket", FakeWebSocket);

afterEach(() => fetchMock.mockReset());

describe("TenonApi（§15 客户端）", () => {
  it("所有请求携带 X-Tenon-Token 头", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({}), { status: 200 }));
    const api = new TenonApi({ port: 9999, token: "tok-1" });
    await api.getSession("s1");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("http://127.0.0.1:9999/session/s1");
    expect(init.headers["X-Tenon-Token"]).toBe("tok-1");
  });

  it("json 请求设置 Content-Type 并序列化", async () => {
    fetchMock.mockImplementation(async () => new Response("{}", { status: 200 }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.sendMessage("s1", "hello");
    const [, init] = fetchMock.mock.calls[0];
    expect(init.method).toBe("POST");
    expect(init.headers["Content-Type"]).toBe("application/json");
    expect(init.body).toBe(JSON.stringify({ text: "hello" }));
  });

  it("非 2xx 抛出错误信息", async () => {
    fetchMock.mockResolvedValue(new Response("denied", { status: 403 }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await expect(api.models()).rejects.toThrow("API 403");
  });

  it("多项目文件 API 绑定 project_id（§6.4）", async () => {
    fetchMock.mockImplementation(async () => new Response("{}", { status: 200 }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.readFile("project-a", "src/a.txt");
    await api.writeFile("project-b", "src/b.txt", "hello");
    await api.search("project-c", "beta");
    expect(fetchMock.mock.calls[0][0]).toBe(
      "http://127.0.0.1:9999/project/project-a/file?path=src%2Fa.txt"
    );
    expect(fetchMock.mock.calls[1][0]).toBe(
      "http://127.0.0.1:9999/project/project-b/file"
    );
    expect(JSON.parse(fetchMock.mock.calls[1][1].body).path).toBe("src/b.txt");
    expect(fetchMock.mock.calls[2][0]).toBe(
      "http://127.0.0.1:9999/project/project-c/search?q=beta"
    );
  });

  it("removeProject calls DELETE and never implies disk deletion", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ removed: true, disk_contents_deleted: false }), { status: 200 })
    );
    const api = new TenonApi({ port: 9999, token: "t" });
    const result = await api.removeProject("p-remove");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("http://127.0.0.1:9999/projects/p-remove");
    expect(init.method).toBe("DELETE");
    expect(init.headers["X-Tenon-Token"]).toBe("t");
    expect(result.disk_contents_deleted).toBe(false);
  });

  it("组合任务只提交 project-scoped children", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 200 }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.createPortfolioTask("release", [
      { project_id: "p-a", text: "run A" },
      { project_id: "p-b", text: "run B" },
    ]);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("http://127.0.0.1:9999/portfolio-tasks");
    const body = JSON.parse(init.body);
    expect(body.children.map((c: { project_id: string }) => c.project_id)).toEqual([
      "p-a",
      "p-b",
    ]);
  });

  it("权限策略使用专用收窄配置端点", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({}), { status: 200 }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.putTeamPolicy({ force_interactive: true, denied_tools: ["git_push"], max_cost_usd: 2 });
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("http://127.0.0.1:9999/team-policy");
    expect(init.method).toBe("PUT");
    expect(JSON.parse(init.body)).toEqual({
      force_interactive: true,
      denied_tools: ["git_push"],
      max_cost_usd: 2,
    });
  });

  it("更新执行器使用专用检查与应用端点", async () => {
    fetchMock.mockImplementation(async () =>
      new Response(JSON.stringify({}), { status: 200 })
    );
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.checkUpdates();
    await api.applyUpdates();
    expect(fetchMock.mock.calls[0][0]).toBe("http://127.0.0.1:9999/updates/check");
    expect(fetchMock.mock.calls[0][1].method).toBe("POST");
    expect(fetchMock.mock.calls[1][0]).toBe("http://127.0.0.1:9999/updates/apply");
    expect(fetchMock.mock.calls[1][1].method).toBe("POST");
  });

  it("connectEvents 先换票、首帧携带 ticket（ADR-10）", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ ticket: "one-time", expires_in_s: 60 }), { status: 200 })
    );
    const api = new TenonApi({ port: 9999, token: "t" });
    const events: unknown[] = [];
    const ws = (await api.connectEvents((ev) => events.push(ev))) as unknown as FakeWebSocket;
    expect(ws.url).toContain("127.0.0.1:9999/ws");
    // 首帧必须是票据而非头（浏览器 WS 限制，ADR-10）
    expect(ws.sent[0]).toBe("one-time");
    await new Promise((r) => setTimeout(r, 0));
    expect(events).toEqual([{ type: "user_input" }]);
  });
});
