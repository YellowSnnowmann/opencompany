// @vitest-environment jsdom

import { act, createElement, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { ManageHostsPage } from "@/components/manage-hosts";
import { HostsProvider, useHosts, type HostsValue } from "@/connections/HostsContext";
import type { Connection } from "@/connections/types";

/**
 * The manage page as it is actually mounted: under `HostsProvider`, beside a
 * console rather than inside it.
 *
 * The exported predicates are unit-tested next door; what this covers is the
 * wiring those cannot reach — that the page draws only when the switcher asked
 * for it, that a host this client does not own offers no buttons that would
 * undo themselves, and that "forget" is confirmed rather than immediate.
 */

function host(over: Partial<Connection> = {}): Connection {
  return {
    id: "conn-remote",
    defaultCompany: null,
    label: "Acme",
    baseUrl: "https://acme.test",
    credential: { kind: "cookie" },
    status: "live",
    identity: null,
    companies: [],
    connector: { kind: "remote" },
    ...over,
  };
}

let container: HTMLDivElement;
let root: Root;
const removed: string[] = [];

function value(connections: Connection[]): HostsValue {
  return {
    connections,
    selected: connections[0]?.id ?? null,
    onSelect: () => {},
    onAdd: () => {},
    onEditHost: () => {},
    onRemoveHost: (id) => removed.push(id),
    localInstances: [],
    hub: false,
  };
}

async function show(connections: Connection[], open: boolean) {
  await act(async () => {
    root.render(
      createElement(HostsProvider, {
        value: value(connections),
        children: createElement(Opener, { open }),
      }),
    );
  });
}

/**
 * The switcher's one job here: raising the flag the page renders from.
 *
 * Rendered inside the provider, because that is the only place the flag lives
 * — it is deliberately not owned by the page, whose own actions remount the
 * console around it.
 */
function Opener({ open }: { open: boolean }) {
  const hosts = useHosts();
  useEffect(() => {
    hosts.setManagingHosts(open);
  }, [hosts, open]);
  return createElement(ManageHostsPage);
}

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  removed.length = 0;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

describe("the manage-hosts page", () => {
  it("draws nothing until the switcher asks for it", async () => {
    await show([host()], false);
    expect(container.querySelector('[data-testid="manage-hosts"]')).toBeNull();
  });

  it("lists a host with its address and what it is connected as", async () => {
    await show([host()], true);
    const row = container.querySelector('[data-testid="manage-host-conn-remote"]');
    expect(row).not.toBeNull();
    expect(row?.textContent).toContain("Acme");
    expect(row?.textContent).toContain("https://acme.test");
    expect(row?.textContent).toContain("Connected");
  });

  it("offers no edit or forget on a host this application manages itself", async () => {
    // The roster re-applies the name and address on every refresh, and a
    // forgotten row comes back under a fresh id on the next poll — so buttons
    // here would undo themselves silently.
    await show([host({ id: "conn-local", connector: { kind: "local" } })], true);
    expect(container.querySelector('[data-testid="manage-host-edit-conn-local"]')).toBeNull();
    expect(container.querySelector('[data-testid="manage-host-forget-conn-local"]')).toBeNull();
    expect(
      container.querySelector('[data-testid="manage-host-managed-conn-local"]')?.textContent,
    ).toContain("Runs on this computer");
  });

  it("does not forget a host on the first click", async () => {
    // The id goes with it, and with the id every scoped key underneath. That
    // is a confirmation's worth of consequence.
    await show([host()], true);
    const forget = container.querySelector<HTMLButtonElement>(
      '[data-testid="manage-host-forget-conn-remote"]',
    );
    expect(forget).not.toBeNull();
    await act(async () => forget!.click());
    expect(removed).toEqual([]);
  });
});
