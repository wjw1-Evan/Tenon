// 三栏合并预览（设计方案 §8.6）：你的改动 / 代理改动 / 合并结果（base）。
// 用户三选一：应用合并结果（前端 merge）/ 保留我的 / 应用代理版本。
import { useMemo, useState } from "react";
import { mergeThreeWay } from "../lib/merge";

export interface DirtyConflict {
  path: string;
  base: string;
  ours: string;
  theirs: string;
}

interface Props {
  conflict: DirtyConflict;
  onResolve: (path: string, chosenContent: string, clearBuffer: boolean) => void;
  labels: {
    title: string;
    conflictNote: string;
    ours: string;
    theirs: string;
    base: string;
    apply: string;
    keepMine: string;
    useAgent: string;
  };
}

export function ThreePaneMerge({ conflict, onResolve, labels }: Props) {
  const [merged, mergeError] = useMemo(() => {
    try {
      return [mergeThreeWay(conflict.base, conflict.ours, conflict.theirs), null];
    } catch {
      return [null, "conflict"];
    }
  }, [conflict]);

  const [selected, setSelected] = useState<"merged" | "theirs" | "ours">(merged ? "merged" : "theirs");
  const chosen =
    selected === "ours" ? conflict.ours : selected === "theirs" ? conflict.theirs : merged ?? "";

  return (
    <div className="merge-pane" data-testid="three-pane-merge">
      <div className="merge-head">
        <strong>{labels.title}</strong>
        <code>{conflict.path}</code>
        {mergeError && <span className="muted">{labels.conflictNote}</span>}
      </div>
      <div className="merge-cols">
        <div className="merge-col">
          <div className="merge-col-title">
            <label>
              <input
                type="radio"
                checked={selected === "theirs"}
                onChange={() => setSelected("theirs")}
              />{" "}
              {labels.theirs}
            </label>
          </div>
          <pre data-testid="pane-theirs">{conflict.theirs}</pre>
        </div>
        <div className="merge-col">
          <div className="merge-col-title">
            <label>
              <input
                type="radio"
                checked={selected === "ours"}
                onChange={() => setSelected("ours")}
              />{" "}
              {labels.ours}
            </label>
          </div>
          <pre data-testid="pane-ours">{conflict.ours}</pre>
        </div>
        <div className={`merge-col ${merged ? "" : "disabled"}`}>
          <div className="merge-col-title">
            <label>
              <input
                type="radio"
                disabled={!merged}
                checked={selected === "merged"}
                onChange={() => setSelected("merged")}
              />{" "}
              {labels.base}
            </label>
          </div>
          <pre data-testid="pane-merged">{merged ?? "—"}</pre>
        </div>
      </div>
      <div className="merge-actions">
        <button data-testid="merge-apply" onClick={() => onResolve(conflict.path, chosen, true)}>
          {labels.apply}
        </button>
        <button
          data-testid="merge-keep-mine"
          onClick={() => onResolve(conflict.path, conflict.theirs, false)}
        >
          {labels.keepMine}
        </button>
        <button
          data-testid="merge-use-agent"
          onClick={() => onResolve(conflict.path, conflict.ours, true)}
        >
          {labels.useAgent}
        </button>
      </div>
    </div>
  );
}
