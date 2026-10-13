// 受限 markdown 渲染器（v1.109 会话过程流，设计 §7.5「会话过程流」；v2.59 对话内容渲染质量扩展）。
// 零依赖：全部文本先 HTML 转义（代码块 / 表格单元格 / 语言标注 / 按钮文案同转义），
// 不解析内嵌 HTML，XSS 面为零。
// 支持集合：#~### 标题 / 无序·有序·任务列表 / 行内代码 / 围栏代码块（语言标注 + 复制头）/
// **粗体** *斜体* / http(s) 链接 / 表格 / 引用块 / 分隔线 / 段落。
const ESCAPE: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (ch) => ESCAPE[ch]);
}

/** 渲染参数：copyLabel = 代码块复制按钮文案（i18n）；缺省不渲染复制钮（向后兼容）。 */
export interface MarkdownLiteOptions {
  copyLabel?: string;
}

/** 行内渲染：先转义再做行内标记替换；行内代码以占位符隔离，避免其中标记被二次解析。 */
function renderInline(raw: string): string {
  const s = escapeHtml(raw);
  const codes: string[] = [];
  const masked = s.replace(/`([^`]+)`/g, (_m, c: string) => {
    codes.push(c);
    return `\u0000${codes.length - 1}\u0000`;
  });
  const linked = masked.replace(
    /\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
    '<a href="$2" target="_blank" rel="noreferrer">$1</a>',
  );
  const bolded = linked.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  const italicked = bolded.replace(/\*([^*\n]+)\*/g, "<em>$1</em>");
  return italicked.replace(/\u0000(\d+)\u0000/g, (_m, i: string) => `<code>${codes[Number(i)]}</code>`);
}

/** 表格分隔行：整行仅由 `:---` / `---:` / `---` 形态的单元格组成（可选外围管道符）。 */
function isTableDelim(line: string): boolean {
  const cells = splitTableRow(line);
  return cells.length >= 1 && cells.every((c) => /^:?-+:?$/.test(c));
}

/** 拆一行表格行为单元格：剥外围管道符后按 `|` 切分（单元格内 `\|` 转义不支持，受限集）。 */
function splitTableRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith("|")) s = s.slice(1);
  if (s.endsWith("|")) s = s.slice(0, -1);
  return s.split("|").map((c) => c.trim());
}

/** 分隔行单元格 → 对齐方式（th/td 的 text-align）。 */
function cellAlign(cell: string): "left" | "center" | "right" | null {
  const l = cell.startsWith(":");
  const r = cell.endsWith(":");
  if (l && r) return "center";
  if (r) return "right";
  if (l) return "left";
  return null;
}

interface ListItem {
  text: string;
  /** 任务清单标记：null = 普通项；"" = 未勾选；"x" = 已勾选。 */
  task: string | null;
}

interface ListGroup {
  ordered: boolean;
  items: ListItem[];
}

export function renderMarkdownLite(md: string, opts?: MarkdownLiteOptions): string {
  if (!md.trim()) return "";
  const lines = md.replace(/\r\n?/g, "\n").split("\n");
  const out: string[] = [];
  let para: string[] = [];
  let list: ListGroup | null = null;
  let quote: string[] = [];
  let code: { lang: string; lines: string[] } | null = null;

  const flushPara = () => {
    if (para.length) {
      // 行间断点用占位符传递（\u0001 不会被 escapeHtml 改写，行内代码
      // 占位用的是 \u0000）：转义后再换回 <br/>，直接拼 <br/> 会被
      // renderInline 的先转义策略转成字面文本
      out.push(
        `<p>${renderInline(para.join("\u0001")).replace(/\u0001/g, "<br/>")}</p>`,
      );
      para = [];
    }
  };
  const flushList = () => {
    if (list) {
      const tag = list.ordered ? "ol" : "ul";
      out.push(
        `<${tag}>${list.items
          .map((it) => {
            if (it.task === null) return `<li>${renderInline(it.text)}</li>`;
            const checked = it.task.toLowerCase() === "x" ? " checked" : "";
            return `<li class="task-item"><input type="checkbox" disabled${checked}/> ${renderInline(it.text)}</li>`;
          })
          .join("")}</${tag}>`,
      );
      list = null;
    }
  };
  const flushQuote = () => {
    if (quote.length) {
      out.push(
        `<blockquote><p>${renderInline(quote.join("\u0001")).replace(/\u0001/g, "<br/>")}</p></blockquote>`,
      );
      quote = [];
    }
  };
  const flushCode = () => {
    if (!code) return;
    // 流式草稿常见未闭合围栏：按已收到的内容直接成块。
    const langAttr = code.lang ? ` data-lang="${escapeHtml(code.lang)}"` : "";
    const copyBtn = opts?.copyLabel
      ? `<button type="button" class="code-copy">${escapeHtml(opts.copyLabel)}</button>`
      : "";
    const head =
      code.lang || copyBtn
        ? `<div class="code-head">${code.lang ? `<span class="code-lang">${escapeHtml(code.lang)}</span>` : ""}${copyBtn}</div>`
        : "";
    out.push(
      `<div class="code-block">${head}<pre><code${langAttr}>${escapeHtml(code.lines.join("\n"))}</code></pre></div>`,
    );
    code = null;
  };
  const flushAll = () => {
    flushPara();
    flushList();
    flushQuote();
  };

  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (code) {
      if (/^```/.test(line.trim())) {
        flushCode();
      } else {
        code.lines.push(line);
      }
      i += 1;
      continue;
    }
    const fence = line.trim().match(/^```(\S*)\s*$/);
    if (fence) {
      flushAll();
      code = { lang: fence[1] ?? "", lines: [] };
      i += 1;
      continue;
    }
    const t = line.trim();
    if (!t) {
      flushAll();
      i += 1;
      continue;
    }
    // 表格：当前行含管道符且下一行是分隔行（列数须与表头一致，GFM 语义）→
    // 收表头 + 体行（体行以「含管道符」延续，空行 / 围栏 / 标题等终止；流式半截表格按已到内容成表）。
    if (t.includes("|") && i + 1 < lines.length && isTableDelim(lines[i + 1])) {
      const header = splitTableRow(t);
      const delim = splitTableRow(lines[i + 1]);
      if (header.length === delim.length && header.some((c) => c !== "")) {
        flushAll();
        const aligns = delim.map(cellAlign);
        const alignAttr = (idx: number) =>
          aligns[idx] ? ` style="text-align:${aligns[idx]}"` : "";
        const thead = `<thead><tr>${header
          .map((c, idx) => `<th${alignAttr(idx)}>${renderInline(c)}</th>`)
          .join("")}</tr></thead>`;
        i += 2;
        const body: string[] = [];
        while (i < lines.length) {
          const row = lines[i].trim();
          if (!row || !row.includes("|")) break;
          const cells = splitTableRow(row);
          // 列数向表头归一化：缺则补空、多则截断，保证 table 合法
          const norm = header.map((_, idx) => cells[idx] ?? "");
          body.push(
            `<tr>${norm.map((c, idx) => `<td${alignAttr(idx)}>${renderInline(c)}</td>`).join("")}</tr>`,
          );
          i += 1;
        }
        out.push(`<table>${thead}<tbody>${body.join("")}</tbody></table>`);
        continue;
      }
    }
    const h = t.match(/^(#{1,3})\s+(.*)$/);
    if (h) {
      flushAll();
      const n = h[1].length;
      out.push(`<h${n}>${renderInline(h[2])}</h${n}>`);
      i += 1;
      continue;
    }
    if (/^(-{3,}|\*{3,})$/.test(t)) {
      flushAll();
      out.push("<hr/>");
      i += 1;
      continue;
    }
    const bq = t.match(/^>\s?(.*)$/);
    if (bq) {
      flushPara();
      flushList();
      quote.push(bq[1]);
      i += 1;
      continue;
    }
    const ul = t.match(/^[-*]\s+(.*)$/);
    const ol = t.match(/^(\d+)[.、)]\s+(.*)$/);
    if (ul || ol) {
      flushPara();
      flushQuote();
      const ordered = Boolean(ol);
      let item = ol ? ol[2] : (ul?.[1] ?? "");
      let task: string | null = null;
      const tm = item.match(/^\[([ xX])\]\s+(.*)$/);
      if (tm && !ordered) {
        task = tm[1];
        item = tm[2];
      }
      if (!list || list.ordered !== ordered) {
        flushList();
        list = { ordered, items: [] };
      }
      list.items.push({ text: item, task });
      i += 1;
      continue;
    }
    flushList();
    flushQuote();
    para.push(t);
    i += 1;
  }
  flushCode();
  flushAll();
  return out.join("");
}
