// TenonApi 端点覆盖：会话/文件/LSP/插件/回滚/模型切换/设置/UI 偏好。
import { afterEach, describe, expect, it, vi } from "vitest";
import { TenonApi } from "../lib/api";

const fetchMock = vi.fn();
vi.stubGlobal("fetch", fetchMock);

afterEach(() => fetchMock.mockReset());

function ok(data: unknown = {}) {
  return new Response(JSON.stringify(data), { status: 200 });
}

describe("TenonApi 端点补测", () => {
  it("getUiPrefs 失败回退空对象", async () => {
    fetchMock.mockRejectedValue(new Error("offline"));
    const api = new TenonApi({ port: 9999, token: "t" });
    expect(await api.getUiPrefs()).toEqual({});
  });

  it("setUiPrefs fire-and-forget 不抛错", async () => {
    fetchMock.mockRejectedValue(new Error("offline"));
    const api = new TenonApi({ port: 9999, token: "t" });
    expect(() => api.setUiPrefs({ theme: "dark" })).not.toThrow();
  });

  it("getSettings / putSettings 端点正确", async () => {
    fetchMock.mockImplementation(async () => ok({ theme: "dark" }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.getSettings();
    expect(fetchMock.mock.calls[0][0]).toBe("http://127.0.0.1:9999/settings");
    await api.putSettings({ theme: "light" });
    expect(fetchMock.mock.calls[1][1].method).toBe("PUT");
    expect(JSON.parse(fetchMock.mock.calls[1][1].body)).toEqual({ theme: "light" });
  });

  it("getUpdates 端点", async () => {
    fetchMock.mockImplementation(async () => ok({ current_version: "1.0" }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.getUpdates();
    expect(fetchMock.mock.calls[0][0]).toBe("http://127.0.0.1:9999/updates");
  });

  it("listProjects GET /projects", async () => {
    fetchMock.mockImplementation(async () => ok({ projects: [] }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.listProjects();
    expect(fetchMock.mock.calls[0][0]).toBe("http://127.0.0.1:9999/projects");
  });

  it("openProject POST /projects/open", async () => {
    fetchMock.mockImplementation(async () => ok({ id: "p1", path: "/x", display_name: "x", trusted: false }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.openProject("/x");
    expect(fetchMock.mock.calls[0][1].method).toBe("POST");
  });

  it("setTrust PUT /project/trust", async () => {
    fetchMock.mockImplementation(async () => ok({ ok: true }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.setTrust("p1", true);
    expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toEqual({ project_id: "p1", trusted: true });
  });

  it("projectUiState 失败回退空对象", async () => {
    fetchMock.mockRejectedValue(new Error("404"));
    const api = new TenonApi({ port: 9999, token: "t" });
    expect(await api.projectUiState("p1")).toEqual({});
  });

  it("saveProjectUiState PUT fire-and-forget", async () => {
    fetchMock.mockRejectedValue(new Error("offline"));
    const api = new TenonApi({ port: 9999, token: "t" });
    expect(() => api.saveProjectUiState("p1", { sessionId: "s1" })).not.toThrow();
  });

  it("createSession 带 worktree 参数", async () => {
    fetchMock.mockImplementation(async () => ok({ session_id: "s1", project_id: "p1" }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.createSession("p1", "openai", "managed");
    const body = JSON.parse(fetchMock.mock.calls[0][1].body);
    expect(body).toEqual({ project_id: "p1", provider: "openai", worktree: "managed" });
  });

  it("control POST /session/:id/control", async () => {
    fetchMock.mockImplementation(async () => ok({ ok: true }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.control("s1", "stop");
    expect(fetchMock.mock.calls[0][0]).toBe("http://127.0.0.1:9999/session/s1/control");
  });

  it("mergeWorktreeSession POST", async () => {
    fetchMock.mockImplementation(async () => ok({ merged: [], skipped: [], conflicts: [] }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.mergeWorktreeSession("s1");
    expect(fetchMock.mock.calls[0][0]).toContain("/worktree/merge");
    expect(fetchMock.mock.calls[0][1].method).toBe("POST");
  });

  it("discardWorktreeSession POST with confirm", async () => {
    fetchMock.mockImplementation(async () => ok({ discarded: true }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.discardWorktreeSession("s1", true);
    expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toEqual({ confirm: true });
  });

  it("trace 带 after 游标", async () => {
    fetchMock.mockImplementation(async () => ok({ events: [], latest_seq: 0 }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.trace("s1", 42);
    expect(fetchMock.mock.calls[0][0]).toContain("after=42");
  });

  it("checkpoints GET", async () => {
    fetchMock.mockImplementation(async () => ok({ checkpoints: [] }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.checkpoints("s1");
    expect(fetchMock.mock.calls[0][0]).toContain("/checkpoints");
  });

  it("rollbackCheckpoint POST", async () => {
    fetchMock.mockImplementation(async () => ok({ rolled_back: [] }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.rollbackCheckpoint("cp1");
    expect(fetchMock.mock.calls[0][0]).toContain("/checkpoint/cp1/rollback");
  });

  it("tree / fuzzyFiles 带编码查询", async () => {
    fetchMock.mockImplementation(async () => ok({ entries: [], hits: [] }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.tree("p1", "src/app.ts");
    expect(fetchMock.mock.calls[0][0]).toContain("path=src%2Fapp.ts");
    await api.fuzzyFiles("p1", "app.ts", 20);
    expect(fetchMock.mock.calls[1][0]).toContain("q=app.ts&limit=20");
  });

  it("fileOps POST", async () => {
    fetchMock.mockImplementation(async () => ok({ results: [{ ok: true }] }));
    const api = new TenonApi({ port: 9999, token: "t" });
    const ops = [{ op: "delete" as const, path: "old.ts" }];
    await api.fileOps("p1", ops);
    expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toEqual({ ops });
  });

  it("l4Stats / l4Rebuild", async () => {
    fetchMock.mockImplementation(async () => ok({ chunks: 0 }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.l4Stats("p1");
    expect(fetchMock.mock.calls[0][0]).toContain("/l4/stats");
    await api.l4Rebuild("p1");
    expect(fetchMock.mock.calls[1][1].method).toBe("POST");
  });

  it("searchPreview / applySearchReplace", async () => {
    fetchMock.mockImplementation(async () => ok({ previews: [] }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.searchPreview("p1", "old", "new");
    expect(fetchMock.mock.calls[0][0]).toContain("/search?q=old&replace=new");
    await api.applySearchReplace("p1", "old", "new", ["a.ts"]);
    expect(fetchMock.mock.calls[1][0]).toContain("/search/replace");
  });

  it("lsp POST /project/:id/lsp", async () => {
    fetchMock.mockImplementation(async () => ok({ result: {} }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.lsp({ project_id: "p1", path: "a.ts", action: "diagnostics" });
    expect(fetchMock.mock.calls[0][0]).toContain("/lsp");
  });

  it("inlineComplete POST", async () => {
    fetchMock.mockImplementation(async () => ok({ completion: "code", provider: "mock" }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.inlineComplete({
      project_id: "p1",
      session_id: "s1",
      path: "a.ts",
      prefix: "const ",
      suffix: "",
    });
    expect(fetchMock.mock.calls[0][0]).toContain("/inline-complete");
  });

  it("getHighlights / getGitView", async () => {
    fetchMock.mockImplementation(async () => ok({}));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.getHighlights("p1", "a.ts");
    expect(fetchMock.mock.calls[0][0]).toContain("/highlight?path=a.ts");
    await api.getGitView("p1", "a.ts");
    expect(fetchMock.mock.calls[1][0]).toContain("path=a.ts");
  });

  it("applyLspEdit POST", async () => {
    fetchMock.mockImplementation(async () => ok({}));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.applyLspEdit("p1", { changes: {} }, "s1");
    expect(fetchMock.mock.calls[0][0]).toContain("/lsp/apply");
  });

  it("evalRuns", async () => {
    fetchMock.mockImplementation(async () => ok({}));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.evalRuns();
    expect(fetchMock.mock.calls[0][0]).toContain("/evals");
  });

  it("modelSuggest / switchSessionModel", async () => {
    fetchMock.mockImplementation(async () => ok({}));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.modelSuggest("write code");
    expect(fetchMock.mock.calls[0][0]).toContain("/model-suggest");
    await api.switchSessionModel("s1", "anthropic", "claude-3");
    expect(JSON.parse(fetchMock.mock.calls[1][1].body)).toEqual({ provider: "anthropic", model: "claude-3" });
  });

  it("detectLanguagePacks / installLanguagePack", async () => {
    fetchMock.mockImplementation(async () => ok({ packs: [] }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.detectLanguagePacks("p1");
    expect(fetchMock.mock.calls[0][0]).toContain("/language-packs");
    await api.installLanguagePack("p1", "typescript");
    expect(JSON.parse(fetchMock.mock.calls[1][1].body)).toEqual({ pack: "typescript" });
  });

  it("putBuffer / clearBuffer", async () => {
    fetchMock.mockImplementation(async () => ok({ ok: true }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.putBuffer("p1", "a.ts", "content");
    expect(fetchMock.mock.calls[0][1].method).toBe("PUT");
    await api.clearBuffer("p1", "a.ts");
    expect(fetchMock.mock.calls[1][1].method).toBe("DELETE");
  });

  it("costs 带可选 sessionId", async () => {
    fetchMock.mockImplementation(async () => ok({}));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.costs();
    expect(fetchMock.mock.calls[0][0]).toBe("http://127.0.0.1:9999/costs");
    await api.costs("s1");
    expect(fetchMock.mock.calls[1][0]).toContain("session=s1");
  });

  it("listPlugins / searchPlugins / installPlugin", async () => {
    fetchMock.mockImplementation(async () => ok({}));
    const api = new TenonApi({ port: 9999, token: "t" });
    await api.listPlugins();
    expect(fetchMock.mock.calls[0][1]?.method ?? "GET").toBeTruthy();
    await api.searchPlugins("lsp");
    await api.installPlugin(
      { id: "lsp-1", version: "1.0", sha256: "0".repeat(64), signature: "sig", url: "https://x/m.tgz" },
      []
    );
    expect(fetchMock.mock.calls[2][1].method).toBe("POST");
  });

  it("wsTicket GET /ws-ticket", async () => {
    fetchMock.mockImplementation(async () => ok({ ticket: "tk", expires_in_s: 60 }));
    const api = new TenonApi({ port: 9999, token: "t" });
    expect(await api.wsTicket()).toBe("tk");
  });

  it("daemonErrorMessage：非 JSON 403 返回原文", async () => {
    fetchMock.mockResolvedValue(new Response("plain text error", { status: 403 }));
    const api = new TenonApi({ port: 9999, token: "t" });
    await expect(api.models()).rejects.toThrow("plain text error");
  });

  it("daemonErrorMessage：JSON error 字段优先", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ error: "specific reason" }), { status: 400 })
    );
    const api = new TenonApi({ port: 9999, token: "t" });
    await expect(api.models()).rejects.toThrow("specific reason");
  });
});
