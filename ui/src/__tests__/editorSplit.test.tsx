// 多标签分栏（§8.1）：同一项目文件可右栏独立编辑，分栏路径随项目 UI 状态持久化。
// v1.75：bindEditorApi 绑定 active editor 的撤销/重做（命令面板入口）。
import { useEffect, useRef } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { EditorPane, type EditorTab } from "../components/EditorPane";

const { fakeEditor, monacoStub } = vi.hoisted(() => ({
  fakeEditor: {
    trigger: vi.fn(),
    focus: vi.fn(),
    revealLineInCenter: vi.fn(),
    getModel: () => null,
    createDecorationsCollection: () => ({ clear() {}, set() {} }),
    onDidChangeCursorSelection: () => ({ dispose() {} }),
  },
  // registerLanguageProviders 挂 provider（返回 disposable）、Uri.parse、registerCommand。
  monacoStub: {
    Uri: { parse: (s: string) => ({ path: s }) },
    editor: { registerCommand: () => ({ dispose() {} }) },
    languages: new Proxy(
      {},
      { get: () => () => ({ dispose() {} }) }
    ),
  },
}));

vi.mock("@monaco-editor/react", async () => {
  const { useEffect, useRef } = await import("react");
  return {
    default: function FakeMonaco({
      path,
      value,
      onMount,
    }: {
      path: string;
      value: string;
      onMount?: (editor: unknown, monaco: unknown) => void;
    }) {
      const called = useRef(false);
      useEffect(() => {
        if (!called.current && onMount) {
          called.current = true;
          onMount(fakeEditor, monacoStub);
        }
      }, [onMount]);
      return <div data-testid={`monaco-${path}`}>{`${path}:${value}`}</div>;
    },
  };
});

const tabs: EditorTab[] = [
  { path: "src/a.ts", content: "alpha" },
  { path: "src/b.ts", content: "beta" },
  { path: "src/c.ts", content: "gamma" },
];

function renderPane(splitPath: string | null = null, onSelectSplit = vi.fn(), extraProps: Record<string, unknown> = {}) {
  render(
    <EditorPane
      t={(key) => key}
      api={{} as never}
      projectId="project-1"
      tabs={tabs}
      activePath="src/a.ts"
      splitPath={splitPath}
      onSelectSplit={onSelectSplit}
      onSelect={() => {}}
      onClose={() => {}}
      onChange={() => {}}
      {...extraProps}
    />
  );
  return { onSelectSplit };
}

describe("EditorPane split view", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("opens the first non-primary file in a right-hand split", () => {
    const onSelectSplit = vi.fn();
    renderPane(null, onSelectSplit);
    expect(screen.queryByTestId("editor-split-group")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("editor-split"));
    expect(onSelectSplit).toHaveBeenCalledWith("src/b.ts");
  });

  it("renders an independent secondary editor and changes its file", () => {
    const onSelectSplit = vi.fn();
    renderPane("src/b.ts", onSelectSplit);
    expect(screen.getByTestId("editor-split-group")).toBeInTheDocument();
    expect(screen.getByTestId("monaco-src/a.ts")).toHaveTextContent("src/a.ts:alpha");
    expect(screen.getByTestId("monaco-src/b.ts")).toHaveTextContent("src/b.ts:beta");
    fireEvent.change(screen.getByTestId("split-file-select"), {
      target: { value: "src/c.ts" },
    });
    expect(onSelectSplit).toHaveBeenCalledWith("src/c.ts");
  });

  it("closes split from the toolbar", () => {
    const onSelectSplit = vi.fn();
    renderPane("src/b.ts", onSelectSplit);
    fireEvent.click(screen.getByTestId("editor-split"));
    expect(onSelectSplit).toHaveBeenCalledWith(null);
  });

  it("bindEditorApi 绑定 active editor 撤销/重做，卸载解绑（v1.75）", () => {
    const bound: Array<{ undo: () => void; redo: () => void } | null> = [];
    const { unmount } = render(
      <EditorPane
        t={(key) => key}
        api={{} as never}
        projectId="project-1"
        tabs={tabs}
        activePath="src/a.ts"
        onSelect={() => {}}
        onClose={() => {}}
        onChange={() => {}}
        bindEditorApi={(api) => bound.push(api)}
      />
    );
    expect(bound.length).toBe(1);
    bound[0]!.undo();
    expect(fakeEditor.trigger).toHaveBeenCalledWith("palette", "undo", null);
    bound[0]!.redo();
    expect(fakeEditor.trigger).toHaveBeenCalledWith("palette", "redo", null);
    unmount();
    expect(bound.at(-1)).toBeNull();
  });
});
