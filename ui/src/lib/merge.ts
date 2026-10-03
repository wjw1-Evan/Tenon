// 前端三方合并（与内核 tenon-core::merge 语义一致）：
// 行级 diff3——不相交改动自动合并；同区改动抛错（三栏手动选择）。
interface Hunk {
  baseStart: number;
  baseEnd: number;
  replacement: string;
}

function splitLines(s: string): string[] {
  return s === "" ? [] : s.split(/(?<=\n)/);
}

function hunks(base: string, other: string): Hunk[] {
  const bl = splitLines(base);
  const ol = splitLines(other);
  const n = bl.length;
  const m = ol.length;
  // LCS 长度表
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = bl[i] === ol[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  // 收集连续变更块
  const out: Hunk[] = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && bl[i] === ol[j]) {
      i += 1;
      j += 1;
      continue;
    }
    const start = i;
    const replacement: string[] = [];
    while ((i < n && j < m && bl[i] !== ol[j]) || (i < n && j >= m) || (j < m && i >= n)) {
      if (i < n && j < m && bl[i] !== ol[j] && dp[i + 1][j] >= dp[i][j + 1]) {
        i += 1; // 删除 base 行
      } else if (j < m) {
        replacement.push(ol[j]);
        j += 1; // 插入 other 行
      } else if (i < n) {
        i += 1;
      } else {
        break;
      }
      // 连续同向消耗直到回到匹配
      if (i < n && j < m && bl[i] === ol[j]) break;
      if (i >= n && j >= m) break;
    }
    out.push({ baseStart: start, baseEnd: i, replacement: replacement.join("") });
  }
  return out;
}

export function mergeThreeWay(base: string, ours: string, theirs: string): string {
  const baseLines = splitLines(base);
  const oursHunks = hunks(base, ours);
  const theirsHunks = hunks(base, theirs);

  let result = "";
  let pos = 0;
  let oi = 0;
  let ti = 0;
  for (;;) {
    const o = oursHunks[oi];
    const t = theirsHunks[ti];
    if (!o && !t) break;
    const useOurs = o && (!t || o.baseStart <= t.baseStart);
    const hunk = useOurs ? o! : t!;
    const other = useOurs ? t : o;
    const overlaps =
      other &&
      ((hunk.baseStart < other.baseEnd && other.baseStart < hunk.baseEnd) ||
        (hunk.baseStart === other.baseStart && hunk.baseEnd === other.baseEnd));
    if (overlaps) {
      if (
        o!.replacement === t!.replacement &&
        o!.baseStart === t!.baseStart
      ) {
        for (let i = pos; i < hunk.baseStart && i < baseLines.length; i++) {
          result += baseLines[i];
        }
        result += o!.replacement;
        pos = hunk.baseEnd;
        oi += 1;
        ti += 1;
        continue;
      }
      throw new Error("merge conflict");
    }
    for (let i = pos; i < hunk.baseStart && i < baseLines.length; i++) {
      result += baseLines[i];
    }
    result += hunk.replacement;
    pos = hunk.baseEnd;
    if (useOurs) oi += 1;
    else ti += 1;
  }
  for (let i = pos; i < baseLines.length; i++) {
    result += baseLines[i];
  }
  return result;
}
