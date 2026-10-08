// 性能遥测（§8.7 / §18.4）：只保留本地样本，不上报； cold start 的终点是
// 代理任务输入框完成两帧渲染（布局 / 合成有机会稳定），可被用户实际键入。

export interface ColdStartSample {
  duration_ms: number;
  recorded_at: string;
}

export interface ColdStartStatus {
  ready: boolean;
  duration_ms?: number;
  p50_ms?: number;
  samples?: ColdStartSample[];
}

export interface CompletionPerfStatus {
  p50_ms?: number;
  latest_ms?: number;
  samples: number[];
}

declare global {
  interface Window {
    __TENON_COLD_START__?: ColdStartStatus;
    __TENON_COMPLETION_PERF__?: CompletionPerfStatus;
  }
}

const STORAGE_KEY = "tenon:performance:coldStart";
const COMPLETION_STORAGE_KEY = "tenon:performance:completion";
const SAMPLE_LIMIT = 10;
const COMPLETION_SAMPLE_LIMIT = 20;

let completionSamples: number[] = [];

/** Storage 禁用时的进程内兜底；也让遥测纯函数可测。 */
let memorySamples: ColdStartSample[] = [];

function readSamples(): ColdStartSample[] {
  if (memorySamples.length) return memorySamples;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    const parsed = raw ? (JSON.parse(raw) as unknown) : [];
    return Array.isArray(parsed)
      ? parsed.filter(
          (item): item is ColdStartSample =>
            !!item &&
            typeof item === "object" &&
            Number.isFinite((item as ColdStartSample).duration_ms) &&
            (item as ColdStartSample).duration_ms >= 0
        )
      : [];
  } catch {
    return [];
  }
}

/** 小样本 P50；偶数取上中位，保证门禁不因取整而偏乐观。 */
export function percentile(samples: number[], ratio: number): number | undefined {
  const values = samples.filter(Number.isFinite).sort((a, b) => a - b);
  if (!values.length) return undefined;
  // floor：偶数个样本时落在上半区间（n=4 → index 2 = 上中位）
  const index = Math.min(values.length - 1, Math.floor(values.length * ratio));
  return values[Math.max(0, index)];
}

/** 测试隔离；应用路径不调用。 */
export function resetColdStartTelemetry(): void {
  memorySamples = [];
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    // storage 禁用时内存已清空
  }
  window.__TENON_COLD_START__ = undefined;
}

/** 供 UI / E2E 读取；同样写入 window，避免 storage 被安全策略禁用后不可见。 */
export function coldStartStatus(): ColdStartStatus {
  const samples = readSamples();
  const latest = samples[samples.length - 1];
  const p50 = percentile(
    samples.map((sample) => sample.duration_ms),
    0.5
  );
  return {
    ready: latest !== undefined,
    duration_ms: latest?.duration_ms,
    p50_ms: p50,
    samples,
  };
}

/** 显式记录 ready；测试 / 嵌入方可注入高精度时间。 */
export function recordWorkspaceInputReady(durationMs: number): ColdStartStatus {
  const safe = Number.isFinite(durationMs) ? Math.max(0, durationMs) : performance.now();
  const sample: ColdStartSample = {
    duration_ms: Math.round(safe * 100) / 100,
    recorded_at: new Date().toISOString(),
  };
  const samples = [...readSamples(), sample].slice(-SAMPLE_LIMIT);
  memorySamples = samples;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(samples));
  } catch {
    // storage 禁用时 window 快照仍然可观测
  }
  const status = coldStartStatus();
  window.__TENON_COLD_START__ = status;
  return status;
}

function nextFrame(): Promise<void> {
  return new Promise((resolve) => {
    if (typeof requestAnimationFrame === "function") {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
      return;
    }
    setTimeout(resolve, 0);
  });
}

/** AgentPanel 任务输入首帧渲染后调用；重复调用为 no-op。 */
export function markWorkspaceInputReady(): void {
  if (window.__TENON_COLD_START__?.ready) return;
  void nextFrame().then(() => {
    if (window.__TENON_COLD_START__?.ready) return;
    recordWorkspaceInputReady(performance.now());
  });
}

function readCompletionSamples(): number[] {
  if (completionSamples.length) return completionSamples;
  try {
    const parsed = JSON.parse(
      localStorage.getItem(COMPLETION_STORAGE_KEY) ?? "[]"
    ) as unknown;
    completionSamples = Array.isArray(parsed)
      ? parsed.filter((value): value is number => Number.isFinite(value) && value >= 0)
      : [];
  } catch {
    completionSamples = [];
  }
  return completionSamples;
}

/** 本地补全呈现遥测：P50 供 §8.7 门禁诊断，不上报。 */
export function recordCompletionPresentation(durationMs: number): CompletionPerfStatus {
  const safe = Number.isFinite(durationMs) ? Math.max(0, durationMs) : 0;
  completionSamples = [...readCompletionSamples(), safe].slice(-COMPLETION_SAMPLE_LIMIT);
  try {
    localStorage.setItem(
      COMPLETION_STORAGE_KEY,
      JSON.stringify(completionSamples)
    );
  } catch {
    // storage 禁用时 window 快照仍然可观测
  }
  const sorted = [...completionSamples].sort((a, b) => a - b);
  const p50 = sorted[Math.ceil(sorted.length / 2) - 1];
  const status: CompletionPerfStatus = {
    latest_ms: completionSamples[completionSamples.length - 1],
    p50_ms: p50,
    samples: [...completionSamples],
  };
  window.__TENON_COMPLETION_PERF__ = status;
  return status;
}

/** 测试隔离；应用路径不调用。 */
export function resetCompletionTelemetry(): void {
  completionSamples = [];
  try {
    localStorage.removeItem(COMPLETION_STORAGE_KEY);
  } catch {
    // 内存已清空
  }
  window.__TENON_COMPLETION_PERF__ = undefined;
}
