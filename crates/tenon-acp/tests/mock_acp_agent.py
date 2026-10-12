#!/usr/bin/env python3
"""Mock ACP agent（tenon-acp 测试专用）：LSP 式 Content-Length 帧的 JSON-RPC 2.0
stdio 服务端——initialize / session/new / session/prompt（两条 agent_message_chunk
流 + 一条 tool_call update 后收尾）/ session/cancel。环境变量 MOCK_ACP_FAIL
控制 initialize 返回 JSON-RPC 错误（错误路径测试）。"""
import json
import os
import sys


def read_msg():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line or line in (b"\r\n", b"\n"):
            break
        k, _, v = line.decode().partition(":")
        headers[k.strip().lower()] = v.strip()
    n = int(headers.get("content-length", "0"))
    return json.loads(sys.stdin.buffer.read(n))


def write_msg(obj):
    body = json.dumps(obj)
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n{body}".encode())
    sys.stdout.buffer.flush()


def main():
    while True:
        try:
            msg = read_msg()
        except Exception:
            break
        method = msg.get("method", "")
        mid = msg.get("id")
        if method == "initialize":
            if os.environ.get("MOCK_ACP_FAIL"):
                write_msg({"jsonrpc": "2.0", "id": mid,
                           "error": {"code": -32000, "message": "mock init failure"}})
                continue
            write_msg({"jsonrpc": "2.0", "id": mid,
                       "result": {"protocolVersion": 1,
                                  "agentCapabilities": {"loadSession": False}}})
        elif method == "session/new":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": "sess-1"}})
        elif method == "session/prompt":
            sid = msg["params"]["sessionId"]
            for chunk in ("你好，", "任务已完成。"):
                write_msg({"jsonrpc": "2.0", "method": "session/update",
                           "params": {"sessionId": sid,
                                      "update": {"type": "agent_message_chunk",
                                                 "content": {"type": "text",
                                                             "text": chunk}}}})
            write_msg({"jsonrpc": "2.0", "method": "session/update",
                       "params": {"sessionId": sid,
                                  "update": {"type": "tool_call",
                                             "toolCallId": "tc1",
                                             "rawInput": "ls",
                                             "status": "in_progress"}}})
            write_msg({"jsonrpc": "2.0", "id": mid,
                       "result": {"stopReason": "end_turn"}})
        elif method == "session/cancel":
            write_msg({"jsonrpc": "2.0", "id": mid, "result": {}})
        else:
            if mid is not None:
                write_msg({"jsonrpc": "2.0", "id": mid,
                           "error": {"code": -32601, "message": f"unknown {method}"}})


if __name__ == "__main__":
    main()
