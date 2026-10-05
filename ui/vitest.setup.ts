import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";
import React, { useEffect, useRef } from "react";

// 全局 Monaco Editor mock（jsdom 不支持真实 Monaco worker）：
// 渲染可交互 div，onMount 同步触发，onChange 经 testid 触发。
const fakeEditor = {
  trigger: vi.fn(),
  focus: vi.fn(),
  revealLineInCenter: vi.fn(),
  getModel: () => null,
  createDecorationsCollection: vi.fn(() => ({ clear() {}, set() {} })),
  onDidChangeCursorSelection: vi.fn(() => ({ dispose() {} })),
};

const monacoStub = {
  Uri: { parse: (s: string) => ({ path: s }) },
  editor: {
    defineTheme: vi.fn(),
    registerCommand: vi.fn(() => ({ dispose() {} })),
  },
  languages: new Proxy({}, { get: () => () => ({ dispose() {} }) }),
  KeyMod: {},
  KeyCode: {},
};

vi.mock("@monaco-editor/react", async () => {
  return {
    default: function FakeMonaco({
      path,
      value,
      onMount,
      onChange,
    }: {
      path?: string;
      value?: string;
      onMount?: (editor: unknown, monaco: unknown) => void;
      onChange?: (value: string | undefined) => void;
    }) {
      const called = useRef(false);
      useEffect(() => {
        if (!called.current && onMount) {
          called.current = true;
          onMount(fakeEditor, monacoStub);
        }
      }, [onMount]);
      const testId = `monaco-${path ?? "editor"}`;
      return React.createElement("div", {
        "data-testid": testId,
        "data-value": value,
        onClick: () => onChange?.("mock edit"),
      }, `${path}:${value}`);
    },
    loader: { config: vi.fn() },
  };
});
