// v2.59 对话内容渲染质量：markdownLite 扩展（表格 / 任务清单 / 引用块 / 分隔线 /
// 代码块头部条）与「先转义后标记」XSS 不变式回归。
import { describe, expect, it } from "vitest";
import { renderMarkdownLite } from "../lib/markdownLite";

describe("markdownLite 表格", () => {
  it("表头 + 分隔行 + 体行成表，对齐映射 text-align，单元格行内标记照常解析", () => {
    const html = renderMarkdownLite(
      "| 名称 | 值 |\n| --- | ---: |\n| `x` | **重要** |",
    );
    expect(html).toContain("<table><thead><tr><th>名称</th>");
    expect(html).toContain('<th style="text-align:right">值</th>');
    expect(html).toContain("<td><code>x</code></td>");
    expect(html).toContain('<td style="text-align:right"><strong>重要</strong></td>');
  });

  it("体行列数向表头归一化：缺补空、多截断", () => {
    const html = renderMarkdownLite("| a | b | c |\n| --- | --- | --- |\n| 1 |\n| 2 | 3 | 4 | 5 |");
    const rows = html.match(/<tr>/g) ?? [];
    expect(rows).toHaveLength(3); // 表头一行 + 体两行
    expect((html.match(/<td>/g) ?? []).length + (html.match(/<td style/g) ?? []).length).toBe(6);
  });

  it("无分隔行回退段落（流式半截表格不误判）", () => {
    const html = renderMarkdownLite("| a | b |\n| 1 | 2 |");
    expect(html).not.toContain("<table>");
    expect(html).toContain("<p>");
  });

  it("分隔行列数与表头不一致不判表格（GFM 语义）", () => {
    const html = renderMarkdownLite("a | b\n---");
    expect(html).not.toContain("<table>");
  });
});

describe("markdownLite 任务清单 / 引用块 / 分隔线", () => {
  it("任务清单渲染只读 checkbox，勾选态正确", () => {
    const html = renderMarkdownLite("- [x] 已完成\n- [ ] 待办");
    expect(html).toContain('<li class="task-item"><input type="checkbox" disabled checked/> 已完成</li>');
    expect(html).toContain('<li class="task-item"><input type="checkbox" disabled/> 待办</li>');
  });

  it("普通列表不受影响", () => {
    const html = renderMarkdownLite("- 普通\n- 项");
    expect(html).not.toContain("task-item");
    expect(html).toContain("<li>普通</li>");
  });

  it("连续引用行合并为单个 blockquote", () => {
    const html = renderMarkdownLite("> 第一行\n> 第二行");
    expect((html.match(/<blockquote>/g) ?? []).length).toBe(1);
    expect(html).toContain("第一行<br/>第二行");
  });

  it("--- 与 *** 渲染分隔线", () => {
    expect(renderMarkdownLite("上\n---\n下")).toContain("<hr/>");
    expect(renderMarkdownLite("***")).toContain("<hr/>");
  });
});

describe("markdownLite 代码块头部条（v2.59）", () => {
  it("语言标注 + 复制按钮（文案经参数注入并转义）", () => {
    const html = renderMarkdownLite("```ts\nconst x = 1;\n```", { copyLabel: "复制代码" });
    expect(html).toContain('<div class="code-head"><span class="code-lang">ts</span>');
    expect(html).toContain('<button type="button" class="code-copy">复制代码</button>');
    expect(html).toContain("<pre><code data-lang=\"ts\">const x = 1;</code></pre>");
  });

  it("无语言标注时头部只含复制钮；copyLabel 缺省不渲染钮（向后兼容）", () => {
    const withLang = renderMarkdownLite("```\nplain\n```", { copyLabel: "复制" });
    expect(withLang).not.toContain("code-lang");
    expect(withLang).toContain("code-copy");
    const legacy = renderMarkdownLite("```ts\nconst x = 1;\n```");
    expect(legacy).not.toContain("code-copy");
    expect(legacy).toContain("data-lang=\"ts\"");
  });
});

describe("markdownLite XSS 不变式（v2.59 扩展面同守）", () => {
  const sneaky = '<script>alert(1)</script>';

  it("代码块体 / 语言标注 / 复制按钮文案均转义", () => {
    const html = renderMarkdownLite("```" + sneaky + "\nx\n```", { copyLabel: '"><img>' });
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain('<img>');
  });

  it("表格单元格与引用行转义", () => {
    const html = renderMarkdownLite("| " + sneaky + " | b |\n| --- | --- |\n| x | y |\n> " + sneaky);
    expect(html).not.toContain("<script>");
    expect((html.match(/&lt;script&gt;/g) ?? []).length).toBeGreaterThanOrEqual(2);
  });

  it("既有行为回归：标题 / 链接 / 行内代码占位隔离", () => {
    const html = renderMarkdownLite("## 标题\n[页](https://x.y)\n`**不粗**`");
    expect(html).toContain("<h2>标题</h2>");
    expect(html).toContain('<a href="https://x.y" target="_blank" rel="noreferrer">页</a>');
    expect(html).toContain("<code>**不粗**</code>");
  });
});
