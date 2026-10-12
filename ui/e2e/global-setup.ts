// Playwright 真 daemon 环境（§18.1）：本地 mock OpenAI server + 隔离 SQLite +
// 静态 UI 由 daemon 托管；不访问外网、不使用全局 ~/.tenon 状态。
import { execSync, spawn, type ChildProcess } from "node:child_process";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const runtimeFile = path.resolve(__dirname, "../test-results/tenon-runtime.json");

let daemon: ChildProcess | null = null;

async function waitForHttp(url: string, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url, { cache: "no-store" });
      if (response.ok) return;
    } catch {
      // daemon 正在启动
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`daemon did not become ready: ${url}`);
}

function startModelServer() {
  return new Promise<{ port: number; pid: number }>((resolve, reject) => {
    const child = spawn(process.execPath, [path.resolve(__dirname, "mock-model-server.mjs")], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let buffered = "";
    const timeout = setTimeout(() => reject(new Error("model server startup timeout")), 10_000);
    child.stdout?.on("data", (chunk) => {
      buffered += String(chunk);
      const line = buffered.split("\n").find((value) => value.trim().startsWith("{"));
      if (!line) return;
      try {
        const parsed = JSON.parse(line) as { modelPort?: number };
        if (parsed.modelPort) {
          clearTimeout(timeout);
          resolve({ port: parsed.modelPort, pid: child.pid ?? 0 });
        }
      } catch {
        // JSON 行尚未完整
      }
    });
    child.stderr?.on("data", (chunk) => process.stderr.write(chunk));
    child.once("exit", (code) => {
      clearTimeout(timeout);
      reject(new Error(`model server exited early (${code})`));
    });
  });
}

export default async function setup() {
  try {
  const workspace = await mkdtemp(path.join(tmpdir(), "tenon-e2e-"));
  const projectA = path.join(workspace, "project-a");
  const projectB = path.join(workspace, "project-b");
  await mkdir(projectA, { recursive: true });
  await mkdir(projectB, { recursive: true });
  await writeFile(path.join(projectA, "a.txt"), "alpha\n");
  await writeFile(path.join(projectB, "b.txt"), "beta\n");

  const modelServer = await startModelServer();
  const configPath = path.join(workspace, "config.toml");
  await writeFile(configPath, `
[session]
first_edit_buffer = 5

[projects]
max_open = 12
max_concurrent_agent_tasks = 2

[models]
default = "mock"

[models.providers.mock]
kind = "openai"
base_url = "http://127.0.0.1:${modelServer.port}/v1"
api_key = "e2e-local"
model = "mock-1"
`);

  const daemonPath = path.resolve(__dirname, "../../target/debug/tenon-daemon");
  // v2.28 新鲜度加固（v2.17 实录：spawn 10-07 陈旧二进制致 gate 字段缺失、
  // 徽标断言落空且无任何报错）：先增量构建（幂等，秒级命中缓存），再校验
  // 二进制 mtime 晚于 crates 最新源——cargo 偶发「Finished 而产物未更新」
  // （sccache / 中断残留）时 fail fast 给出可行动错误。
  try {
    const repoRoot = path.resolve(__dirname, "../..");
    execSync("cargo build -p tenon-daemon", { cwd: repoRoot, stdio: "pipe", timeout: 300_000 });
  } catch (e) {
    throw new Error(`E2E 前置构建 daemon 失败：${e}`);
  }
  {
    let newestSrc = "";
    try {
    newestSrc = execSync(
      // 只比对 src 树——tests/ / benches/ 源不进 daemon 二进制，纳入比对会对
      // 纯测试新增永久误报陈旧（v2.37 permissions_v237_tests 实录）
      "find crates -path '*/src/*.rs' -newer target/debug/tenon-daemon -print -quit",
      { cwd: path.resolve(__dirname, "../.."), encoding: "utf8", timeout: 30_000 },
    ).trim();
    } catch {
      // find 不可用：跳过自检（构建已保证语义新鲜）
    }
    if (newestSrc) {
      throw new Error(
        `daemon 二进制陈旧（构建后仍早于源 ${newestSrc}）——请 cargo clean -p tenon-daemon 后重试`,
      );
    }
  }
  daemon = spawn(daemonPath, [
    "--db", path.join(workspace, "db.sqlite"),
    "--project", projectA,
    "--config", configPath,
    "--settings", path.join(workspace, "settings.json"),
    "--no-lock",
  ], {
    env: { ...process.env, TENON_UI_DIST: path.resolve(__dirname, "../dist"), RUST_LOG: "error" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stderr = "";
  daemon.stderr?.on("data", (chunk) => {
    stderr += String(chunk);
  });
  const handshake = await new Promise<{ port: number; token: string }>((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`daemon handshake timeout\n${stderr}`)), 20_000);
    let buffered = "";
    daemon.stdout?.on("data", (chunk) => {
      buffered += String(chunk);
      const line = buffered.split("\n").find((value) => value.trim().startsWith("{"));
      if (!line) return;
      try {
        const parsed = JSON.parse(line) as { port?: number; token?: string };
        if (parsed.port && parsed.token) {
          clearTimeout(timeout);
          resolve({ port: parsed.port, token: parsed.token });
        }
      } catch {
        // 握手 JSON 行尚未完整
      }
    });
    daemon.once("exit", (code) => reject(new Error(`daemon exited early (${code})\n${stderr}`)));
  });
  await waitForHttp(`http://127.0.0.1:${handshake.port}/health`);

  await mkdir(path.dirname(runtimeFile), { recursive: true });
  await writeFile(runtimeFile, JSON.stringify({
    baseUrl: `http://127.0.0.1:${handshake.port}`,
    daemonPid: daemon.pid,
    modelPid: modelServer.pid,
    projectA,
    projectB,
    workspace,
  }));
  } catch (error) {
    console.error("[tenon-e2e] setup failed", error);
    throw error;
  }
}
