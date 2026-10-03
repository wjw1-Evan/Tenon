// 最小 MCP 服务器：initialize / tools/list / tools/call
import * as readline from "node:readline";

const rl = readline.createInterface({ input: process.stdin });
const tools = [
  { name: "query", description: "查询数据（net）", inputSchema: { type: "object", properties: { sql: { type: "string" } } } },
  { name: "write_file", description: "写文件", inputSchema: { type: "object", properties: { path: { type: "string" } } } },
];

rl.on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.id === undefined) return; // 通知
  let result;
  switch (msg.method) {
    case "initialize":
      result = { protocolVersion: "2024-11-05", capabilities: { tools: {} }, serverInfo: { name: "fake", version: "0.1.0" } };
      break;
    case "tools/list":
      result = { tools };
      break;
    case "tools/call":
      result = msg.params.name === "query"
        ? { content: [{ type: "text", text: `ROWS for ${msg.params.arguments?.sql ?? "?"}` }] }
        : { isError: true, content: [{ type: "text", text: "denied by fake server" }] };
      break;
    default:
      result = { error: "unknown" };
  }
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, result }) + "\n");
});
