// EditorPane 语言 provider（LSP）与装饰逻辑深度测试。
import { render, screen, waitFor, act } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { EditorPane, type EditorTab } from "../components/EditorPane";

const { fakeEditor, monacoStub } = vi.hoisted(() => ({
  fakeEditor: {
    trigger: vi.fn(),
    focus: vi.fn(),
    revealLineInCenter: vi.fn(),
    getModel: () => ({
      getValueInRange: (r: { startLineNumber: number; endLineNumber: number }) =>
        r.startLineNumber === r.endLineNumber ? "" : "selected text",
    }),
    createDecorationsCollection: vi.fn(() => ({ clear() {}, set() {} })),
    onDidChangeCursorSelection: vi.fn(() => ({ dispose() {} })),
  },
  monacoStub: {
    Uri: { parse: (s: string) => ({ path: s }) },
    editor: {
      registerCommand: (_id: string, _fn: (...a: unknown[]) => void) => ({ dispose() {} }),
    },
    languages: new Proxy(
      {},
      {
        get: () => () => ({ dispose() {} }),
      }
    ),
  },
}));

// 存储注册的 provider 和 command 以便测试调用。
const providers: Record<string, Record<string, unknown>> = {};
const commands: Record<string, (...a: unknown[]) => void> = {};

vi.mock("@monaco-editor/react", async () => {
  const { useEffect, useRef } = await import("react");
  return {
    default: function FakeMonaco({
      path,
      value,
      onMount,
    }: {
      path: string;
      value: string;
      onMount?: (editor: unknown, monaco: unknown) => void;
    }) {
      const called = useRef(false);
      useEffect(() => {
        if (!called.current && onMount) {
          called.current = true;
          onMount(fakeEditor, monacoStub);
        }
      }, [onMount]);
      return <div data-testid={`monaco-${path}`}>{`${path}:${value}`}</div>;
    },
  };
});

// 用 Proxy 捕获注册调用
(monacoStub.languages as unknown as Record<string, unknown>) = new Proxy(
  {} as Record<string, unknown>,
  {
    get(_target, method: string) {
      return (selector: string, provider: Record<string, unknown>) => {
        providers[method] = provider;
        return { dispose() {} };
      };
    },
  }
);

(monacoStub.editor as unknown as Record<string, unknown>).registerCommand = (
  id: string,
  fn: (...a: unknown[]) => void
) => {
  commands[id] = fn;
  return { dispose() {} };
};

const lspMock = vi.fn();
const applyLspEditMock = vi.fn();
const highlightsMock = vi.fn();
const inlineCompleteMock = vi.fn();

function api() {
  return {
    lsp: lspMock,
    applyLspEdit: applyLspEditMock,
    getHighlights: highlightsMock,
    inlineComplete: inlineCompleteMock,
  } as never;
}

const tabs: EditorTab[] = [{ path: "src/app.ts", content: "const x = 1;" }];

function noop(key?: unknown) {
  return String(key);
}

beforeEach(() => {
  vi.clearAllMocks();
  for (const k of Object.keys(providers)) delete providers[k];
  for (const k of Object.keys(commands)) delete commands[k];
  lspMock.mockResolvedValue({ result: null });
  applyLspEditMock.mockResolvedValue({ files: [{ path: "a.ts" }] });
  highlightsMock.mockResolvedValue({ fallback: true });
});

function mountEditor(extra: Record<string, unknown> = {}) {
  return render(
    <EditorPane
      t={noop}
      api={api()}
      projectId="p1"
      projectRoot="/tmp/proj"
      tabs={tabs}
      activePath="src/app.ts"
      onSelect={noop}
      onClose={noop}
      onChange={noop}
      {...extra}
    />
  );
}

describe("EditorPane providers", () => {
  it("renders empty state when no active tab", () => {
    render(
      <EditorPane t={noop} api={api()} projectId="p1" tabs={[]} activePath={null}
        onSelect={noop} onClose={noop} onChange={noop} />
    );
    expect(screen.getByText("Tenon Harness")).toBeTruthy();
  });

  it("registers all language providers on mount", () => {
    mountEditor();
    expect(Object.keys(providers).length).toBeGreaterThanOrEqual(8);
  });

  it("completion provider returns parsed LSP suggestions", async () => {
    lspMock.mockResolvedValue({
      result: [
        { label: "myFunction", kind: 3, detail: "() => void" },
        { label: "myVariable", kind: 6 },
      ],
    });
    mountEditor();
    const p = providers.registerCompletionItemProvider as {
      provideCompletionItems: (model: unknown, pos: unknown) => Promise<{ suggestions: unknown[] }>;
    };
    const model = {
      uri: { path: "/tmp/proj/src/app.ts" },
      getWordUntilPosition: () => ({ startColumn: 7, endColumn: 8 }),
    };
    const result = await p.provideCompletionItems(model, { lineNumber: 1, column: 8 });
    expect(lspMock).toHaveBeenCalledWith(expect.objectContaining({
      project_id: "p1", path: "tmp/proj/src/app.ts", action: "completion", line: 0, character: 7,
    }));
    expect(result.suggestions.length).toBe(2);
  });

  it("hover provider returns LSP hover content", async () => {
    lspMock.mockResolvedValue({ result: { contents: { value: "Type: string" } } });
    mountEditor();
    const p = providers.registerHoverProvider as {
      provideHover: (model: unknown, pos: unknown, token: unknown) => Promise<{ contents: Array<{ value: string }> }>;
    };
    const token = { isCancellationRequested: false };
    const result = await p.provideHover(
      { uri: { path: "/tmp/proj/src/app.ts" } }, { lineNumber: 1, column: 7 }, token
    );
    expect(result.contents[0].value).toBe("Type: string");
  });

  it("hover returns empty on cancellation", async () => {
    mountEditor();
    const p = providers.registerHoverProvider as {
      provideHover: (m: unknown, p: unknown, t: unknown) => Promise<{ contents: unknown[] }>;
    };
    const result = await p.provideHover(
      { uri: { path: "/x.ts" } }, { lineNumber: 1, column: 1 }, { isCancellationRequested: true }
    );
    expect(result.contents).toEqual([]);
  });

  it("definition provider maps LSP locations", async () => {
    lspMock.mockResolvedValue({
      result: [{ uri: "file:///tmp/proj/src/other.ts", range: { startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 10 } }],
    });
    mountEditor();
    const p = providers.registerDefinitionProvider as {
      provideDefinition: (m: unknown, p: unknown, t: unknown) => Promise<unknown[]>;
    };
    const result = await p.provideDefinition(
      { uri: { path: "/tmp/proj/src/app.ts" } }, { lineNumber: 1, column: 7 }, { isCancellationRequested: false }
    );
    expect(result.length).toBe(1);
  });

  it("references provider maps LSP locations", async () => {
    lspMock.mockResolvedValue({
      result: [{ uri: "file:///tmp/proj/src/ref.ts", range: { startLineNumber: 2, startColumn: 1, endLineNumber: 2, endColumn: 5 } }],
    });
    mountEditor();
    const p = providers.registerReferenceProvider as {
      provideReferences: (m: unknown, p: unknown, c: unknown, t: unknown) => Promise<unknown[]>;
    };
    const result = await p.provideReferences(
      { uri: { path: "/tmp/proj/src/app.ts" } }, { lineNumber: 1, column: 7 }, {}, { isCancellationRequested: false }
    );
    expect(result.length).toBe(1);
  });

  it("signatureHelp provider returns parsed signature", async () => {
    lspMock.mockResolvedValue({ result: { signatures: [{ label: "fn(a: string)" }] } });
    mountEditor();
    const p = providers.registerSignatureHelpProvider as {
      provideSignatureHelp: (m: unknown, p: unknown, t: unknown, c: unknown) => Promise<{ value: { signatures: unknown[] } } | null>;
    };
    const result = await p.provideSignatureHelp(
      { uri: { path: "/x.ts" } }, { lineNumber: 1, column: 1 }, { isCancellationRequested: false }, {}
    );
    expect(result?.value.signatures.length).toBe(1);
  });

  it("codeActions provider returns parsed actions with command", async () => {
    lspMock.mockResolvedValue({
      result: [{ title: "Add import", kind: "quickfix", edit: { workspaceEdit: { changes: {} } } }],
    });
    mountEditor();
    const p = providers.registerCodeActionProvider as {
      provideCodeActions: (m: unknown, r: unknown, c: unknown, t: unknown) => Promise<{ actions: Array<{ title: string }> }>;
    };
    const result = await p.provideCodeActions(
      { uri: { path: "/x.ts" } }, {}, {}, { isCancellationRequested: false }
    );
    expect(result.actions[0].title).toBe("Add import");
  });

  it("rename provider requests rename and applies edit", async () => {
    lspMock.mockResolvedValue({ result: { changes: { "file:///a.ts": [] } } });
    const onFlush = vi.fn();
    const onApplied = vi.fn();
    mountEditor({ sessionId: "s1", onFlushFile: onFlush, onWorkspaceApplied: onApplied });
    const p = providers.registerRenameProvider as {
      provideRenameEdits: (m: unknown, p: unknown, newName: string, t: unknown) => Promise<unknown>;
    };
    await p.provideRenameEdits(
      { uri: { path: "/x.ts" } }, { lineNumber: 1, column: 7 }, "newName", { isCancellationRequested: false }
    );
    expect(lspMock).toHaveBeenCalledWith(expect.objectContaining({ action: "rename" }));
    expect(applyLspEditMock).toHaveBeenCalled();
  });

  it("formatting provider applies workspace edit", async () => {
    lspMock.mockResolvedValue({ result: [{ range: {}, newText: "formatted" }] });
    mountEditor({ sessionId: "s1" });
    const p = providers.registerDocumentFormattingEditProvider as {
      provideDocumentFormattingEdits: (m: unknown, t: unknown, o: unknown) => Promise<unknown>;
    };
    const result = await p.provideDocumentFormattingEdits({ uri: { path: "/x.ts" } }, {}, {});
    expect(result).toEqual([]);
    expect(applyLspEditMock).toHaveBeenCalled();
  });

  it("goto reveals line in center when matching path", () => {
    mountEditor({ goto: { path: "src/app.ts", line: 42 } });
    expect(fakeEditor.revealLineInCenter).toHaveBeenCalledWith(42);
    expect(fakeEditor.focus).toHaveBeenCalled();
  });

  it("does not reveal line for mismatched path", () => {
    mountEditor({ goto: { path: "other.ts", line: 1 } });
    expect(fakeEditor.revealLineInCenter).not.toHaveBeenCalled();
  });

  it("creates AI line decorations for modified lines", async () => {
    mountEditor({ aiModifiedLines: { "src/app.ts": [3, 7, 12] } });
    await waitFor(() => {
      expect(fakeEditor.createDecorationsCollection).toHaveBeenCalledWith(
        expect.arrayContaining([
          expect.objectContaining({ options: expect.objectContaining({ className: "ai-line" }) }),
        ])
      );
    });
  });

  it("reports selection change callback with text", () => {
    const onSelectionChange = vi.fn();
    mountEditor({ onSelectionChange });
    const selCb = (fakeEditor.onDidChangeCursorSelection as ReturnType<typeof vi.fn>).mock.calls[0]?.[0] as (e: unknown) => void;
    act(() => {
      selCb({ selection: { startLineNumber: 1, startColumn: 1, endLineNumber: 3, endColumn: 5 } });
    });
    expect(onSelectionChange).toHaveBeenCalledWith(expect.objectContaining({ path: "src/app.ts" }));
  });

  it("highlights tokens from tree-sitter LSP", async () => {
    highlightsMock.mockResolvedValue({
      fallback: false,
      tokens: [{ kind: "keyword", start_line: 0, start_column: 0, end_line: 0, end_column: 5 }],
    });
    mountEditor();
    await waitFor(() => expect(highlightsMock).toHaveBeenCalledWith("p1", "src/app.ts"));
    await waitFor(() => {
      expect(fakeEditor.createDecorationsCollection).toHaveBeenCalledWith(
        expect.arrayContaining([
          expect.objectContaining({ options: expect.objectContaining({ inlineClassName: "tenon-hl-keyword" }) }),
        ])
      );
    });
  });

  it("shows unsaved dot for dirty files", () => {
    mountEditor({ unsavedPaths: { "src/app.ts": true }, unsavedTitle: "未保存" });
    expect(screen.getByTestId("unsaved-src/app.ts")).toBeTruthy();
  });
});

describe("EditorPane inline completions", () => {
  function mountInline(extra: Record<string, unknown> = {}) {
    return render(
      <EditorPane
        t={noop}
        api={api()}
        projectId="p1"
        projectRoot="/tmp/proj"
        sessionId="s1"
        inlineCompletionEnabled={true}
        tabs={tabs}
        activePath="src/app.ts"
        onSelect={noop}
        onClose={noop}
        onChange={noop}
        {...extra}
      />
    );
  }

  it("inline completions freeInlineCompletions is a no-op", () => {
    mountInline();
    const p = providers.registerInlineCompletionsProvider as { freeInlineCompletions: () => void };
    expect(() => p.freeInlineCompletions()).not.toThrow();
  });

  it("inline completions returns empty when disabled", async () => {
    render(
      <EditorPane t={noop} api={api()} projectId="p1" sessionId="s1"
        inlineCompletionEnabled={false} tabs={tabs} activePath="src/app.ts"
        onSelect={noop} onClose={noop} onChange={noop} />
    );
    const p = providers.registerInlineCompletionsProvider as {
      provideInlineCompletions: (m: unknown, pos: unknown, c: unknown, t: unknown) => Promise<{ items: unknown[] }>;
    };
    const result = await p.provideInlineCompletions(
      { uri: { path: "/x.ts" } }, {}, {}, { isCancellationRequested: false }
    );
    expect(result.items).toEqual([]);
    expect(inlineCompleteMock).not.toHaveBeenCalled();
  });

  it("inline completions returns empty without session", async () => {
    render(
      <EditorPane t={noop} api={api()} projectId="p1" sessionId={null}
        inlineCompletionEnabled={true} tabs={tabs} activePath="src/app.ts"
        onSelect={noop} onClose={noop} onChange={noop} />
    );
    const p = providers.registerInlineCompletionsProvider as {
      provideInlineCompletions: (m: unknown, pos: unknown, c: unknown, t: unknown) => Promise<{ items: unknown[] }>;
    };
    const result = await p.provideInlineCompletions(
      { uri: { path: "/x.ts" } }, {}, {}, { isCancellationRequested: false }
    );
    expect(result.items).toEqual([]);
  });

  it("inline completions handles API error gracefully", async () => {
    inlineCompleteMock.mockRejectedValue(new Error("timeout"));
    mountInline();
    const p = providers.registerInlineCompletionsProvider as {
      provideInlineCompletions: (m: unknown, pos: unknown, c: unknown, t: unknown) => Promise<{ items: unknown[] }>;
    };
    const model = {
      uri: { path: "/tmp/proj/src/app.ts" },
      getValue: () => "x",
      getOffsetAt: () => 1,
    };
    vi.useFakeTimers();
    const promise = p.provideInlineCompletions(model, {}, {}, { isCancellationRequested: false });
    vi.advanceTimersByTime(400);
    const result = await promise;
    vi.useRealTimers();
    expect(result.items).toEqual([]);
  });

  it("inline completions returns empty on cancellation after delay", async () => {
    mountInline();
    const p = providers.registerInlineCompletionsProvider as {
      provideInlineCompletions: (m: unknown, pos: unknown, c: unknown, t: unknown) => Promise<{ items: unknown[] }>;
    };
    const model = { uri: { path: "/x.ts" }, getValue: () => "x", getOffsetAt: () => 1 };
    const token = { isCancellationRequested: false };
    vi.useFakeTimers();
    const promise = p.provideInlineCompletions(model, {}, {}, token);
    // 停顿期间取消
    token.isCancellationRequested = true;
    vi.advanceTimersByTime(400);
    const result = await promise;
    vi.useRealTimers();
    expect(result.items).toEqual([]);
  });

  it("formatting provider returns empty for no edits", async () => {
    lspMock.mockResolvedValue({ result: [] });
    mountEditor({ sessionId: "s1" });
    const p = providers.registerDocumentFormattingEditProvider as {
      provideDocumentFormattingEdits: (m: unknown, t: unknown, o: unknown) => Promise<unknown[]>;
    };
    const result = await p.provideDocumentFormattingEdits({ uri: { path: "/x.ts" } }, {}, {});
    expect(result).toEqual([]);
    expect(applyLspEditMock).not.toHaveBeenCalled();
  });
});
