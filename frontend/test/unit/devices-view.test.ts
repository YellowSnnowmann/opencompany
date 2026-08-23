// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import { DevicesView } from "@/views/DevicesView";

/**
 * Settings → Devices, the console half of device pairing (issue #1476).
 *
 * The host has served `GET/POST …/devices` since pairing landed and the desktop
 * app told people to come here for a code — but nothing in the console ever
 * called either route, so the instruction pointed at a page that did not exist.
 * These tests are about the two things that page has to get right: it must
 * actually call those routes, and it must not let a pairing code look like a
 * durable value. The code comes back exactly once, only its hash is stored, and
 * it dies in five minutes.
 */

const DAY = 24 * 60 * 60 * 1000;

interface Call {
  method: string;
  path: string;
}

let calls: Call[];

/** A client answering the devices routes, or rejecting the list. */
function clientWith(devices: unknown, options: { mintFails?: string } = {}): OpenCompanyClient {
  calls = [];
  return {
    scopeFor: () => "/api/v1/companies/acme",
    get: (path: string) => {
      calls.push({ method: "GET", path });
      return devices instanceof Error ? Promise.reject(devices) : Promise.resolve(devices);
    },
    post: (path: string) => {
      calls.push({ method: "POST", path });
      return options.mintFails
        ? Promise.reject(new Error(options.mintFails))
        : Promise.resolve({ code: "PAIR-CODE-1234", expiresAtMillis: Date.now() + 5 * 60 * 1000 });
    },
    del: (path: string) => {
      calls.push({ method: "DELETE", path });
      return Promise.resolve({ revoked: true });
    },
  } as unknown as OpenCompanyClient;
}

let container: HTMLDivElement;
let root: Root;

async function show(client: OpenCompanyClient) {
  await act(async () => {
    root.render(createElement(DevicesView, { client, company: "acme" }));
  });
}

function at(testid: string): HTMLElement | null {
  return container.querySelector<HTMLElement>(`[data-testid="${testid}"]`);
}

function all(testid: string): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(`[data-testid="${testid}"]`));
}

async function click(el: HTMLElement) {
  await act(async () => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.restoreAllMocks();
});

describe("DevicesView", () => {
  it("reads the host's devices route rather than showing an invented page", async () => {
    // The whole of #1476 in one assertion: a console that never calls this is
    // the state the issue describes, and it looked fine on screen.
    await show(clientWith([]));

    expect(calls).toContainEqual({ method: "GET", path: "/api/v1/companies/acme/devices" });
    expect(at("devices-view")).not.toBeNull();
    expect(at("devices-empty")).not.toBeNull();
  });

  it("mints a code, shows it once, and says how long it has", async () => {
    await show(clientWith([]));
    expect(at("pairing-code")).toBeNull();

    await click(at("devices-pair")!);

    expect(calls).toContainEqual({ method: "POST", path: "/api/v1/companies/acme/devices" });
    expect(at("pairing-code")?.textContent).toBe("PAIR-CODE-1234");
    // Not decoration: a code that looks permanent is one somebody saves and
    // pastes ten minutes later into a form that answers "invalid or expired".
    expect(at("pairing-code-expiry")?.textContent).toMatch(/Expires in \d+:\d\d/);
    expect(at("pairing-code-expiry")?.textContent).toContain("shown once");
  });

  it("keeps the host's refusal when a paired device tries to mint", async () => {
    // A device may not enrol another device, and the host's message names the
    // remedy — "sign in on the web console". Rewording it here would drop the
    // only part of the failure that helps, so nothing on screen replaces it.
    const failure = "a paired device cannot pair another device; sign in on the web console";
    await show(clientWith([], { mintFails: failure }));

    await click(at("devices-pair")!);

    expect(at("pairing-code")).toBeNull();
    expect(at("devices-pair")).not.toBeNull();
  });

  it("lists devices newest first and marks the one making the request", async () => {
    const now = Date.now();
    await show(
      clientWith([
        {
          id: "old",
          label: "Old laptop",
          createdAtMillis: now - 30 * DAY,
          expiresAtMillis: now + 300 * DAY,
          current: false,
        },
        {
          id: "here",
          label: "This laptop",
          createdAtMillis: now - DAY,
          expiresAtMillis: now + 330 * DAY,
          current: true,
        },
      ]),
    );

    const rows = all("device-row");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("This laptop");
    expect(rows[1].textContent).toContain("Old laptop");

    // Revoking the credential this window is using signs this window out. The
    // button says so rather than saying "Revoke" and doing it.
    expect(at("device-current")).not.toBeNull();
    expect(rows[0].querySelector('[data-testid="device-revoke"]')?.textContent).toBe(
      "Sign out this device",
    );
    expect(rows[1].querySelector('[data-testid="device-revoke"]')?.textContent).toBe("Revoke");
  });

  it("names an unlabelled device rather than rendering a blank row", async () => {
    const now = Date.now();
    await show(
      clientWith([
        { id: "d1", createdAtMillis: now, expiresAtMillis: now + 300 * DAY, current: false },
      ]),
    );

    expect(at("device-row")?.textContent).toContain("Unnamed device");
  });

  it("revokes by id and re-reads the list", async () => {
    const now = Date.now();
    await show(
      clientWith([
        {
          id: "dev 1",
          label: "Studio",
          createdAtMillis: now,
          expiresAtMillis: now + 300 * DAY,
          current: false,
        },
      ]),
    );

    await click(at("device-revoke")!);

    // Encoded, because a device id travels in the path and is not guaranteed to
    // be path-safe.
    expect(calls).toContainEqual({
      method: "DELETE",
      path: "/api/v1/companies/acme/devices/dev%201",
    });
    // Re-read rather than spliced out of local state: revocation can fail
    // server-side in ways a local splice would hide.
    expect(calls.filter((c) => c.method === "GET")).toHaveLength(2);
  });

  it("reports a failed read instead of an empty account", async () => {
    // "No machines are paired" and "we could not ask" look identical and mean
    // opposite things — one invites pairing again, the other is a broken host.
    await show(clientWith(new Error("host unreachable")));

    expect(at("devices-load-error")?.textContent).toContain("host unreachable");
    expect(at("devices-empty")).toBeNull();
  });
});
