// Playwright 真 daemon 统一回收：global setup 启动的进程只属于本套件。
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { ChildProcess } from "node:child_process";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

interface Runtime {
  daemonPid?: number;
  modelPid?: number;
}

export default async function teardown() {
  try {
    const runtimeFile = path.resolve(__dirname, "../test-results/tenon-runtime.json");
    const runtime = JSON.parse(await readFile(runtimeFile, "utf8")) as Runtime;
    for (const pid of [runtime.daemonPid, runtime.modelPid]) {
      if (typeof pid === "number" && pid > 0) {
        try {
          process.kill(pid, "SIGTERM");
        } catch {
          // 已退出
        }
      }
    }
  } catch {
    // setup 未完成时没有可回收的长活进程
  }
}
