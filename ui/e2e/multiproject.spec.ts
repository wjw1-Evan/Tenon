// 真 daemon 多项目 E2E（§18.1 / §18.2）：A 任务直执/回滚，B 并行隔离。
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

interface Runtime {
  baseUrl: string;
  daemonPid?: number;
  modelPid?: number;
  projectA: string;
  projectB: string;
}

let runtime: Runtime;

test.beforeAll(async () => {
  const runtimeFile = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../test-results/tenon-runtime.json");
  runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
});

test("concurrent projects keep writes and rollbacks isolated", async ({ page }) => {
  test.setTimeout(90_000);
  page.on("dialog", (dialog) => void dialog.accept());
  await page.goto(runtime.baseUrl);
  // v1.63 项目文件夹树常驻：登记项目无需打开下拉即可见
  await expect(page.getByTestId("project-list").getByText("project-a", { exact: true })).toBeVisible();

  // 登记 B：ProjectRuntime 由 daemon 隐式激活；当前 UI 切到 B。
  // 断言窗口 20s：共享开发机高负载（load 40+）下 daemon L4 索引 / 会话建立
  // 可超默认 5s（健康环境满足即返回，不增加通过耗时）。
  await page.getByTestId("project-add").click();
  await page.getByTestId("project-add-path").fill(runtime.projectB);
  await page.getByTestId("project-add-form").getByRole("button", { name: "Open" }).click();
  await expect(
    page.getByTestId("project-list").getByText("project-b", { exact: true }).locator("xpath=ancestor::div[1]")
  ).toHaveClass(/active/, { timeout: 20_000 });

  // 登记即用：切回 A 执行首个写入 / 回滚链路（点击文件夹行即切换并展开）。
  await page.getByTestId("project-list").getByText("project-a", { exact: true }).click();

  // A 写入任务：v1.89 直接执行并落盘。
  const taskA = [
    "E2E_WRITE",
    "e2e-a.txt",
    Buffer.from("written by project A\n").toString("base64"),
  ].join("\n");
  await page.getByTestId("task-input").fill(taskA);
  await page.getByTestId("send").click();
  await expect(page.getByTestId("agent-feed")).toContainText("E2E write complete", { timeout: 30_000 });
  await expect.poll(async () => readFile(path.join(runtime.projectA, "e2e-a.txt"), "utf8").catch(() => ""), {
    timeout: 10_000,
  }).toContain("written by project A");

  // Checkpoint 回滚：磁盘恢复到任务前；unrevert 恢复写入语义。
  // v1.78 后底栏默认收起；细条入口保留稳定 testid。
  // 点时间轴最后一个 Roll back：时间轴反序渲染（新→旧），最新节点是写入完成后的
  // 树快照（v1.111 起 checkpoint_rollback 真按 id 恢复，恢复它=无效果）；
  // 最旧节点 = 任务前快照，恢复它才撤销本任务的写入效果。
  await page.getByTestId("bottom-open").click();
  const rollback = page.locator(".timeline-node").getByRole("button", { name: /Roll back|Rollback|回滚/ }).last();
  await expect(rollback).toBeVisible();
  await rollback.click();
  await expect.poll(async () => readFile(path.join(runtime.projectA, "e2e-a.txt"), "utf8").catch(() => ""), {
    timeout: 10_000,
  }).not.toContain("written by project A");
  await page.getByRole("button", { name: /Undo last rollback|撤销最近回滚/ }).click();
  await expect.poll(async () => readFile(path.join(runtime.projectA, "e2e-a.txt"), "utf8").catch(() => ""), {
    timeout: 10_000,
  }).toContain("written by project A");

  // B 执行上下文仍可操作，且磁盘未被 A 任务污染。
  await page.getByTestId("project-list").getByText("project-b", { exact: true }).click();
  await page.getByTestId("task-input").fill("summarize project B without edits");
  await page.getByTestId("send").click();
  await expect(page.getByTestId("agent-feed")).toContainText("missing write directive", { timeout: 30_000 });
  await expect(readFile(path.join(runtime.projectB, "b.txt"), "utf8")).resolves.toBe("beta\n");
  await expect(readFile(path.join(runtime.projectB, "e2e-a.txt"), "utf8")).rejects.toThrow();
});
