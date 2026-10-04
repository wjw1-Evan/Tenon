import { describe, expect, it } from "vitest";
import { parseLspCompletions, toMonacoCompletionSuggestions } from "../lib/lsp";

function largeCompletionPayload(items: number) {
  return {
    items: Array.from({ length: items }, (_, index) => ({
      label: `symbol-${index}`,
      kind: (index % 12) + 1,
      detail: `detail-${index}`,
      documentation: { value: `documentation-${index}` },
      sortText: `${String(index).padStart(6, "0")}`,
      filterText: `filter-${index}`,
      textEdit: { newText: `insert-${index}` },
    })),
  };
}

describe("completion presentation budget (§8.7)", () => {
  it("normalizes and maps 5k LSP items within 80ms P50", () => {
    const payload = largeCompletionPayload(5_000);
    const range = {
      startLineNumber: 1,
      startColumn: 1,
      endLineNumber: 1,
      endColumn: 4,
    };
    const samples: number[] = [];
    for (let round = 0; round < 5; round += 1) {
      const started = performance.now();
      const suggestions = toMonacoCompletionSuggestions(
        parseLspCompletions(payload),
        range
      );
      samples.push(performance.now() - started);
      expect(suggestions).toHaveLength(5_000);
      expect(suggestions[4_999].insertText).toBe("insert-4999");
    }
    const p50 = [...samples].sort((a, b) => a - b)[2];
    expect(p50).toBeLessThan(80);
  });
});
