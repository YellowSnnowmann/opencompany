// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { CompanyStatus, TeamMemberDto } from "@/api/types";
import { AppShell } from "@/components/app-shell";
import { ConnectionScopeProvider } from "@/connections/ConnectionContext";
import { HostsProvider, type HostsValue } from "@/connections/HostsContext";
import type { Connection, ConnectionId, LocalScope } from "@/connections/types";

/**
 * `shouldHoldShellPending` (`@/setup/hold-shell`) holds `AppShell` in a neutral
 * pending state — `<RouteLoading>` — for as long as `setupChecked` is `false`,
 * and `SetupController`'s own `onOpenChange` is the only thing that ever sets
 * it (see that state's own doc in `app-shell.tsx`).
 *
 * Two things have to hold at once for that to work, and neither is provable
 * from the predicate alone — both are about which JSX branch `AppShell` mounts
 * `<SetupController>` under:
 *
 * 1. The controller mounts while the shell is held, or its roster read never
 *    runs, `onOpenChange` never fires, and the hold is permanent.
 * 2. It sits at the same position in the held branch and the ordinary one, or
 *    React reconciles it as a different node and unmounts it on the very
 *    transition its own read causes — discarding the proven `unstaffed`/`open`
 *    state and issuing a second `listTeam`.
 */

const SCOPE: LocalScope = { connection: "test-connection" as ConnectionId, company: null };

const STATUS: CompanyStatus = {
  id: "co",
  name: "Acme",
  lifecycle: "running",
  pending_approvals: 0,
};

/** A staffed roster — baseline plus one real teammate (`teamIsUnstaffed`'s own contract). */
const STAFFED: TeamMemberDto[] = [
  { id: "operations", role: "Analyst", inboxEnabled: false, global: true } as TeamMemberDto,
  { id: "ada", role: "Operations", inboxEnabled: false } as TeamMemberDto,
];

/** A promise that never settles — every mount-time read this test is not exercising. */
function hang(): Promise<never> {
  return new Promise<never>(() => {});
}

/**
 * A minimal `OpenCompanyClient` double.
 *
 * `listDesks` is deliberately left hanging: `AppShell`'s own thread-hydration
 * effect only calls `client.listTeam` inside `listDesks(...).then(...)`, so
 * hanging `listDesks` suppresses that unrelated call and leaves the roster reads
 * this test counts as the only ones. `/auth/me` routes through `get`, which
 * hangs, so the admin role stays unresolved throughout — it is not what decides
 * either branch. Anything else this large a component reaches for becomes a
 * permanently-pending no-op via the `Proxy` rather than a hard crash.
 */
function buildClient(listTeam: ReturnType<typeof vi.fn>): OpenCompanyClient {
  const known = {
    baseUrl: "",
    scopeFor: (company: string | null) => `/api/v1/companies/${company ?? ""}`,
    listTeam,
    subscribeToEvents: () => () => {},
    get: hang,
    status: hang,
    approvals: hang,
    listDesks: hang,
  };
  return new Proxy(known, {
    get(target, prop, receiver) {
      if (prop in target) return Reflect.get(target, prop, receiver);
      return hang;
    },
  }) as unknown as OpenCompanyClient;
}

/**
 * The one host this console is connected to. Reaching the ordinary shell draws
 * the sidebar's `HostSwitcher`, and `useHosts` throws outside a provider by
 * design (see its own doc).
 */
const CONNECTION: Connection = {
  id: SCOPE.connection,
  defaultCompany: null,
  label: "test",
  baseUrl: "",
  credential: { kind: "cookie" },
  status: "live",
  identity: null,
  companies: [],
  connector: { kind: "remote" },
};

const HOSTS: HostsValue = {
  connections: [CONNECTION],
  selected: SCOPE.connection,
  onSelect: () => {},
  onAdd: () => {},
  localInstances: [],
  onEditHost: () => {},
  onRemoveHost: () => {},
  hub: false,
};

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  // jsdom implements no media queries at all. `useIsMobile` calls
  // `window.matchMedia` on mount, and `SidebarProvider` — which only the
  // ordinary shell renders — uses it. Always desktop: the breakpoint is not
  // what any test here is about.
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
  window.location.hash = "#/overview";
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

/** Mounts the shell with the roster read deferred, so it starts held. */
async function mountHeld(): Promise<{
  listTeam: ReturnType<typeof vi.fn>;
  landRoster: (roster: TeamMemberDto[]) => void;
}> {
  let landRoster!: (roster: TeamMemberDto[]) => void;
  const roster = new Promise<TeamMemberDto[]>((resolve) => {
    landRoster = resolve;
  });
  const listTeam = vi.fn(() => roster);
  const client = buildClient(listTeam);

  await act(async () => {
    root.render(
      createElement(HostsProvider, {
        value: HOSTS,
        children: createElement(ConnectionScopeProvider, {
          scope: SCOPE,
          children: createElement(AppShell, {
            client,
            company: null,
            initialStatus: STATUS,
            companies: [STATUS],
            onSwitchCompany: () => {},
          }),
        }),
      }),
    );
  });

  return { listTeam, landRoster };
}

describe("AppShell holds the console until the setup roster read lands", () => {
  it("renders the loader, and mounts SetupController under it so the read can run", async () => {
    const { listTeam, landRoster } = await mountHeld();

    expect(container.textContent).toContain("Loading");
    expect(container.querySelector("#main-content")).toBeNull();
    // The bug this guards: nested in JSX only the resolved shell reached,
    // `SetupController` never mounted and this read never went out — so the
    // hold waiting on it could never end.
    expect(listTeam).toHaveBeenCalled();

    await act(async () => landRoster(STAFFED));

    expect(container.querySelector("#main-content")).not.toBeNull();
  });
});

describe("AppShell keeps SetupController mounted across its branch transition", () => {
  it("does not re-read the roster when the shell leaves the hold", async () => {
    const { listTeam, landRoster } = await mountHeld();

    const readsWhilePending = listTeam.mock.calls.length;
    expect(readsWhilePending).toBeGreaterThan(0);

    await act(async () => landRoster(STAFFED));

    // The hand-off actually happened (this assertion is what keeps the one
    // below from passing vacuously on a shell that never left the hold).
    expect(container.querySelector("#main-content")).not.toBeNull();

    // Same position in both branches, so the controller was never torn down and
    // never re-read the roster.
    //
    // The ordinary branch paints `#/overview`, which is the company graph, and
    // the graph's snapshot takes one roster read of its own on mount. That read
    // belongs to the graph, not to a re-mounted controller, so it is accounted
    // for exactly rather than folded into a `>=` that would let a genuine
    // re-read through unnoticed.
    const GRAPH_ROSTER_READS = 1;
    // `RoomView` is mounted on every route since #2130, not only on `#/chat`:
    // the sidebar's channel rail is portalled out of it and is pinned there on
    // every section, so the view that feeds it has to outlive the route that
    // used to own it. It takes one roster read of its own on mount, for the
    // rail's direct-message rows. Accounted for by name for the same reason the
    // graph's is.
    const CHAT_RAIL_ROSTER_READS = 1;
    expect(listTeam.mock.calls.length).toBe(
      readsWhilePending + GRAPH_ROSTER_READS + CHAT_RAIL_ROSTER_READS,
    );
  });
});
