import { createServer } from "node:http";

// v2.49 持续运行 E2E 相位状态机（模块级；Playwright 单 worker 串行执行，
// 无并发串扰）：E2E_PLAN_RESUME 任务的回合推进为 plan → written → 复位——
//   plan    = 首轮发 submit_plan → 计划暂停（终态 paused，v2.49 起交互会话
//             无回合上限，计划批准是仅存的脚本可控暂停源）；
//   written = 恢复续跑回合首轮（lastUser 为本地化继续语、无标记）：转写入相，
//             复用 apply_patch 管线真实落盘（恢复效果断言）；
//   复位    = 写入后的下一请求文本收尾（E2E resume continuation complete）。
// 关键事实：daemon 每个任务都是全新对话（system + user，session.rs run_task）
// ——续跑回合不携带前回合工具历史（任务文本经 L2 goals 进系统提示），故相位
// 不能跨回合按工具结果数推算，须按「标记 → 无标记」的回合状态机推进。
let loopPhase = null; // null | "loop" | "written"
let loopFile = null;
let loopContent = null;

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
      // v2.6 审查面 E2E：E2E_SLOW_ALL_MS=<n> 对含工具结果的后续调用也延迟——
      // apply_patch 落 diff 后撑出确定的运行窗口（供审查面内容断言）
      const slowAllMatch = task.match(/E2E_SLOW_ALL_MS=(\d+)/);
      const afterToolDelay = !isMetaCall && slowAllMatch ? Number(slowAllMatch[1]) : 0;
      // v2.35 E2E：相 W 需要改写 match（写入载荷来自循环标记而非最后一条 user 消息）
      let match = task.match(/E2E_WRITE\n(.+?)\n([A-Za-z0-9+/=]+)/s);
      // v2.1 档位确认 E2E（design-v2.md §4.1）：D 级 git_commit 触发 Hold
      const commitMatch = task.match(/E2E_GIT_COMMIT\n(.+)/);
      // v2.8 审查摘要卡 E2E：E2E_SUBTASKS\n[{"title","status"}] 两腿脚本——
      // 趟 1（0 个工具结果）发 subtasks 清单，趟 2（恰 1 个工具结果）再走
      // E2E_WRITE 改码，趟 3 文本收尾；驱动审查面子任务卡 + 产物卡真链路
      const subtasksMatch = task.match(/E2E_SUBTASKS\n(\[[^\n]+\])/);
      const toolResultCount = messages.filter((message) => message.role === "tool").length;
      const wantSubtasks = Boolean(subtasksMatch) && toolResultCount === 0;
      const subtasksSecondLeg = Boolean(subtasksMatch) && toolResultCount === 1 && Boolean(match);
      // v2.49 持续运行 E2E：回合状态机推进（详见文件头注释）
      let planCall = false;
      let writtenLeg = false;
      let finalText = null;
      if (!isMetaCall) {
        const loopTask = String(task).match(/E2E_PLAN_RESUME\n(.+?)\n([A-Za-z0-9+/=]+)/s);
        if (loopTask) {
          loopPhase = "plan";
          loopFile = loopTask[1];
          loopContent = Buffer.from(loopTask[2], "base64").toString("utf8");
        } else if (loopPhase === "plan") {
          // 相 written：恢复续跑回合首轮——转写入，复用 apply_patch 管线
          loopPhase = "written";
          writtenLeg = true;
          match = ["", loopFile, Buffer.from(loopContent, "utf8").toString("base64")];
        } else if (loopPhase === "written") {
          // 相复位：写入后的下一请求文本收尾
          loopPhase = null;
          loopFile = null;
          loopContent = null;
          finalText = "E2E resume continuation complete";
        }
        planCall = loopPhase === "plan";
      }
      if (!match && !commitMatch && !wantSubtasks && !(planCall || writtenLeg)) {
        if (payload.stream) {
          const turnWait = hasToolResult ? afterToolDelay : turnDelay;
      if (turnWait) await new Promise((r) => setTimeout(r, turnWait));
          const usage = { prompt_tokens: 1, completion_tokens: 1 };
          response.setHeader("Content-Type", "text/event-stream");
          response.write(`data: ${JSON.stringify({
            model: "mock-1",
            choices: [{ delta: { content: finalText ?? "E2E task missing write directive" } }],
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
        const turnWait = hasToolResult ? afterToolDelay : turnDelay;
      if (turnWait) await new Promise((r) => setTimeout(r, turnWait));
        response.end(JSON.stringify({
          choices: [{ message: { role: "assistant", content: finalText ?? "E2E task missing write directive" }, finish_reason: "stop" }],
          usage: { prompt_tokens: 1, completion_tokens: 1 },
        }));
        return;
      }
      const [, file, encoded] = match ?? [];
      const content = match ? Buffer.from(encoded, "base64").toString("utf8") : "";
      // 计划相 / 写入相必须发工具调用——既有 hasToolResult「有结果即收尾」
      // 启发式会让下一请求直出文本
      const forceToolCall = planCall || writtenLeg;
      const toolName = planCall
        ? "submit_plan"
        : wantSubtasks
          ? "subtasks"
          : commitMatch
            ? "git_commit"
            : "apply_patch";
      const toolArguments = planCall
        ? JSON.stringify({ items: ["改 A 文件支持 X", "补测试并跑通"] })
        : wantSubtasks
          ? JSON.stringify({ items: JSON.parse(subtasksMatch[1]) })
          : commitMatch
            ? JSON.stringify({ message: commitMatch[1] })
            : JSON.stringify({ file, range: null, content });
      const turnWait = hasToolResult ? afterToolDelay : turnDelay;
      if (turnWait) await new Promise((r) => setTimeout(r, turnWait));
      if (!payload.stream) {
        const message = hasToolResult && !subtasksSecondLeg && !forceToolCall
          ? { role: "assistant", content: "E2E write complete" }
          : {
              role: "assistant",
              tool_calls: [{
                id: `call-${Date.now()}`,
                type: "function",
                function: {
                  name: toolName,
                  arguments: toolArguments,
                },
              }],
            };
        response.setHeader("Content-Type", "application/json");
        response.end(JSON.stringify({
          choices: [{ message, finish_reason: hasToolResult && !subtasksSecondLeg && !forceToolCall ? "stop" : "tool_use" }],
          usage: { prompt_tokens: 1, completion_tokens: 1 },
        }));
        return;
      }

      const callId = `call-${Date.now()}`;
      const delta = hasToolResult && !subtasksSecondLeg && !forceToolCall
        ? { content: "E2E write complete" }
        : {
            tool_calls: [{
              index: 0,
              id: callId,
              type: "function",
              function: {
                name: toolName,
                arguments: toolArguments,
              },
            }],
          };
      const usage = { prompt_tokens: 1, completion_tokens: 1 };
      response.setHeader("Content-Type", "text/event-stream");
      response.write(`data: ${JSON.stringify({ model: "mock-1", choices: [{ delta }] })}\n\n`);
      response.write(`data: ${JSON.stringify({
        model: "mock-1",
        choices: [{ delta: {}, finish_reason: hasToolResult && !subtasksSecondLeg && !forceToolCall ? "stop" : "tool_use" }],
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
