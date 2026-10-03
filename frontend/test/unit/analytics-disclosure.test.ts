// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { AnalyticsDisclosure } from "@/components/analytics-disclosure";
import { DISCLOSURE } from "@/lib/analytics-copy";

import { installShell, removeShell } from "./support/analytics-bridge";

/**
 * The first-launch notice. "Shown once" is the shell's own `source`: `default`
 * until somebody chooses, `setting` after, so the stand-in flips it on a save
 * and a second mount is the next launch.
 */

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLElement;
let root: Root;

beforeEach(() => {
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});
afterEach(() => {
  act(() => root.unmount());
  host.remove();
  removeShell();
});

async function mount() {
  await act(async () => {
    root.render(createElement(AnalyticsDisclosure));
  });
}
const notice = () => host.querySelector('[data-testid="analytics-disclosure"]');
const button = (id: string) =>
  host.querySelector<HTMLButtonElement>(`[data-testid="analytics-disclosure-${id}"]`)!;

describe("AnalyticsDisclosure", () => {
  it("renders nothing and calls nothing in a browser", async () => {
    await mount();
    expect(notice()).toBeNull();
  });

  it("is shown on first launch with the copy and both actions", async () => {
    installShell();
    await mount();
    expect(notice()).not.toBeNull();
    expect(host.textContent).toContain(DISCLOSURE.title);
    expect(host.textContent).toContain("Settings → Privacy");
    expect(button("got-it").textContent).toBe("Got it");
    expect(button("turn-off").textContent).toBe("Turn off");
  });

  it("Got it dismisses it and is remembered across a relaunch", async () => {
    const calls = installShell();
    await mount();
    await act(async () => button("got-it").click());
    expect(notice()).toBeNull();
    expect(calls).toContainEqual({ command: "oc_set_analytics_preference", args: { enabled: true } });

    // The next launch: a fresh mount against the same shell state.
    act(() => root.unmount());
    root = createRoot(host);
    await mount();
    expect(notice()).toBeNull();
  });

  it("Turn off opts out, dismisses it, and stays dismissed", async () => {
    const calls = installShell();
    await mount();
    await act(async () => button("turn-off").click());
    expect(calls).toContainEqual({ command: "oc_set_analytics_preference", args: { enabled: false } });
    expect(notice()).toBeNull();

    act(() => root.unmount());
    root = createRoot(host);
    await mount();
    expect(notice()).toBeNull();
  });

  it("is not shown once the person has chosen, or when the environment decides", async () => {
    installShell({ source: "setting" });
    await mount();
    expect(notice()).toBeNull();
    act(() => root.unmount());
    root = createRoot(host);
    installShell({ source: "env", enabled: false });
    await mount();
    expect(notice()).toBeNull();
  });

  it("stays and says so when the choice could not be saved", async () => {
    installShell();
    const shell = (window as unknown as { __TAURI__: { core: { invoke: unknown } } }).__TAURI__;
    shell.core.invoke = async (command: string) => {
      if (command === "oc_set_analytics_preference") throw new Error("disk full");
      return { enabled: true, source: "default", restart_required: false, status: null };
    };
    await mount();
    await act(async () => button("got-it").click());
    expect(notice()).not.toBeNull();
    expect(host.textContent).toContain(DISCLOSURE.failed);
  });
});
