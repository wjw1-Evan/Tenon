// 行级 AI 角标（设计方案 §7.5 / §8.6）：从统一 diff 解析代理改动的行号。
// 所有代理写入的行带「AI」角标，直到用户编辑该区域或确认。

export function aiLinesFromDiff(diff: string): number[] {
  const lines: number[] = [];
  let current = 0;
  let inHunk = false;
  for (const line of diff.split("\n")) {
    if (line.startsWith("@@")) {
      const m = /\+(\d+)/.exec(line);
      current = m ? parseInt(m[1], 10) : 0;
      inHunk = true;
      continue;
    }
    if (!inHunk) continue;
    if (line.startsWith("+")) {
      lines.push(current);
      current += 1;
    } else if (line.startsWith("-")) {
      // 删除行不计新行号
    } else {
      current += 1;
    }
  }
  return lines;
}

/** 合并多份 diff 的 AI 行（同文件多次补丁取并集）。 */
export function unionLines(a: number[], b: number[]): number[] {
  return Array.from(new Set([...a, ...b])).sort((x, y) => x - y);
}
