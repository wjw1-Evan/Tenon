// §8.7 / §18.4：WebView 冷启动到任务输入可键入的 P50 验收门。
// daemon / mock model server 由 global setup 预启动，预算只覆盖 WebView 导航。
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test, type BrowserType } from "@playwright/test";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

interface Runtime {
  baseUrl: string;
}

interface ColdStartStatus {
  ready: boolean;
  duration_ms?: number;
}

let runtime: Runtime;

test.beforeAll(async () => {
  const runtimeFile = path.resolve(
    __dirname,
    "../test-results/tenon-runtime.json"
  );
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

async function measureColdStart(browser: BrowserType): Promise<number> {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto(runtime.baseUrl, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    () => Boolean((window as { __TENON_COLD_START__?: ColdStartStatus }).__TENON_COLD_START__?.ready),
    undefined,
    { timeout: 10_000, polling: 25 }
  );
  const status = (await page.evaluate(
    () => (window as { __TENON_COLD_START__?: ColdStartStatus }).__TENON_COLD_START__
  )) as ColdStartStatus;
  await context.close();
  expect(status.ready).toBe(true);
  expect(typeof status.duration_ms).toBe("number");
  return status.duration_ms!;
}

test("cold start reaches the task input within 1.5s P50", async ({ browser }) => {
  test.setTimeout(45_000);
  const samples: number[] = [];
  for (let index = 0; index < 5; index += 1) {
    samples.push(await measureColdStart(browser));
  }
  const sorted = [...samples].sort((a, b) => a - b);
  const p50 = sorted[Math.ceil(sorted.length / 2) - 1];
  console.info(`[startup-performance] samples=${samples.join(",")} p50=${p50}ms`);
  expect(p50).toBeLessThan(1_500);
});
