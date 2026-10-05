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
    // 用户消息气泡独立于 AI 内容容器（turn-body）之外，按侧区分
    const turn1 = screen.getAllByTestId("turn")[0];
    const userBubble = turn1.querySelector('[data-testid="turn-user"]')!;
    expect(userBubble).toBeTruthy();
    expect(userBubble.querySelector(".turn-task")!.textContent).toBe("任务一");
    expect(turn1.querySelector(".turn-body")!.contains(userBubble)).toBe(false);
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

  // v1.131 回合模型标注：气泡下按钮组行尾显示回合内首个 decision 的 model；
  // 无 model 的历史事件（旧档案 / 系统回合）不显示。
  it("气泡下标注该回合所用模型，无 model 事件不标注", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务一" } },
      { id: 2, seq: 2, type: "decision", payload: { intent: "答案一", model: "glm-5.3" } },
      { id: 3, seq: 3, type: "user_input", payload: { text: "任务二" } },
      { id: 4, seq: 4, type: "decision", payload: { intent: "答案二" } },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await screen.findByText("答案二");
    expect(screen.getByTestId("turn-model-1").textContent).toBe("glm-5.3");
    expect(screen.queryByTestId("turn-model-3")).toBeNull();
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

  it("信息行仅保留回滚语义（v1.112 降噪：压缩 / 记忆 / 降级不显示）", async () => {
    const events = [
      { id: 1, seq: 1, type: "rollback", payload: {} },
      { id: 2, seq: 2, type: "compaction", payload: {} },
      { id: 3, seq: 3, type: "memory_saved", payload: {} },
      { id: 4, seq: 4, type: "model_fallback", payload: {} },
      { id: 5, seq: 5, type: "sensing", payload: {} },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    expect(await screen.findByText(/thread\.rollback/)).toBeTruthy();
    expect(screen.queryByText(/thread\.compaction/)).toBeNull();
    expect(screen.queryByText(/thread\.memory_saved/)).toBeNull();
    expect(screen.queryByText(/thread\.model_fallback/)).toBeNull();
    expect(screen.queryByText(/sensing/)).toBeNull();
  });

  it("v1.112 只读步骤聚合为单行动词计数，不落卡", async () => {
    const ro = (seq: number, tool: string, ok = true) => ({
      id: seq, seq, type: "command_run",
      payload: { tool, args: { path: "x" }, output: { ok, content: "..." } },
    });
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      ro(2, "read_file"),
      ro(3, "read_file"),
      ro(4, "grep"),
      { id: 5, seq: 5, type: "patch_applied", payload: { tool: "apply_patch", args: { file: "a.rs" }, output: { ok: true, content: "", changed_files: ["a.rs"] } } },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const summary = await screen.findByTestId("turn-readonly");
    expect(summary.textContent).toContain("tool.read_file 2");
    expect(summary.textContent).toContain("tool.grep 1");
    // 只读步骤不产生独立步骤卡
    const cards = screen.getAllByTestId("turn-step");
    expect(cards).toHaveLength(1);
  });

  it("失败的只读步骤仍单独红显卡，不计入聚合行", async () => {
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "任务" } },
      { id: 2, seq: 2, type: "command_run", payload: { tool: "read_file", args: { path: "x" }, output: { ok: false, content: "not found" } } },
      { id: 3, seq: 3, type: "command_run", payload: { tool: "grep", args: {}, output: { ok: true, content: "hit" } } },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    const card = await screen.findByTestId("turn-step");
    expect(card.className).toContain("turn-step-fail");
    expect(screen.getByText("not found")).toBeTruthy();
    const summary = screen.getByTestId("turn-readonly");
    expect(summary.textContent).toContain("tool.grep 1");
    expect(summary.textContent).not.toContain("tool.read_file");
  });

  it("verification 为空不渲染证据空壳", async () => {
    const events = [
      { id: 1, seq: 1, type: "diagnostics", payload: { verification: "" } },
    ];
    render(<AgentPanel api={mockApi(events)} t={t} sessionId="s1" />);
    await new Promise((r) => setTimeout(r, 30));
    expect(screen.queryByText(/evidence\.title/)).toBeNull();
  });
});

describe("v1.111 消息级撤销", () => {
  function mockUndoApi(events: Array<Record<string, unknown>>, status = "idle") {
    return {
      models: vi.fn().mockResolvedValue(models),
      trace: vi.fn().mockResolvedValue({ events, latest_seq: events.length }),
      getSession: vi.fn().mockResolvedValue({ session_id: "s1", status, latest_seq: events.length, outcome: null }),
      sendMessage: vi.fn().mockResolvedValue({}),
      control: vi.fn().mockResolvedValue({ ok: true }),
      checkpoints: vi.fn().mockResolvedValue({
        checkpoints: [
          { id: "cp1", tree: "t1", files: ["a.rs"], created_at: "", event_seq: 2 },
          { id: "cp2", tree: "t2", files: ["b.rs"], created_at: "", event_seq: 4 },
        ],
      }),
      rollbackCheckpoint: vi.fn().mockResolvedValue({ rolled_back: ["a.rs"] }),
    } as unknown as TenonApi;
  }

  const twoTurnEvents = [
    { id: 1, seq: 1, type: "user_input", payload: { text: "任务一" } },
    { id: 2, seq: 2, type: "patch_applied", payload: { tool: "apply_patch", args: { file: "a.rs" }, output: { ok: true, content: "", changed_files: ["a.rs"] } } },
    { id: 3, seq: 3, type: "user_input", payload: { text: "任务二" } },
    { id: 4, seq: 4, type: "patch_applied", payload: { tool: "apply_patch", args: { file: "b.rs" }, output: { ok: true, content: "", changed_files: ["b.rs"] } } },
  ];

  // v1.127：每个含改动的回合都提供撤销（恢复到该消息发送前状态）。
  it("每个含改动的回合显示撤销钮", async () => {
    render(<AgentPanel api={mockUndoApi(twoTurnEvents)} t={t} sessionId="s1" />);
    await screen.findByText("任务二");
    const turns = screen.getAllByTestId("turn");
    expect(turns[0].querySelector("[data-testid^=\"turn-undo-\"]")).toBeTruthy();
    expect(turns[1].querySelector("[data-testid^=\"turn-undo-\"]")).toBeTruthy();
    // 纯问答无改动回合仍无撤销钮
    const events = [
      { id: 1, seq: 1, type: "user_input", payload: { text: "纯问答" } },
      { id: 2, seq: 2, type: "decision", payload: { intent: "答案" } },
    ];
    const { container: qaContainer } = render(
      <AgentPanel api={mockUndoApi(events)} t={t} sessionId="s1" />
    );
    await screen.findByText("答案");
    expect(qaContainer.querySelector("[data-testid^=\"turn-undo-\"]")).toBeNull();
  });

  // v1.135 撤销三合一：truncate 回滚 + 任务文本回填输入框 + 线程截断（气泡及其后信息消失），
  // 重做改挂「已撤销」提示条。
  it("点击撤销：truncate 回滚、文字回填输入框、气泡截断、重做挂提示条", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    let call = 0;
    const api = mockUndoApi(twoTurnEvents);
    (api.trace as ReturnType<typeof vi.fn>).mockImplementation((_sid: string, after = 0) => {
      call += 1;
      const visible = call === 1 ? twoTurnEvents : twoTurnEvents.filter((e) => (e.seq as number) < 3);
      return Promise.resolve({
        events: visible.filter((e) => (e.seq as number) > after),
        latest_seq: 4,
      });
    });
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    fireEvent.click(await screen.findByTestId("turn-undo-3"));
    await waitFor(() => expect(api.rollbackCheckpoint).toHaveBeenCalledWith("cp2", true));
    // ① 任务文本回填输入框
    await waitFor(() => {
      const box = screen.getByRole("textbox") as HTMLTextAreaElement;
      expect(box.value).toBe("任务二");
    });
    // ② 气泡及其后信息消失（重载返回截断后事件；「任务一」仍在）
    await waitFor(() =>
      expect(screen.queryByText("任务二", { selector: ".turn-task" })).toBeNull(),
    );
    expect(screen.getByText("任务一")).toBeTruthy();
    // ③ 重做挂「已撤销」提示条
    expect(screen.getByTestId("undo-notice")).toBeTruthy();
    expect(screen.getByTestId("undo-redo")).toBeTruthy();
    confirmSpy.mockRestore();
  });

  it("确认取消则不发起任何请求", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);
    const api = mockUndoApi(twoTurnEvents);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    const undo = await screen.findByTestId("turn-undo-3");
    fireEvent.click(undo);
    // 取消即中止：checkpoints 与 rollback 均不调用
    await new Promise((r) => setTimeout(r, 30));
    expect(api.checkpoints).not.toHaveBeenCalled();
    expect(api.rollbackCheckpoint).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });

  it("运行态撤销钮禁用", async () => {
    render(<AgentPanel api={mockUndoApi(twoTurnEvents, "executing")} t={t} sessionId="s1" />);
    const undo = await screen.findByTestId("turn-undo-3");
    expect(undo).toBeDisabled();
  });

  // v1.123 气泡下按钮组：复制恒可用；重做初始不存在，撤销后挂在被撤销回合上（v1.127）。
  it("每回合渲染复制钮，点击写入剪贴板并回显已复制", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    try {
      render(<AgentPanel api={mockUndoApi(twoTurnEvents)} t={t} sessionId="s1" />);
      await screen.findByText("任务二");
      // 无改动回合也有复制钮；初始无重做钮
      expect(screen.getByTestId("turn-copy-1")).toBeTruthy();
      expect(screen.queryByTestId(/^turn-redo-/)).toBeNull();
      fireEvent.click(screen.getByTestId("turn-copy-3"));
      await waitFor(() => expect(writeText).toHaveBeenCalledWith("任务二"));
      expect(await screen.findByText("thread.copied")).toBeTruthy();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  // v1.135：重做从气泡钮改挂「已撤销」提示条——调 unrollback 清水位，成功即收。
  it("撤销后重做挂提示条并调 unrollback，成功即收", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    const api = mockUndoApi(twoTurnEvents);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    expect(screen.queryByTestId("undo-notice")).toBeNull();
    fireEvent.click(await screen.findByTestId("turn-undo-3"));
    fireEvent.click(await screen.findByTestId("undo-redo"));
    await waitFor(() => expect(api.control).toHaveBeenCalledWith("s1", "unrollback"));
    await waitFor(() => expect(screen.queryByTestId("undo-notice")).toBeNull());
    confirmSpy.mockRestore();
  });

  // v1.135：撤销后发送新消息，提示条（重做入口）即收。
  it("撤销后发送新消息提示条即收", async () => {
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(true);
    const api = mockUndoApi(twoTurnEvents);
    render(<AgentPanel api={api} t={t} sessionId="s1" />);
    fireEvent.click(await screen.findByTestId("turn-undo-3"));
    expect(await screen.findByTestId("undo-notice")).toBeTruthy();
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "新任务" } });
    fireEvent.click(screen.getByRole("button", { name: "message.send" }));
    await waitFor(() => expect(api.sendMessage).toHaveBeenCalledWith("s1", "新任务"));
    await waitFor(() => expect(screen.queryByTestId("undo-notice")).toBeNull());
    confirmSpy.mockRestore();
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
