// 受限 markdown 渲染器（v1.109 会话过程流，设计 §7.5「会话过程流」）。
// 零依赖：全部文本先 HTML 转义（代码块同转义），不解析内嵌 HTML，XSS 面为零。
// 支持集合：#~### 标题 / 无序有序列表 / 行内代码 / 围栏代码块 / **粗体** *斜体* / http(s) 链接 / 段落。
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

interface ListGroup {
  ordered: boolean;
  items: string[];
}

export function renderMarkdownLite(md: string): string {
  if (!md.trim()) return "";
  const lines = md.replace(/\r\n?/g, "\n").split("\n");
  const out: string[] = [];
  let para: string[] = [];
  let list: ListGroup | null = null;
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
        `<${tag}>${list.items.map((i) => `<li>${renderInline(i)}</li>`).join("")}</${tag}>`,
      );
      list = null;
    }
  };
  const flushCode = () => {
    if (code) {
      // 流式草稿常见未闭合围栏：按已收到的内容直接成块。
      const langAttr = code.lang ? ` data-lang="${escapeHtml(code.lang)}"` : "";
      out.push(`<pre><code${langAttr}>${escapeHtml(code.lines.join("\n"))}</code></pre>`);
      code = null;
    }
  };

  for (const line of lines) {
    if (code) {
      if (/^```/.test(line.trim())) {
        flushCode();
      } else {
        code.lines.push(line);
      }
      continue;
    }
    const fence = line.trim().match(/^```(\S*)\s*$/);
    if (fence) {
      flushPara();
      flushList();
      code = { lang: fence[1] ?? "", lines: [] };
      continue;
    }
    const t = line.trim();
    if (!t) {
      flushPara();
      flushList();
      continue;
    }
    const h = t.match(/^(#{1,3})\s+(.*)$/);
    if (h) {
      flushPara();
      flushList();
      const n = h[1].length;
      out.push(`<h${n}>${renderInline(h[2])}</h${n}>`);
      continue;
    }
    const ul = t.match(/^[-*]\s+(.*)$/);
    const ol = t.match(/^(\d+)[.、)]\s+(.*)$/);
    if (ul || ol) {
      flushPara();
      const ordered = Boolean(ol);
      const item = ol ? ol[2] : (ul?.[1] ?? "");
      if (!list || list.ordered !== ordered) {
        flushList();
        list = { ordered, items: [] };
      }
      list.items.push(item);
      continue;
    }
    flushList();
    para.push(t);
  }
  flushCode();
  flushPara();
  flushList();
  return out.join("");
}
