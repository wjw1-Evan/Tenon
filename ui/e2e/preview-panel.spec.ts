// 嵌入式浏览器预览动线 E2E（§7.6 v2.77，真 daemon 反代 + 真控制台桥）：面板经
// /preview/token 取反代前缀 → daemon 反代（preview.rs）访问回环 dev server →
// 剥嵌入限制 + 根绝对 URL 改写 + 注入控制台桥 → iframe 呈现；console / 导航
// 事件经 postMessage 桥回流面板。此前仅 vitest（jsdom mock api）与 preview.rs
// Rust 单测覆盖解析 / 改写 / 桥注入各自一段，浏览器级串联（反代取页 → 桥回流
// → 面板状态）无回归。
// 本 spec 自起一个回环静态 server（listen(0) 随机端口）充当 dev server，全程
// 无外网；spec 注释口径同 v2.12：共享 daemon 下自造数据、不留跨 spec 依赖。
import { createServer, type Server } from "node:http";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test, type Page } from "@playwright/test";

interface Runtime {
  baseUrl: string;
}

let runtime: Runtime;

const INDEX_HTML = `<!doctype html><html><head><title>e2e-preview</title></head>
<body><h1 id="title">E2E preview page</h1>
<a id="sub-link" href="/sub">go sub</a>
<script>
  console.log("E2E preview log ready");
  console.error("E2E preview error entry");
</script></body></html>`;

const SUB_HTML = `<!doctype html><html><head><title>e2e-preview-sub</title></head>
<body><h2 id="sub-title">E2E preview sub page</h2></body></html>`;

let dev: Server;
let devPort = 0;

test.beforeAll(async () => {
  const runtimeFile = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../test-results/tenon-runtime.json",
  );
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
  dev = createServer((req, res) => {
    const route = (req.url ?? "/").split("?")[0];
    if (route !== "/" && route !== "/sub") {
      res.statusCode = 404;
      res.end("not found");
      return;
    }
    res.setHeader("Content-Type", "text/html; charset=utf-8");
    res.end(route === "/sub" ? SUB_HTML : INDEX_HTML);
  });
  await new Promise<void>((resolve) => dev.listen(0, "127.0.0.1", resolve));
  devPort = (dev.address() as { port: number }).port;
});

test.afterAll(async () => {
  await new Promise<void>((resolve) => dev.close(() => resolve()));
});

/** 打开工作区并切到预览 tab。底栏开合与 tab 选择经项目 ui-state 持久化——
 *  上一 spec 留下的展开态下 bottom-open 不存在，先兜底（checkpoint spec 同款）。 */
async function openPreviewTab(page: Page) {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();
  const collapsedToggle = page.getByTestId("bottom-open");
  if (await collapsedToggle.count()) await collapsedToggle.click();
  await expect(page.getByTestId("bottom-close")).toBeVisible();
  await page.getByTestId("tab-preview").click();
  await expect(page.getByTestId("preview-panel")).toBeVisible();
}

/** 回到时间线 tab 收尾：不留 preview 为项目持久 tab，防后续 spec 断言落空。 */
async function restoreTimelineTab(page: Page) {
  await page.locator(".bottom-tabs button").first().click();
  await expect(page.getByTestId("preview-panel")).toHaveCount(0);
}

test("invalid targets are rejected with a readable loopback-only error", async ({
  page,
}) => {
  await openPreviewTab(page);
  await expect(page.getByTestId("preview-empty")).toBeVisible();

  // 非回环主机：前端提前拦截（§7.6 约束与 daemon 侧同源），给可读错误而非 502
  await page.getByTestId("preview-url").fill("example.com:3000");
  await page.getByTestId("preview-go").click();
  await expect(page.getByTestId("preview-error")).toContainText("Invalid address");
  await expect(page.getByTestId("preview-frame")).toHaveCount(0);

  // 缺显式端口同样非法（反代路径需端口段）
  await page.getByTestId("preview-url").fill("localhost");
  await page.getByTestId("preview-go").click();
  await expect(page.getByTestId("preview-error")).toContainText("Invalid address");
  await expect(page.getByTestId("preview-frame")).toHaveCount(0);
  await restoreTimelineTab(page);
});

test("loopback dev server renders through the daemon proxy with bridged console", async ({
  page,
}) => {
  await openPreviewTab(page);
  await page.getByTestId("preview-url").fill(String(devPort));
  await page.getByTestId("preview-go").click();

  // iframe 经 daemon 反代取回环页：正文真实呈现（剥嵌入限制后可嵌）
  const frame = page.frameLocator('[data-testid="preview-frame"]');
  await expect(frame.locator("h1")).toContainText("E2E preview page", {
    timeout: 20_000,
  });
  await expect(page.getByTestId("preview-error")).toHaveCount(0);

  // 导航桥：iframe 加载完成后 nav 消息回填当前地址（反代路径含端口段）
  await expect(page.getByTestId("preview-address")).toContainText(
    `/proxy/${devPort}/`,
    { timeout: 10_000 }
  );

  // 控制台桥：页面脚本 console.log / console.error 回流面板（分级落行）
  await expect(page.getByTestId("preview-log-log")).toContainText(
    "E2E preview log ready",
    { timeout: 10_000 }
  );
  await expect(page.getByTestId("preview-log-error")).toContainText(
    "E2E preview error entry"
  );
  await restoreTimelineTab(page);
});

test("tab switch restores the saved target and reload recaptures console", async ({
  page,
}) => {
  await openPreviewTab(page);
  await page.getByTestId("preview-url").fill(String(devPort));
  await page.getByTestId("preview-go").click();
  const frame = page.frameLocator('[data-testid="preview-frame"]');
  await expect(frame.locator("h1")).toContainText("E2E preview page", {
    timeout: 20_000,
  });

  // 切走再切回：目标地址按项目存 localStorage，重挂载自动重开（免手输）
  await page.locator(".bottom-tabs button").first().click();
  await expect(page.getByTestId("preview-panel")).toHaveCount(0);
  await page.getByTestId("tab-preview").click();
  await expect(
    page.frameLocator('[data-testid="preview-frame"]').locator("h1")
  ).toContainText("E2E preview page", { timeout: 20_000 });
  await expect(page.getByTestId("preview-url")).toHaveValue(String(devPort));

  // 清空控制台 → 刷新 iframe：桥随页重注入，日志重新回流
  await page.getByTestId("preview-console-clear").click();
  await expect(page.getByTestId("preview-console")).toContainText(
    "No console output yet"
  );
  await page.getByTestId("preview-reload").click();
  await expect(page.getByTestId("preview-log-log")).toContainText(
    "E2E preview log ready",
    { timeout: 10_000 }
  );

  // 页内链接导航：反代路径改写后命中 /sub，nav 桥回填新地址
  await page
    .frameLocator('[data-testid="preview-frame"]')
    .locator("a")
    .click();
  await expect(
    page.frameLocator('[data-testid="preview-frame"]').locator("h2")
  ).toContainText("E2E preview sub page", { timeout: 10_000 });
  await expect(page.getByTestId("preview-address")).toContainText(
    `/proxy/${devPort}/sub`,
    { timeout: 10_000 }
  );
  await restoreTimelineTab(page);
});
