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
  toMonacoCompletionSuggestions,
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

  it("lspRange handles missing/invalid positions gracefully", () => {
    expect(lspRange(null)).toEqual({ startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 1 });
    expect(lspRange({})).toEqual({ startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 1 });
    expect(lspRange({ start: { line: -1, character: -1 } })).toEqual({
      startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 1,
    });
  });

  it("lspUriToModelPath handles edge cases", () => {
    // Windows 盘符路径
    expect(lspUriToModelPath("file:///C:/repo/src/a.ts", "C:/repo")).toBe("src/a.ts");
    // root 本身
    expect(lspUriToModelPath("file:///tmp/repo", "/tmp/repo")).toBe("");
    // 非 file URI
    expect(lspUriToModelPath("https://example.com/a.ts")).toBe("https://example.com/a.ts");
    // 无效 URI
    expect(lspUriToModelPath("not a uri")).toBe("not a uri");
    // 无 projectRoot
    expect(lspUriToModelPath("file:///tmp/x.ts")).toBe("/tmp/x.ts");
  });

  it("parseLspHover handles array contents and empty", () => {
    expect(parseLspHover(null)).toBe("");
    expect(parseLspHover({})).toBe("");
    expect(parseLspHover({ contents: [{ value: "line1" }, { value: "line2" }] })).toBe("line1\n\nline2");
    expect(parseLspHover({ contents: "plain" })).toBe("plain");
    expect(parseLspHover({ contents: [{ value: "" }] })).toBe("");
  });

  it("parseLspCompletions handles items wrapper, textEdit, documentation object", () => {
    const result = parseLspCompletions([
      { label: "a", textEdit: { newText: "inserted" }, documentation: { value: "doc text" }, sortText: "0001", filterText: "flt" },
      { label: "", kind: 1 },
      { label: "b" },
    ]);
    expect(result).toHaveLength(2);
    expect(result[0].insertText).toBe("inserted");
    expect(result[0].documentation).toBe("doc text");
    expect(result[0].sortText).toBe("0001");
    expect(result[1].insertText).toBe("b");
  });

  it("toMonacoCompletionSuggestions maps items to Monaco format", () => {
    const range = { startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 5 };
    const result = toMonacoCompletionSuggestions(
      [{ label: "x", kind: 3, detail: "d", documentation: "doc", insertText: "xx" }],
      range
    );
    expect(result[0]).toEqual({
      label: "x", kind: 3, detail: "d", documentation: "doc",
      insertText: "xx", sortText: undefined, filterText: undefined, range,
    });
    // kind 缺省为 0
    const noKind = toMonacoCompletionSuggestions([{ label: "y" }], range);
    expect(noKind[0].kind).toBe(0);
  });

  it("parseLspSignatureHelp handles empty / missing / fallback", () => {
    expect(parseLspSignatureHelp(null)).toBe("");
    expect(parseLspSignatureHelp({})).toBe("");
    expect(parseLspSignatureHelp({ signatures: [] })).toBe("");
    expect(parseLspSignatureHelp({ signatures: [{ label: "" }] })).toBe("");
    // activeSignature 缺省 0
    expect(parseLspSignatureHelp({ signatures: [{ label: "fn()" }] })).toBe("fn()");
    // 负 activeSignature → 回退 0
    expect(parseLspSignatureHelp({
      activeSignature: -1, signatures: [{ label: "fb()" }, { label: "real()" }],
    })).toBe("fb()");
  });

  it("parseLspLocations handles LocationLink and wrapped locations", () => {
    // targetUri / targetSelectionRange（LocationLink 格式）
    const link = {
      targetUri: "file:///tmp/repo/target.ts",
      targetSelectionRange: { start: { line: 0, character: 0 }, end: { line: 0, character: 5 } },
    };
    const result = parseLspLocations(link, "/tmp/repo");
    expect(result[0].uri).toBe("target.ts");
    // { locations: [...] } 包装
    const wrapped = { locations: [{ uri: "file:///x.ts", range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } } }] };
    expect(parseLspLocations(wrapped)).toHaveLength(1);
    // 无 uri 的记录被过滤
    expect(parseLspLocations([{ range: {} }])).toHaveLength(0);
  });

  it("parseLspCodeActions handles actions wrapper and missing edit", () => {
    expect(parseLspCodeActions(null)).toEqual([]);
    expect(parseLspCodeActions({ actions: [{ title: "x", edit: { workspaceEdit: {} } }] })).toHaveLength(1);
    expect(parseLspCodeActions([{ title: "no edit" }])).toEqual([]);
  });
});
