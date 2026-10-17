// Playwright 真 daemon 统一回收：global setup 启动的进程只属于本套件。
// 三层保障（缺一层就有残留窗口）：
//   1. 本 teardown——正常退出的主路径：SIGTERM 优雅停（daemon 自收尾其无头
//      Chrome 子进程），超时升级 SIGKILL，再清临时工作区；
//   2. daemon `--parent-pid` 看门狗（§6.2 v2.88）——Playwright 主进程被强杀 /
//      崩溃时 teardown 不执行，daemon 自持生命周期退出；
//   3. mock `TENON_E2E_PARENT_PID` 同款看门狗——同上，防孤儿 mock 常驻端口。
import { readFile, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

interface Runtime {
  daemonPid?: number;
  modelPid?: number;
  workspace?: string;
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/** SIGTERM → 等待 → SIGKILL：优雅优先，超时兜底，绝不阻塞收尾。 */
async function stop(pid: number, graceMs: number) {
  if (typeof pid !== "number" || pid <= 0 || !alive(pid)) return;
  try {
    process.kill(pid, "SIGTERM");
  } catch {
    return;
  }
  const deadline = Date.now() + graceMs;
  while (Date.now() < deadline) {
    if (!alive(pid)) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  try {
    process.kill(pid, "SIGKILL");
  } catch {
    // 期间已退出
  }
}

export default async function teardown() {
  let runtime: Runtime = {};
  try {
    const runtimeFile = path.resolve(__dirname, "../test-results/tenon-runtime.json");
    runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
  } catch {
    // setup 未完成时没有可回收的长活进程
    return;
  }
  // daemon 先停（优雅收尾自身子进程），mock 后停；各自留 5s 优雅窗口。
  await stop(runtime.daemonPid ?? 0, 5_000);
  await stop(runtime.modelPid ?? 0, 2_000);
  // 临时工作区（含 SQLite / 项目夹具）回收：只认 tenon-e2e- 前缀，绝不误删
  if (runtime.workspace && path.basename(runtime.workspace).startsWith("tenon-e2e-")) {
    await rm(runtime.workspace, { recursive: true, force: true }).catch(() => {});
  }
}
