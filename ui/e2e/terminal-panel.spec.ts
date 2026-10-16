// 项目终端动线 E2E（§7.2 v1.197）：底部面板切终端 tab → xterm 挂载 → 真实
// shell（项目 cwd 的 /bin/sh，WS 票据鉴权）执行命令 → 副作用落盘取证。终端
// 输出渲染在 xterm 画布/DOM 内不可靠断言，故以「命令写文件 → 测试进程直读
// 文件」取证 WS 双向链路（输入到达 shell、stdout 不需回读）。
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

interface Runtime {
  baseUrl: string;
  projectA: string;
  projectB: string;
}

let runtime: Runtime;

test.beforeAll(async () => {
  const runtimeFile = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    "../test-results/tenon-runtime.json"
  );
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

test("project terminal runs a command whose side effect lands in the project", async ({
  page,
}) => {
  await page.goto(runtime.baseUrl);
  await expect(
    page.getByTestId("project-list").getByText("project-a", { exact: true })
  ).toBeVisible();

  // 底部面板 → 终端 tab：xterm 挂载（懒加载 chunk + WS 票据链路）。底栏开合
  // 经项目 ui-state 持久化——上一 spec 留下的展开态下 bottom-open 不存在，兜底。
  const collapsedToggle = page.getByTestId("bottom-open");
  if (await collapsedToggle.count()) await collapsedToggle.click();
  await expect(page.getByTestId("bottom-close")).toBeVisible();
  await page.getByTestId("tab-terminal").click();
  const terminal = page.locator(".terminal-panel .xterm");
  await expect(terminal).toBeVisible({ timeout: 20_000 });

  // 键入命令并回车：经 WS 到达项目 cwd 的 shell
  await terminal.click();
  const proof = "tenon-e2e-terminal";
  await page.keyboard.type(`echo ${proof} > term-proof.txt`);
  await page.keyboard.press("Enter");

  // 取证：文件出现在 project-a 且内容忠实（shell 真实执行）
  const target = path.join(runtime.projectA, "term-proof.txt");
  await expect
    .poll(async () => (await readFile(target, "utf8").catch(() => "")), { timeout: 20_000 })
    .toContain(proof);
});
