import { describe, expect, it } from "vitest";
import { mergeThreeWay } from "../lib/merge";
import { aiLinesFromDiff, unionLines } from "../lib/aiLines";

const BASE = "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\n";

describe("前端三方合并（§8.6，与内核语义一致）", () => {
  it("单侧改动应用", () => {
    const merged = mergeThreeWay(BASE, BASE.replace("l5", "OURS"), BASE);
    expect(merged).toContain("OURS");
  });

  it("不相交改动干净合并", () => {
    const ours = BASE.replace("l2", "OURS2");
    const theirs = BASE.replace("l8", "THEIRS8");
    const merged = mergeThreeWay(BASE, ours, theirs);
    expect(merged).toContain("OURS2");
    expect(merged).toContain("THEIRS8");
    expect(merged).toContain("l5\n");
  });

  it("相同改动去重", () => {
    const both = BASE.replace("l3", "BOTH");
    expect(mergeThreeWay(BASE, both, both).match(/BOTH/g)?.length).toBe(1);
  });

  it("同区改动冲突抛错", () => {
    const ours = BASE.replace("l5", "OURS5");
    const theirs = BASE.replace("l5", "THEIRS5");
    expect(() => mergeThreeWay(BASE, ours, theirs)).toThrow("merge conflict");
  });

  it("尾部同位异文插入 = 冲突（与内核/git 语义一致）", () => {
    const base = "a\n";
    expect(() => mergeThreeWay(base, "a\nours\n", "a\ntheirs\n")).toThrow("merge conflict");
    // 相同追加 → 去重
    expect(mergeThreeWay(base, "a\nours\n", "a\nours\n")).toBe("a\nours\n");
  });
});

describe("行级 AI 角标（§7.5）", () => {
  it("从统一 diff 解析新增行号", () => {
    const diff = "--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,4 @@\n a\n+added\n b\n+second\n c\n";
    expect(aiLinesFromDiff(diff)).toEqual([2, 4]);
  });

  it("多补丁行号取并集", () => {
    expect(unionLines([3, 1], [5, 1])).toEqual([1, 3, 5]);
  });
});
