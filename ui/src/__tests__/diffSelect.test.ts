// v2.7 diff 级反馈回路（design.md §7.2 / design-v2.md §6）：行号映射纯函数契约——
// +++ 头取路径（/dev/null 回退 --- 旧路径）、@@ 头取新文件起始行、上下文 / + 行
// 进位、- 行不计新行号；选区目标提取（含端点、纯元数据选区拒发、纯删除退化文件级）。
import { describe, expect, it } from "vitest";
import { mapDiffLines, selectionTargetFromLines } from "../lib/diffSelect";

const MULTI = [
  "diff --git a/src/a.ts b/src/a.ts",
  "index 111..222 100644",
  "--- a/src/a.ts",
  "+++ b/src/a.ts",
  "@@ -1,4 +1,4 @@",
  " line1",
  "-line2",
  "+line2-new",
  " line3",
  " line4",
  "diff --git a/src/b.ts b/src/b.ts",
  "index 333..444 100644",
  "--- a/src/b.ts",
  "+++ b/src/b.ts",
  "@@ -10,2 +10,3 @@",
  " ctx",
  "+added",
  "+added2",
].join("\n");

describe("mapDiffLines 行号映射（v2.7）", () => {
  it("上下文 / + 行携带新行号并进位，- 行不计", () => {
    const infos = mapDiffLines(MULTI);
    // 索引：0 diff --git / 1 index / 2 --- / 3 +++ / 4 @@ / 5..8 内容
    expect(infos[0].kind).toBe("meta");
    expect(infos[0].path).toBeNull();
    expect(infos[3]).toMatchObject({ path: "src/a.ts", newLine: null, kind: "meta" });
    expect(infos[4]).toMatchObject({ kind: "hunk", newLine: null });
    expect(infos[5]).toMatchObject({ path: "src/a.ts", newLine: 1, kind: "context" });
    expect(infos[6]).toMatchObject({ newLine: null, kind: "remove" });
    expect(infos[7]).toMatchObject({ path: "src/a.ts", newLine: 2, kind: "add" });
    expect(infos[8]).toMatchObject({ newLine: 3, kind: "context" });
  });

  it("多文件：第二文件路径切换且 hunk 头重置新行号", () => {
    const infos = mapDiffLines(MULTI);
    expect(infos[13]).toMatchObject({ path: "src/b.ts", kind: "meta" });
    expect(infos[14]).toMatchObject({ kind: "hunk", newLine: null });
    expect(infos[15]).toMatchObject({ path: "src/b.ts", newLine: 10, kind: "context" });
    expect(infos[16]).toMatchObject({ newLine: 11, kind: "add" });
    expect(infos[17]).toMatchObject({ newLine: 12, kind: "add" });
  });

  it("整文件删除：+++ /dev/null 回退 --- 旧路径，- 行不计新行号", () => {
    const diff = ["--- a/gone.ts", "+++ /dev/null", "@@ -1,2 +0,0 @@", "-a", "-b"].join("\n");
    const infos = mapDiffLines(diff);
    expect(infos[1].path).toBe("gone.ts");
    expect(infos[3]).toMatchObject({ path: "gone.ts", newLine: null, kind: "remove" });
  });

  it("hunk 头 -0,0 新建文件形态与 No newline 标记", () => {
    const diff = [
      "--- /dev/null",
      "+++ b/born.ts",
      "@@ -0,0 +1,2 @@",
      "+x",
      "\\ No newline at end of file",
    ].join("\n");
    const infos = mapDiffLines(diff);
    expect(infos[2].kind).toBe("hunk");
    expect(infos[3]).toMatchObject({ path: "born.ts", newLine: 1, kind: "add" });
    expect(infos[4]).toMatchObject({ kind: "meta", newLine: null });
  });
});

describe("selectionTargetFromLines 选区目标（v2.7）", () => {
  it("跨 add/remove/context 选区取新行号 min/max + 原文摘录", () => {
    const lines = MULTI.split("\n");
    // 选中 5..8（context/-old/+new/context → 新行 1..3）
    const target = selectionTargetFromLines(lines, 5, 8);
    expect(target).not.toBeNull();
    expect(target!.path).toBe("src/a.ts");
    expect(target!.startLine).toBe(1);
    expect(target!.endLine).toBe(3);
    expect(target!.excerpt.split("\n")[0]).toBe(" line1");
    expect(target!.excerpt).toContain("+line2-new");
  });

  it("纯删除选区退化为文件级（无行号，摘录仍随文）", () => {
    const diff = ["--- a/gone.ts", "+++ /dev/null", "@@ -1,2 +0,0 @@", "-a", "-b"].join("\n");
    const target = selectionTargetFromLines(diff.split("\n"), 3, 4);
    expect(target).toMatchObject({ path: "gone.ts", startLine: undefined, endLine: undefined });
    expect(target!.excerpt).toBe("-a\n-b");
  });

  it("纯元数据 / hunk 头选区不可发起反馈（返回 null）", () => {
    const lines = MULTI.split("\n");
    expect(selectionTargetFromLines(lines, 0, 4)).toBeNull(); // diff --git..@@ 全元数据
    expect(selectionTargetFromLines(lines, 3, 4)).toBeNull(); // +++ 头 + @@ 头
  });

  it("越界与空 diff 安全：夹取到行界 / 返回 null", () => {
    const lines = MULTI.split("\n");
    expect(selectionTargetFromLines(lines, -5, 999)).not.toBeNull();
    expect(selectionTargetFromLines([], 0, 3)).toBeNull();
    expect(selectionTargetFromLines(["no diff header at all"], 0, 0)).toBeNull();
  });

  it("选区含第二文件内容时取该文件路径", () => {
    const lines = MULTI.split("\n");
    const target = selectionTargetFromLines(lines, 15, 17);
    expect(target).toMatchObject({ path: "src/b.ts", startLine: 10, endLine: 12 });
  });
});
