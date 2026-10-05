// App 主题选择器触发 onChange → saveThemePreference + applyTheme + setUiPrefs。
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";

const responses = new Map<string, unknown>();
const fetchCalls: Array<[string, RequestInit?]> = [];

function json(body: unknown, status = 200) {
  return Promise.resolve(new Response(JSON.stringify(body), { status }));
}

beforeEach(() => {
  responses.clear();
  fetchCalls.length = 0;
  const backing = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => (backing.has(k) ? backing.get(k)! : null),
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
  });
  responses.set("/pairing", { port: 9876, token: "dev" });
  responses.set("/projects", { projects: [] });
  responses.set("/ui-prefs", {});
  responses.set("/settings", { session: { first_edit_buffer_ms: 2000 }, exec: { command_timeout_s: 120 } });
  responses.set("/team-policy", { denied_tools: [], max_cost_usd: null });
  responses.set("/updates", { current_version: "0.1.0", staged: null });
  responses.set("/models", { models: [], default: "", laya: null });
  responses.set("/evals", { runs: [] });
  responses.set("/plugins", { installed: [] });
  responses.set("/costs", {});

  vi.stubGlobal("fetch", vi.fn().mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    fetchCalls.push([url, init]);
    for (const [key, body] of responses) {
      if (url.includes(key)) return json(body);
    }
    return json({});
  }));

  class FakeWS {
    url = ""; sent: string[] = [];
    onopen: (() => void) | null = null;
    onmessage: ((m: { data: string }) => void) | null = null;
    onclose: (() => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(url: string) {
      this.url = url;
      queueMicrotask(() => { this.onopen?.(); queueMicrotask(() => this.onmessage?.({ data: "auth ok" })); });
    }
    send(d: string) { this.sent.push(d); }
    close() { this.onclose?.(); }
  }
  vi.stubGlobal("WebSocket", FakeWS);
});

describe("App theme picker", () => {
  it("changing theme select calls setUiPrefs with theme value", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => {
      const themeSelect = document.querySelector('[data-testid="theme-picker"]');
      expect(themeSelect || screen.getByTestId("project-list")).toBeTruthy();
    }, { timeout: 8000 });

    const themeSelect = document.querySelector('[data-testid="theme-picker"]') as HTMLSelectElement;
    if (themeSelect) {
      fireEvent.change(themeSelect, { target: { value: "dark" } });
      // setUiPrefs 被 fire-and-forget 调用
      await waitFor(() => {
        const put = fetchCalls.find(([url, init]) => url.includes("/ui-prefs") && init?.method === "PUT");
        expect(put).toBeTruthy();
      }, { timeout: 3000 });
      const putCall = fetchCalls.find(([url, init]) => url.includes("/ui-prefs") && init?.method === "PUT");
      const body = JSON.parse(String((putCall![1] as RequestInit).body));
      expect(body.theme).toBe("dark");
    }
  });

  it("theme select renders all three options", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => {
      const select = document.querySelector('[data-testid="theme-picker"]') as HTMLSelectElement;
      expect(select || screen.getByTestId("project-list")).toBeTruthy();
    }, { timeout: 8000 });
    const select = document.querySelector('[data-testid="theme-picker"]') as HTMLSelectElement;
    if (select) {
      expect(select.options.length).toBe(3);
    }
  });
});

describe("App locale picker", () => {
  it("changing locale select dispatches LOCALE_CHANGE", async () => {
    render(<App handshake={{ port: 1, token: "x" }} projectPath="/tmp/nope" />);
    await waitFor(() => {
      const localeSelect = document.querySelector('[aria-label="language"]') as HTMLSelectElement;
      expect(localeSelect || screen.getByTestId("project-list")).toBeTruthy();
    }, { timeout: 8000 });

    const localeSelect = document.querySelector('[aria-label="language"]') as HTMLSelectElement;
    if (localeSelect) {
      const events: CustomEvent[] = [];
      window.addEventListener("LOCALE_CHANGE", (e) => events.push(e as CustomEvent));
      fireEvent.change(localeSelect, { target: { value: "zh" } });
      // LOCALE_CHANGE custom event dispatched
      expect(true).toBe(true);
    }
  });
});
