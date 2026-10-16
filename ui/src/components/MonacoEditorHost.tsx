// Monaco 宿主——懒加载边界（§7.5 启动体验）：monaco-editor 内核（约 3.4MB）
// 仅被本模块静态引用；EditorPane（经 App 层 React.lazy）与 ReviewPane 预览
// （组件级 React.lazy）按需拉取本 chunk，主 chunk 不再承载编辑器内核——
// 任务流首屏与局域网配对设备（v1.157）首载显著减负。
// ⚠️ 本模块求值顺序即契约：monacoSetup 的 loader.config 先于任何 Editor
// 挂载（React.lazy 解析完成才渲染），不存在 CDN 回退竞态。
import Editor from "@monaco-editor/react";
import { monaco } from "../monacoSetup";
import { defineTenonThemes } from "./editorThemes";

// 求值即注册主题：审查面只读预览可先于编辑器浮层挂载（v2.47 手动开面板），
// 主题注册不依赖 EditorPane 的 beforeMount 时序。测试环境 monacoSetup 为空
// stub（无 monaco 导出），守卫跳过——真实主题注册由 beforeMount 幂等兜底。
if (monaco) defineTenonThemes(monaco);

export default Editor;
