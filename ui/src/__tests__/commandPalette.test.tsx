// 命令面板键盘可达（§7.4 无障碍）：↑/↓ 选择、Enter 执行当前项、i18n 占位符。
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CommandPalette, type Command } from "../components/CommandPalette";

const commands: Command[] = [
  { id: "a", label: "打开设置", run: vi.fn() },
  { id: "b", label: "切换底部面板", run: vi.fn() },
  { id: "c", label: "停止任务", run: vi.fn() },
];
const t = (key: string) => key;

function renderPalette() {
  render(
    <CommandPalette open onClose={vi.fn()} commands={commands} t={t} />
  );
}

describe("CommandPalette", () => {
  it("↑/↓ 移动高亮，Enter 执行当前项", () => {
    renderPalette();
    const input = screen.getByPlaceholderText("command.placeholder");
    // 初始高亮第一项
    expect(input).toHaveAttribute("placeholder", "command.placeholder");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    const active = document.querySelector(".palette li.active button");
    expect(active).toHaveTextContent("停止任务");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(commands[2].run).toHaveBeenCalled();
  });

  it("过滤后高亮复位，↑ 在首项不再上移", () => {
    renderPalette();
    const input = screen.getByPlaceholderText("command.placeholder");
    fireEvent.change(input, { target: { value: "面板" } });
    const items = document.querySelectorAll(".palette li");
    expect(items).toHaveLength(1);
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(document.querySelector(".palette li.active button")).toHaveTextContent(
      "切换底部面板"
    );
    expect(commands[1].run).not.toHaveBeenCalled();
  });
});
