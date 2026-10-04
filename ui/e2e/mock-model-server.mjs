import { createServer } from "node:http";

const server = createServer((request, response) => {
  if (request.method !== "POST" || !request.url?.endsWith("/chat/completions")) {
    response.statusCode = 404;
    response.end("not found");
    return;
  }
  let body = "";
  request.on("data", (chunk) => {
    body += chunk;
  });
  request.on("end", () => {
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
      const match = task.match(/E2E_WRITE\n(.+?)\n([A-Za-z0-9+/=]+)/s);
      if (!match) {
        if (payload.stream) {
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
        response.end(JSON.stringify({
          choices: [{ message: { role: "assistant", content: "E2E task missing write directive" }, finish_reason: "stop" }],
          usage: { prompt_tokens: 1, completion_tokens: 1 },
        }));
        return;
      }
      const [, file, encoded] = match;
      const content = Buffer.from(encoded, "base64").toString("utf8");
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
