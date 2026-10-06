import { createServer } from "node:http";

const server = createServer(async (request, response) => {
  if (request.method !== "POST" || !request.url?.endsWith("/chat/completions")) {
    response.statusCode = 404;
    response.end("not found");
    return;
  }
  let body = "";
  request.on("data", (chunk) => {
    body += chunk;
  });
    request.on("end", async () => {
    try {
      const payload = JSON.parse(body);
      const messages = payload.messages ?? [];
      // 对话标题生成请求（v1.58）：固定短标题，不消耗任务脚本。
      if (messages.some((message) => (message.content ?? "").includes("TENON_TASK_TITLE"))) {
        response.setHeader("Content-Type", "application/json");
        response.end(JSON.stringify({
          choices: [{ message: { role: "assistant", content: "E2E 会话标题" }, finish_reason: "stop" }],
          usage: { prompt_tokens: 1, completion_tokens: 1 },
        }));
        return;
      }
      const hasToolResult = messages.some((message) => message.role === "tool");
      const task = [...messages].reverse().find((message) => message.role === "user")?.content ?? "";
      // E2E_SLOW_TURN_MS=<n>（v1.147 队列测试）：首趟模型调用延迟 n 毫秒——
      // 撑出确定性的「运行中」窗口供发送消息队列断言；仅无工具结果的调用延迟。
      // 标题 / 记忆提取等 meta 调用（请求携带原任务文本会再次命中标记）不吃延迟，
      // 否则回合 Done 后的提取调用被拖长，store 状态已 done 而 run_task 未返回，
      // drain 与 UI 状态时序错位（实测复现）。
      const isMetaCall = messages.some(
        (message) =>
          (message.content ?? "").includes("TENON_TASK_TITLE") ||
          (message.content ?? "").includes("TENON_MEMORY_EXTRACT"),
      );
      const slowMatch = task.match(/E2E_SLOW_TURN_MS=(\d+)/);
      const turnDelay = !isMetaCall && slowMatch ? Number(slowMatch[1]) : 0;
      const match = task.match(/E2E_WRITE\n(.+?)\n([A-Za-z0-9+/=]+)/s);
      if (!match) {
        if (payload.stream) {
          if (turnDelay && !hasToolResult) await new Promise((r) => setTimeout(r, turnDelay));
          const usage = { prompt_tokens: 1, completion_tokens: 1 };
          response.setHeader("Content-Type", "text/event-stream");
          response.write(`data: ${JSON.stringify({
            model: "mock-1",
            choices: [{ delta: { content: "E2E task missing write directive" } }],
          })}\n\n`);
          response.write(`data: ${JSON.stringify({
            model: "mock-1",
            choices: [{ delta: {}, finish_reason: "stop" }],
          })}\n\n`);
          response.write(`data: ${JSON.stringify({ model: "mock-1", choices: [], usage })}\n\n`);
          response.write("data: [DONE]\n\n");
          response.end();
          return;
        }
        response.setHeader("Content-Type", "application/json");
        if (turnDelay && !hasToolResult) await new Promise((r) => setTimeout(r, turnDelay));
        response.end(JSON.stringify({
          choices: [{ message: { role: "assistant", content: "E2E task missing write directive" }, finish_reason: "stop" }],
          usage: { prompt_tokens: 1, completion_tokens: 1 },
        }));
        return;
      }
      const [, file, encoded] = match;
      const content = Buffer.from(encoded, "base64").toString("utf8");
      if (turnDelay && !hasToolResult) await new Promise((r) => setTimeout(r, turnDelay));
      if (!payload.stream) {
        const message = hasToolResult
          ? { role: "assistant", content: "E2E write complete" }
          : {
              role: "assistant",
              tool_calls: [{
                id: `call-${Date.now()}`,
                type: "function",
                function: {
                  name: "apply_patch",
                  arguments: JSON.stringify({ file, range: null, content }),
                },
              }],
            };
        response.setHeader("Content-Type", "application/json");
        response.end(JSON.stringify({
          choices: [{ message, finish_reason: hasToolResult ? "stop" : "tool_use" }],
          usage: { prompt_tokens: 1, completion_tokens: 1 },
        }));
        return;
      }

      const callId = `call-${Date.now()}`;
      const delta = hasToolResult
        ? { content: "E2E write complete" }
        : {
            tool_calls: [{
              index: 0,
              id: callId,
              type: "function",
              function: {
                name: "apply_patch",
                arguments: JSON.stringify({ file, range: null, content }),
              },
            }],
          };
      const usage = { prompt_tokens: 1, completion_tokens: 1 };
      response.setHeader("Content-Type", "text/event-stream");
      response.write(`data: ${JSON.stringify({ model: "mock-1", choices: [{ delta }] })}\n\n`);
      response.write(`data: ${JSON.stringify({
        model: "mock-1",
        choices: [{ delta: {}, finish_reason: hasToolResult ? "stop" : "tool_use" }],
      })}\n\n`);
      response.write(`data: ${JSON.stringify({ model: "mock-1", choices: [], usage })}\n\n`);
      response.write("data: [DONE]\n\n");
      response.end();
    } catch (error) {
      response.statusCode = 500;
      response.end(String(error));
    }
  });
});

server.listen(0, "127.0.0.1", () => {
  const address = server.address();
  if (address && typeof address === "object") {
    console.log(JSON.stringify({ modelPort: address.port }));
  } else {
    console.error("model server has no TCP address");
    process.exit(1);
  }
});
