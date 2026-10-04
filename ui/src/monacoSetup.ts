// Monaco 本地打包配置（§8.2）：编辑器内核与 worker 全部本地化，
// 不依赖公共 CDN——本地优先（离线 / 内网可用），无供应链风险。
// @monaco-editor/react 的 loader 拿到 monaco 实例后不再走 CDN 路径。
import * as monaco from "monaco-editor";
// 锁定 0.52：使用传统 esm/vs 深路径；升级 0.57+ 时再切换 exports 映射。
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import cssWorker from "monaco-editor/esm/vs/language/css/css.worker?worker";
import htmlWorker from "monaco-editor/esm/vs/language/html/html.worker?worker";
import tsWorker from "monaco-editor/esm/vs/language/typescript/ts.worker?worker";
import { loader } from "@monaco-editor/react";

self.MonacoEnvironment = {
  getWorker(_workerId: string, label: string): Worker {
    if (label === "json") return new jsonWorker();
    if (label === "css" || label === "scss" || label === "less") return new cssWorker();
    if (label === "html" || label === "handlebars" || label === "razor") return new htmlWorker();
    if (label === "typescript" || label === "javascript") return new tsWorker();
    return new editorWorker();
  },
};

loader.config({ monaco });
