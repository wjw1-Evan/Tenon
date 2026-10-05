// 任务完成提示音（§7.5 v1.122）：Agent 回合自运行态转入 done 时播放应用内双音。
// WebAudio 运行时合成（上行两枚正弦短音，总长 <400ms），零音频资源文件；
// 浏览器 autoplay 策略下无用户激活（context suspended）则静默跳过。
import { RUNNING_STATES, type AgentStateName } from "./stateColors";

/** 运行态 → done 才算「任务执行完成」；error / paused / 会话切换重置不响。 */
export function shouldChimeOnTransition(prev: AgentStateName, next: AgentStateName): boolean {
  return RUNNING_STATES.has(prev) && next === "done";
}

let ctx: AudioContext | null = null;

function acquireContext(): AudioContext | null {
  if (ctx) return ctx;
  const Ctor = (window as unknown as { AudioContext?: typeof AudioContext }).AudioContext;
  if (!Ctor) return null; // 无 WebAudio（jsdom / 老内核）：静默跳过
  try {
    ctx = new Ctor();
  } catch {
    return null;
  }
  return ctx;
}

/** 播放完成双音；返回是否真正出声（无 WebAudio 或 autoplay 拦截为 false）。 */
export async function playDoneChime(): Promise<boolean> {
  const ac = acquireContext();
  if (!ac) return false;
  if (ac.state === "suspended") {
    try {
      await ac.resume();
    } catch {
      return false;
    }
    if (ac.state === "suspended") return false;
  }
  const now = ac.currentTime;
  // 上行双音 E5 → A5：短促余韵，指数衰减收尾避免截断爆音
  const notes: ReadonlyArray<readonly [freq: number, at: number]> = [
    [659.25, 0],
    [880, 0.12],
  ];
  for (const [freq, at] of notes) {
    const osc = ac.createOscillator();
    const gain = ac.createGain();
    osc.type = "sine";
    osc.frequency.value = freq;
    gain.gain.setValueAtTime(0, now + at);
    gain.gain.linearRampToValueAtTime(0.12, now + at + 0.02);
    gain.gain.exponentialRampToValueAtTime(0.0001, now + at + 0.28);
    osc.connect(gain).connect(ac.destination);
    osc.start(now + at);
    osc.stop(now + at + 0.3);
  }
  return true;
}

/** 测试专用：丢弃缓存的 AudioContext（vitest 更换 stub 后互不污染）。 */
export function resetChimeForTests(): void {
  ctx = null;
}
