// v1.109 会话过程流（Codex 形态）：回合分组 / 步骤卡展开与失败默认展开 / 助手 markdown 正文。
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { AgentPanel } from "../components/AgentPanel";
import { renderMarkdownLite } from "../lib/markdownLite";
import type { TenonApi } from "../lib/api";

const models = { models: [], default: "", laya: null };
const t = (key: string) => key;

function mockApi(events: Array<Record<string, unknown>>, status = "idle") {
  return {
    models: vi.fn().mockResolvedValue(models),
    trace: vi.fn().mockResolvedValue({ events, latest_seq: events.length }),
    getSession: vi.fn().mockResolvedValue({ session_id: "s1", status, latest_seq: events.length, outcome: null }),
    sendMessage: vi.fn().mockResolvedValue({}),
    control: vi.fn().mockResolvedValue({ ok: true }),
  } as unknown as TenonApi;
}

describe("会话过程流：回合分组", () => {
  it("user_input 开启回合，后续事件归入该回合", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务一" } },
      { id: 2, seq: 2, type: "command_run", payload: { tool: "bash", args: { command: "cargo test" }, output: { ok: true, content: "ok" } } },
      { id: 3, seq: 3, type: "user_input", payload: { text: "任务二" } },
      { id: 4, seq: 4, type: "decision", payload: { intent: "done" } },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText("任务一");
    expect(screen.getByText("任务二")).toBeTruthy();
    // 两个回合
    expect(screen.getAllByTestId("turn")).toHaveLength(2);
    // 工具步骤卡显示目标摘要（args.command）
    expect(screen.getByText(/cargo test/)).toBeTruthy();
    // decision 渲染为回合 markdown 正文
    expect(screen.getByText("done")).toBeTruthy();
  });

  it("user_input 之前的尾部事件归入引导组（无任务块）", async () => {
    const events = [
      { id: 1, seq: 1, type: "command_run", payload: { tool: "bash" } },
      { id: 2, seq: 2, type: "user_input", payload: { text: "任务" } },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText("任务");
    expect(screen.getAllByTestId("turn")).toHaveLength(2);
    const first = screen.getAllByTestId("turn")[0];
    expect(first.textContent).toContain("bash");
  });

  it("运行态显示执行中 spinner 行与最新决策意图", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      { id: 2, seq: 2, type: "decision", payload: { intent: "分析权限矩阵" } },
    ];
    render(<AgentPanel api={mockApi(events, "executing")} t={t} sessionId="s1" />);
    expect(await screen.findByTestId("turn-running")).toHaveTextContent("分析权限矩阵");
  });

  it("空运行态执行中行回退 thread.running 文案", async () => {
    const events = [{ id: 1, seq: 1, type: "user_input", payload: { text: "任务" } }];
    render(<AgentPanel api={mockApi(events, "sensing")} t={t} sessionId="s1" />);
    expect(await screen.findByTestId("turn-running")).toHaveTextContent("thread.running");
  });
});

describe("会话过程流：工具步骤卡", () => {
  it("点击标题行展开 args 与输出，再点收起", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "command_run",
        payload: {
          tool: "bash",
          args: { command: "cargo test --workspace" },
          output: { ok: true, content: "test result: ok" },
        },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const head = (await screen.findByTestId("turn-step")).querySelector("button")!;
    // 默认折叠：输出不可见
    expect(screen.queryByText("test result: ok")).toBeNull();
    fireEvent.click(head);
    expect(screen.getByText("test result: ok")).toBeTruthy();
    // 展开区含「参数」小节标题与序列化 args
    expect(screen.getByText("thread.args")).toBeTruthy();
    expect(screen.getByText(/"command": "cargo test --workspace"/)).toBeTruthy();
    fireEvent.click(head);
    expect(screen.queryByText("test result: ok")).toBeNull();
  });

  it("失败步骤卡红显并默认展开", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "command_run",
        payload: {
          tool: "bash",
          args: { command: "make" },
          output: { ok: false, content: "error: cannot find Makefile" },
        },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const card = await screen.findByTestId("turn-step");
    expect(card.className).toContain("turn-step-fail");
    expect(screen.getByText("thread.failed")).toBeTruthy();
    expect(screen.getByText("error: cannot find Makefile")).toBeTruthy();
  });

  it("patch_applied 显示 changed_files 目标摘要", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "patch_applied",
        payload: {
          tool: "apply_patch",
          args: { file: "a.rs" },
          output: { ok: true, content: "", changed_files: ["src/lib.rs", "src/main.rs"] },
        },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    expect(await screen.findByText(/src\/lib\.rs \+1/)).toBeTruthy();
  });

  it("超长输出截断并提示", async () => {
    const events = [
      {
        id: 1,
        seq: 1,
        type: "command_run",
        payload: {
          tool: "bash",
          args: { command: "dump" },
          output: { ok: true, content: "x".repeat(5000) },
        },
      },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    fireEvent.click((await screen.findByTestId("turn-step")).querySelector("button")!);
    expect(screen.getByText("thread.output_truncated")).toBeTruthy();
  });

  it("denied 错误显示拒绝语义", async () => {
    const events = [
      { id: 1, seq: 1, type: "error", payload: { denied: "bash", reason: "readonly" } },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    expect(await screen.findByText(/thread\.denied bash · readonly/)).toBeTruthy();
  });

  it("信息类事件降级为轻量行", async () => {
    const events = [
      { id: 1, seq: 1, type: "rollback", payload: {} },
      { id: 2, seq: 2, type: "compaction", payload: {} },
      { id: 3, seq: 3, type: "sensing", payload: {} },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    expect(await screen.findByText(/thread\.rollback/)).toBeTruthy();
    expect(screen.getByText(/thread\.compaction/)).toBeTruthy();
    expect(screen.queryByText(/sensing/)).toBeNull();
  });
});

describe("受限 markdown 渲染器", () => {
  it("渲染标题、列表、粗体与行内代码", () => {
    const html = renderMarkdownLite("# 标题\n\n- 项 **粗** `code`\n\n1. 第一\n2. 第二");
    expect(html).toContain("<h1>标题</h1>");
    expect(html).toContain("<ul><li>项 <strong>粗</strong> <code>code</code></li></ul>");
    expect(html).toContain("<ol><li>第一</li><li>第二</li></ol>");
  });

  it("围栏代码块整体转义", () => {
    const html = renderMarkdownLite("```rust\nlet a = \"<b>\";\n```");
    expect(html).toContain('<pre><code data-lang="rust">');
    expect(html).toContain("&lt;b&gt;");
    expect(html).not.toContain("<b>");
  });

  it("HTML 全量转义，不解析内嵌标签", () => {
    const html = renderMarkdownLite('<script>alert(1)</script> & "quotes"');
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
  });

  it("链接仅接受 http(s) 并转义引号", () => {
    const ok = renderMarkdownLite("[站点](https://example.com/a?b=1&c=2)");
    expect(ok).toContain('<a href="https://example.com/a?b=1&amp;c=2"');
    const bad = renderMarkdownLite("[x](javascript:alert(1))");
    expect(bad).not.toContain("href=\"javascript");
  });

  it("流式未闭合围栏按已收内容成块", () => {
    const html = renderMarkdownLite("前文\n\n```js\nconst a = 1;");
    expect(html).toContain("<pre><code data-lang=\"js\">const a = 1;</code></pre>");
  });

  it("空文本返回空串", () => {
    expect(renderMarkdownLite("  \n ")).toBe("");
  });
});

describe("流式草稿挂活跃回合", () => {
  it("streamText 渲染 markdown 且不产生重复卡", async () => {
    const deltaEvents = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      { id: 2, seq: 2, type: "model_delta", payload: { text: "**加粗** 流式" } },
    ];
    let call = 0;
    const api = {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockImplementation(() => {
        call += 1;
        return Promise.resolve(
          call === 1 ? { events: deltaEvents, latest_seq: 2 } : { events: [], latest_seq: 2 },
        );
      }),
      getSession: vi.fn().mockResolvedValue({ session_id: "s1", status: "deciding", latest_seq: 2, outcome: null }),
    } as unknown as TenonApi;
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    await waitFor(() => expect(screen.getByTestId("model-stream")).toBeTruthy());
    expect(screen.getByTestId("model-stream").querySelector("strong")).toBeTruthy();
    expect(screen.queryAllByTestId("model-stream")).toHaveLength(1);
  });
});
