// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import { DevicePairing } from "@/components/device-pairing";
import { ConnectionScopeProvider } from "@/connections/ConnectionContext";
import { addConnection, resetConnections } from "@/connections/registry";
import type { CompanyFeed } from "@/hooks/use-company";
import { SettingsSection } from "@/views/SettingsSection";
import { SETTINGS_PAGES } from "@/views/settings-pages";

/**
 * Proof for issue #1476: the desktop's pairing prompt names a page that exists.
 *
 * The bug was not a stale instruction left behind by a removed feature. The
 * host had served `GET/POST …/devices` all along; the console simply never
 * built the surface, and the desktop told people to go to "Settings → devices"
 * anyway. Following it led nowhere — and because an unknown sub-page silently
 * falls back to General, a person who typed the route by hand landed on a real
 * page and could not tell whether they were lost or the feature was missing.
 *
 * A unit test of either side alone cannot see that: the prompt renders its
 * sentence happily with no such page, and the Settings section is perfectly
 * consistent without one. So this asserts the *join* — the sentence is parsed
 * off the rendered prompt, and the page it names is asked to render.
 */

/** A client answering the devices read, for the Settings render below. */
const client = {
  scopeFor: () => "/api/v1/companies/acme",
  get: () => Promise.resolve([]),
} as unknown as OpenCompanyClient;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  // `DevicePairing` renders in the desktop build only — in a browser the cookie
  // already works and there is nothing to pair.
  (window as unknown as { __TAURI__: unknown }).__TAURI__ = {};
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  resetConnections();
  delete (window as unknown as { __TAURI__?: unknown }).__TAURI__;
});

/** The prompt, over an https host so the pairing form is offered at all. */
async function showPrompt(): Promise<string> {
  const connection = addConnection({ baseUrl: "https://acme.example", defaultCompany: "acme" });
  await act(async () => {
    root.render(
      createElement(ConnectionScopeProvider, {
        scope: { connection, company: "acme" },
        children: createElement(DevicePairing),
      }),
    );
  });
  return container.textContent ?? "";
}

describe("the desktop pairing prompt", () => {
  it("sends people to a Settings page that exists", async () => {
    const text = await showPrompt();

    const named = /Settings → ([^.]+)\./.exec(text);
    expect(named, `no "Settings → …" direction in: ${text}`).not.toBeNull();

    const page = SETTINGS_PAGES.find((p) => p.label === named![1]);
    expect(page, `Settings has no page called "${named![1]}"`).toBeDefined();
  });

  it("sends them to the page that actually mints a code", async () => {
    // Existing is not enough — it has to be the one with the button on it. The
    // route the page reads is asserted in `devices-view.test.ts`.
    const text = await showPrompt();
    const page = SETTINGS_PAGES.find((p) => p.label === /Settings → ([^.]+)\./.exec(text)![1])!;

    await act(async () => {
      root.render(
        createElement(SettingsSection, {
          client,
          company: "acme",
          feed: { messages: [] } as unknown as CompanyFeed,
          sub: page.id,
          onFlag: () => {},
        }),
      );
    });

    expect(container.querySelector('[data-testid="devices-view"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="devices-pair"]')).not.toBeNull();
  });
});
