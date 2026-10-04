// LSP 响应归一化（§8.3）：Monaco 注册器的纯函数协议转换。
import { describe, expect, it } from "vitest";
import {
  lspRange,
  lspUriToModelPath,
  parseLspCompletions,
  parseLspCodeActions,
  parseLspHover,
  parseLspLocations,
  parseLspSignatureHelp,
} from "../lib/lsp";

describe("LSP normalizers", () => {
  it("converts zero-based LSP ranges to one-based Monaco ranges", () => {
    expect(lspRange({ start: { line: 2, character: 3 }, end: { line: 4, character: 7 } })).toEqual({
      startLineNumber: 3,
      startColumn: 4,
      endLineNumber: 5,
      endColumn: 8,
    });
  });

  it("maps absolute file URIs to project-relative model paths", () => {
    expect(lspUriToModelPath("file:///tmp/repo/src/a.ts", "/tmp/repo")).toBe("src/a.ts");
    expect(lspUriToModelPath("untitled:demo", "/tmp/repo")).toBe("untitled:demo");
  });

  it("normalizes hover, completion and signature payloads", () => {
    expect(parseLspHover({ contents: { value: "type A = number" } })).toBe("type A = number");
    expect(parseLspCompletions({ items: [{ label: "add", kind: 3, detail: "fn" }] })).toEqual([{
      label: "add",
      kind: 3,
      detail: "fn",
      documentation: undefined,
      insertText: "add",
      sortText: undefined,
      filterText: undefined,
    }]);
    expect(parseLspSignatureHelp({
      activeSignature: 0,
      activeParameter: 1,
      signatures: [{ label: "add(a: number, b: number)", parameters: [{ label: "a" }, { label: "b" }] }],
    })).toBe("add(a: number, b: number) · b");
  });

  it("normalizes single and multi-file locations", () => {
    const location = { uri: "file:///tmp/repo/src/a.ts", range: { start: { line: 1, character: 0 }, end: { line: 1, character: 3 } } };
    expect(parseLspLocations(location, "/tmp/repo")).toEqual([{
      uri: "src/a.ts",
      range: { startLineNumber: 2, startColumn: 1, endLineNumber: 2, endColumn: 4 },
    }]);
    expect(parseLspLocations([location], "/tmp/repo")).toHaveLength(1);
  });

  it("keeps code actions only when they carry an editable workspace edit", () => {
    expect(parseLspCodeActions([
      { title: "no edit" },
      { title: "extract", kind: "refactor", edit: { workspaceEdit: { changes: {} } } },
    ])).toEqual([{
      title: "extract",
      kind: "refactor",
      workspaceEdit: { changes: {} },
    }]);
  });
});
