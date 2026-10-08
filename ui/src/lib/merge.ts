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
    // 同起点时零宽插入先消费（插入逻辑上位于替换区之前）：否则替换块
    // 先把 pos 推过插入点，补行循环直接跳过、插入静默丢弃（与内核同规）
    let useOurs: boolean;
    if (o && t) {
      useOurs =
        o.baseStart === t.baseStart
          ? o.baseStart === o.baseEnd || t.baseStart !== t.baseEnd
          : o.baseStart < t.baseStart;
    } else {
      useOurs = !!o;
    }
    const hunk = (useOurs ? o : t)!;
    // 与另一侧任一未消费 hunk 判重叠：只看对侧当前指针会漏判
    // 「对侧后续 hunk 落入本 hunk 已应用区间」——既不报冲突又静默复活
    // 已删除行、产出损坏内容（回归见内核 merge.rs 同名测试）
    const rest = useOurs ? theirsHunks.slice(ti) : oursHunks.slice(oi);
    if (rest.some((other) => hunksOverlap(hunk, other))) {
      const a = oursHunks[oi]!;
      const b = theirsHunks[ti]!;
      if (
        a.replacement === b.replacement &&
        a.baseStart === b.baseStart &&
        a.baseEnd === b.baseEnd
      ) {
        // 两侧相同改动：取一次
        for (let i = pos; i < a.baseStart && i < baseLines.length; i++) {
          result += baseLines[i];
        }
        result += a.replacement;
        pos = a.baseEnd;
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

function hunksOverlap(a: Hunk, b: Hunk): boolean {
  // 区间相交（端点相接不算冲突：相邻行各自改动可干净合并）
  if (a.baseStart < b.baseEnd && b.baseStart < a.baseEnd) return true;
  // 等区间：同位置插入对插入、同范围替换对替换
  if (a.baseStart === b.baseStart && a.baseEnd === b.baseEnd) return true;
  // 零宽插入严格落在对方替换区间内部：锚定行已被对方删除，位置语义无法保全
  if (a.baseStart === a.baseEnd) {
    return b.baseStart < a.baseStart && a.baseStart < b.baseEnd;
  }
  if (b.baseStart === b.baseEnd) {
    return a.baseStart < b.baseStart && b.baseStart < a.baseEnd;
  }
  return false;
}
