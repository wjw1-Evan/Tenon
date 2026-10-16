// Tenon monaco 主题令牌（§7.5 设计令牌同步）：与 styles.css 的 --surface /
// --fg 等变量保持一致；data-theme 切换时联动换主题。
// 独立成模块（不含 monaco 运行时依赖，类型仅编译期）：MonacoEditorHost
// 求值即注册，EditorPane beforeMount 复用同一定义（defineTheme 幂等）。
import type { Monaco } from "@monaco-editor/react";
import type { editor as monacoEditor } from "monaco-editor";

export const TENON_DARK = {
  base: "vs-dark",
  inherit: true,
  rules: [],
  colors: {
    "editor.background": "#14171F",
    "editor.foreground": "#DFE4EE",
    "editorGutter.background": "#14171F",
    "editorLineNumber.foreground": "#39415A",
    "editorLineNumber.activeForeground": "#8B93A7",
    "editor.lineHighlightBackground": "#1A1E28",
    "editorCursor.foreground": "#5C8AF5",
    "editor.selectionBackground": "#2C4A86",
    "editorIndentGuide.background1": "#232938",
    "editorWhitespace.foreground": "#2A3145",
    "editorWidget.background": "#1D2230",
    "editorWidget.border": "#2B3140",
    "scrollbarSlider.background": "#8B93A71F",
    "scrollbarSlider.hoverBackground": "#8B93A733",
    "scrollbarSlider.activeBackground": "#8B93A747",
  },
} satisfies monacoEditor.IStandaloneThemeData;

export const TENON_LIGHT = {
  base: "vs",
  inherit: true,
  rules: [],
  colors: {
    "editor.background": "#FFFFFF",
    "editor.foreground": "#1C2434",
    "editorGutter.background": "#FFFFFF",
    "editorLineNumber.foreground": "#C2C9D6",
    "editorLineNumber.activeForeground": "#5F6B81",
    "editor.lineHighlightBackground": "#F2F4F8",
    "editorCursor.foreground": "#2F6BEC",
    "editor.selectionBackground": "#B9D2FB",
    "editorIndentGuide.background1": "#E8EBF1",
    "editorWhitespace.foreground": "#D8DEE9",
    "editorWidget.background": "#FFFFFF",
    "editorWidget.border": "#D4D9E3",
    "scrollbarSlider.background": "#5F6B811F",
    "scrollbarSlider.hoverBackground": "#5F6B8133",
    "scrollbarSlider.activeBackground": "#5F6B8147",
  },
} satisfies monacoEditor.IStandaloneThemeData;

export function defineTenonThemes(monaco: Monaco) {
  monaco.editor.defineTheme("tenon-dark", TENON_DARK);
  monaco.editor.defineTheme("tenon-light", TENON_LIGHT);
}
