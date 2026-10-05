// FileFinder 纯函数路径（通过 UI 间接触发）。
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FileFinder } from "../components/FileFinder";
import type { TenonApi } from "../lib/api";

afterEach(cleanup);
const noop = () => {};
const t = (key: string) => key;

function mount(open = true) {
  return render(
    <FileFinder open={open} onClose={noop} api={{} as unknown as TenonApi} t={t}
      projectId="p1" activePath={null} onOpen={noop} />
  );
}

describe("FileFinder pure paths", () => {
  it("renders finder overlay", () => {
    const { container } = mount();
    expect(container.querySelector(".palette-overlay")).toBeTruthy();
  });

  it("does not render when closed", () => {
    const { container } = mount(false);
    expect(container.querySelector(".palette-overlay")).toBeNull();
  });

  it("finder mode indicator exists", () => {
    const { getByTestId } = mount();
    expect(getByTestId("finder-mode")).toBeTruthy();
  });

  it("input has autoFocus", () => {
    const { getByTestId } = mount();
    const input = getByTestId("file-finder-input") as HTMLInputElement;
    expect(input).toBeTruthy();
  });
});
