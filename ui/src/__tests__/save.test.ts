// 统一保存（§8.2 v1.75）：flush 直通 / 未保存缓冲直写 / 无改动跳过 / 失败保留圆点。
import { describe, expect, it, vi } from "vitest";
import { saveNow } from "../lib/save";
import { createAutoSaver } from "../lib/autosave";

function baseDeps(overrides: Partial<Parameters<typeof saveNow>[1]> = {}) {
  return {
    autosaver: null,
    isUnsaved: () => true,
    getContent: () => "buffer",
    write: vi.fn().mockResolvedValue(undefined),
    onSaved: vi.fn(),
    ...overrides,
  };
}

describe("saveNow", () => {
  it("有待写盘条目走 AutoSaver flush，不重复直写", async () => {
    const saved: Array<[string, string]> = [];
    const autosaver = createAutoSaver(async (p, c) => void saved.push([p, c]), 1000);
    autosaver.schedule("a.ts", "debounced");
    const write = vi.fn().mockResolvedValue(undefined);
    const onSaved = vi.fn();
    await saveNow("a.ts", { autosaver, isUnsaved: () => true, getContent: () => "x", write, onSaved });
    expect(saved).toEqual([["a.ts", "debounced"]]);
    expect(write).not.toHaveBeenCalled();
    // flush 成功后由 AutoSaver 回调解除圆点；saveNow 不再补写
    expect(onSaved).not.toHaveBeenCalled();
    autosaver.dispose();
  });

  it("无待写盘条目时未保存缓冲直接写盘并解除圆点", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    const onSaved = vi.fn();
    await saveNow("b.ts", baseDeps({ write, onSaved }));
    expect(write).toHaveBeenCalledWith("b.ts", "buffer");
    expect(onSaved).toHaveBeenCalledWith("b.ts");
  });

  it("无未保存改动为空操作（不写盘）", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    await saveNow("c.ts", baseDeps({ isUnsaved: () => false, write }));
    expect(write).not.toHaveBeenCalled();
  });

  it("tab 不存在（getContent null）跳过", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    await saveNow("gone.ts", baseDeps({ getContent: () => null, write }));
    expect(write).not.toHaveBeenCalled();
  });

  it("写盘失败不抛出、不解除圆点（下次重试）", async () => {
    const write = vi.fn().mockRejectedValue(new Error("disk full"));
    const onSaved = vi.fn();
    await expect(
      saveNow("d.ts", baseDeps({ write, onSaved }))
    ).resolves.toBeUndefined();
    expect(onSaved).not.toHaveBeenCalled();
  });
});
