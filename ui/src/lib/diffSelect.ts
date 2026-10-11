// v2.7 diff 级反馈回路（design.md §7.2 / design-v2.md §6）：统一 diff 行 →
// (文件, 新文件行号) 纯映射与选区目标提取。规则：`+++` 头取路径（/dev/null
// 回退 `---` 头旧路径——整文件删除）；`@@ -a,b +c,d @@` 头取新文件起始行 c；
// hunk 内上下文 / `+` 行携带当前新行号并进位，`-` 行只存在于旧文件不计新行号；
// hunk 外（diff --git / index / 文件头）为元数据行。纯函数随 diff 原文供测。

export type DiffLineKind = "meta" | "hunk" | "add" | "remove" | "context";

export interface DiffLineInfo {
  /** 该行所属文件（来自最近 +++ / --- 头）；null = 尚未见文件头。 */
  path: string | null;
  /** 新文件行号（1 起）；remove / 元数据行为 null。 */
  newLine: number | null;
  kind: DiffLineKind;
}

/** 逐行标注 diff：与渲染行序一致（含空行），供选区 → 反馈目标换算。 */
export function mapDiffLines(diff: string): DiffLineInfo[] {
  let path: string | null = null;
  let oldPath: string | null = null;
  let newLine = 0;
  let inHunk = false;
  return diff.split(/\r\n|\r|\n/).map((line) => {
    if (line.startsWith("+++ ")) {
      const raw = line.slice(4).split("\t")[0].trim();
      path = raw === "/dev/null" ? oldPath : raw.replace(/^b\//, "");
      inHunk = false;
      return { path, newLine: null, kind: "meta" as const };
    }
    if (line.startsWith("--- ")) {
      const raw = line.slice(4).split("\t")[0].trim();
      oldPath = raw === "/dev/null" ? null : raw.replace(/^a\//, "");
      inHunk = false;
      return { path: oldPath ?? path, newLine: null, kind: "meta" as const };
    }
    if (line.startsWith("@@")) {
      const m = line.match(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      newLine = m ? Number(m[1]) : 0;
      inHunk = true;
      return { path, newLine: null, kind: "hunk" as const };
    }
    if (!inHunk) {
      if (line.startsWith("diff --git")) {
        path = null;
        oldPath = null;
      }
      return { path: path ?? oldPath, newLine: null, kind: "meta" as const };
    }
    if (line.startsWith("+")) {
      return { path, newLine: newLine++, kind: "add" as const };
    }
    if (line.startsWith("-")) {
      return { path, newLine: null, kind: "remove" as const };
    }
    if (line.startsWith("\\")) {
      // "\ No newline at end of file"
      return { path, newLine: null, kind: "meta" as const };
    }
    return { path, newLine: newLine++, kind: "context" as const };
  });
}

export interface DiffSelectionTarget {
  path: string;
  /** 选区内新行号非空行的 min/max；纯删除 / 元数据选区为 undefined（文件级反馈）。 */
  startLine?: number;
  endLine?: number;
  /** 选区原文（统一 diff 行，含 +/- 前缀）——反馈摘录随文。 */
  excerpt: string;
}

/** 提取选区反馈目标；不可发起反馈（纯元数据行 / 越界 / 尚无文件头）返回 null。
 *  lines 与 mapDiffLines 输出同长（调用方复用已解析的渲染行序）。 */
export function selectionTargetFromLines(
  lines: string[],
  start: number,
  end: number,
): DiffSelectionTarget | null {
  const infos = mapDiffLines(lines.join("\n"));
  return selectionTargetFromInfos(lines, infos, start, end);
}

/** infos 已解析时的零重解析入口（ReviewPane 渲染行序复用）。 */
export function selectionTargetFromInfos(
  lines: string[],
  infos: DiffLineInfo[],
  start: number,
  end: number,
): DiffSelectionTarget | null {
  const lo = Math.max(0, Math.min(start, end));
  const hi = Math.min(Math.min(lines.length, infos.length) - 1, Math.max(start, end));
  if (lo > hi) return null;
  const slice = infos.slice(lo, hi + 1);
  // 只含元数据 / hunk 头的选区不可发起反馈（design.md §7.2 v2.7）
  if (!slice.some((info) => info.kind === "add" || info.kind === "remove" || info.kind === "context")) {
    return null;
  }
  let path: string | null = null;
  for (const info of slice) if (info.path) path = info.path;
  if (!path) return null;
  const numbered = slice.filter((info) => info.newLine !== null);
  return {
    path,
    startLine: numbered.length ? Math.min(...numbered.map((i) => i.newLine!)) : undefined,
    endLine: numbered.length ? Math.max(...numbered.map((i) => i.newLine!)) : undefined,
    excerpt: lines.slice(lo, hi + 1).join("\n"),
  };
}
