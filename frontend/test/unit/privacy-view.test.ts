// @vitest-environment jsdom

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { availableSettingsPages, isSettingsPage, resolveSettingsPage } from "@/views/settings-pages";
import {
  ANALYTICS_COLLECTED,
  ANALYTICS_NEVER_COLLECTED,
  PRIVACY,
} from "@/lib/analytics-copy";
import { PrivacyView } from "@/views/settings/PrivacyView";

import { basePreference, installShell, removeShell } from "./support/analytics-bridge";

/**
 * The Privacy page, driven against a stand-in for the shell's two analytics
 * commands. Under jsdom with no `window.__TAURI__` this is a browser console.
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
    root.render(createElement(PrivacyView));
  });
}
const toggle = () => host.querySelector<HTMLElement>('[data-testid="analytics-toggle"]')!;

describe("the Privacy page is desktop only", () => {
  it("is not an available settings page, and its address falls back, in a browser", () => {
    expect(availableSettingsPages().map((page) => page.id as string)).not.toContain("privacy");
    expect(isSettingsPage("privacy")).toBe(false);
    expect(resolveSettingsPage("privacy")).toBe("general");
  });

  it("is listed and addressable under the desktop shell", () => {
    installShell();
    expect(availableSettingsPages().map((page) => page.id as string)).toContain("privacy");
    expect(isSettingsPage("privacy")).toBe(true);
    expect(resolveSettingsPage("privacy")).toBe("privacy");
  });

  it("lists the rail from the runtime-filtered table, not the raw one", () => {
    // SettingsSection renders General in a browser and needs a whole host
    // around it, so the wiring is pinned at the source; the browser run in
    // `test/e2e` is where the rendered rail is checked.
    const section = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), "../../src/views/SettingsSection.tsx"),
      "utf8",
    );
    expect(section).toContain("availableSettingsPages()");
    expect(section).not.toMatch(/SETTINGS_PAGES\.(map|filter)\(/);
  });
});

describe("PrivacyView", () => {
  it("reads the preference on mount and shows the switch on", async () => {
    const calls = installShell();
    await mount();
    expect(calls[0]).toEqual({ command: "oc_analytics_preference", args: undefined });
    expect(toggle().getAttribute("aria-checked")).toBe("true");
    expect(host.querySelector('[data-testid="analytics-restart-note"]')).toBeNull();
    expect(host.querySelector('[data-testid="analytics-env-note"]')).toBeNull();
  });

  it("lists exactly what is sent and what is never sent", async () => {
    installShell();
    await mount();
    const sent = [...host.querySelectorAll('[data-testid="analytics-collected"] li')].map(
      (li) => li.textContent,
    );
    const never = [...host.querySelectorAll('[data-testid="analytics-never"] li')].map(
      (li) => li.textContent,
    );
    expect(sent).toEqual([...ANALYTICS_COLLECTED]);
    expect(never).toEqual([...ANALYTICS_NEVER_COLLECTED]);
    expect(host.textContent).toContain("IP address");
    expect(host.querySelector('a[href*="analytics-desktop.md"]')).not.toBeNull();
  });

  it("calls oc_set_analytics_preference when the switch is flipped", async () => {
    const calls = installShell();
    await mount();
    await act(async () => {
      toggle().click();
    });
    expect(calls).toContainEqual({
      command: "oc_set_analytics_preference",
      args: { enabled: false },
    });
    expect(toggle().getAttribute("aria-checked")).toBe("false");
    expect(host.textContent).toContain(PRIVACY.statusOff);
  });

  it("explains that opting in applies at the next launch", async () => {
    installShell({ enabled: true, source: "setting", restart_required: true, status: { ...basePreference.status!, decision: "off", last_send: "never", last_at: null, accepted: 0, dropped: 0 } });
    await mount();
    expect(host.querySelector('[data-testid="analytics-restart-note"]')?.textContent).toBe(
      PRIVACY.restartNote,
    );
  });

  it("shows the last send: result, status, time and counts", async () => {
    installShell();
    await mount();
    const status = host.querySelector<HTMLElement>('[data-testid="analytics-last-send"]')!;
    expect(status.dataset.lastSend).toBe("accepted");
    expect(status.textContent).toContain("Accepted by the collector");
    expect(status.textContent).toContain("HTTP 200");
    expect(status.textContent).toContain("Accepted7");
    expect(status.textContent).toContain("Dropped1");
  });

  it("shows a refused credential and a send that never happened in plain words", async () => {
    installShell({ status: { ...basePreference.status!, last_send: "refused-credential", last_status: 401 } });
    await mount();
    expect(host.textContent).toContain("Refused by the collector");
    act(() => root.unmount());
    root = createRoot(host);
    installShell({ status: { ...basePreference.status!, last_send: "never", last_status: null, last_at: null } });
    await mount();
    expect(host.textContent).toContain("Nothing sent yet");
  });

  it("locks the switch and says why when the environment decides", async () => {
    const calls = installShell({
      enabled: false,
      source: "env",
      status: { ...basePreference.status!, decision: "off" },
    });
    await mount();
    expect(host.querySelector('[data-testid="analytics-env-note"]')?.textContent).toContain(
      "OPENCOMPANY_ANALYTICS",
    );
    expect(toggle().hasAttribute("disabled") || toggle().getAttribute("aria-disabled") === "true").toBe(true);
    await act(async () => {
      toggle().click();
    });
    expect(calls.some((c) => c.command === "oc_set_analytics_preference")).toBe(false);
  });
});

describe("PrivacyView in a browser", () => {
  it("makes no shell call at all if it were somehow mounted", async () => {
    // Defence in depth: the wrappers answer `null` without a bridge, so even a
    // stray mount reaches for nothing and reports the read failure.
    await mount();
    expect(host.querySelector('[role="alert"]')?.textContent).toBe(PRIVACY.loadFailed);
  });
});
