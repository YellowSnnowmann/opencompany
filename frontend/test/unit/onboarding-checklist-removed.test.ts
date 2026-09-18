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
 * A company that has done none of the three things the post-build checklist
 * used to demand — no confirmed name, no integration, no successful workflow
 * run — opens straight into the ordinary console.
 *
 * This is the one assertion that survives the checklist's deletion, and it is
 * deliberately written against the shell rather than a predicate: the screen was
 * removed by deleting a render branch, so only a real mount can show that
 * nothing renders in its place. The admin role and the funnel read are both
 * answered exactly as they were when the screen did mount — an admin, over an
 * unactivated company — because those were the conditions that produced it.
 */

const SCOPE: LocalScope = { connection: "test-connection" as ConnectionId, company: null };

const STATUS: CompanyStatus = {
  id: "co",
  name: "Acme",
  lifecycle: "running",
  pending_approvals: 0,
};

/** Staffed, so first-run setup has nothing to offer and the shell is free to render. */
const STAFFED: TeamMemberDto[] = [
  { id: "operations", role: "Analyst", inboxEnabled: false, global: true } as TeamMemberDto,
  { id: "ada", role: "Operations", inboxEnabled: false } as TeamMemberDto,
];

/** The funnel with every step outstanding — what `GET {scope}/activation` answers. */
const NOTHING_DONE = {
  nameConfirmed: false,
  integrationConnected: false,
  workflowRunSucceeded: false,
  isActivated: false,
};

function hang(): Promise<never> {
  return new Promise<never>(() => {});
}

function buildClient(): OpenCompanyClient {
  const known = {
    baseUrl: "",
    scopeFor: (company: string | null) => `/api/v1/companies/${company ?? ""}`,
    listTeam: vi.fn(async () => STAFFED),
    subscribeToEvents: () => () => {},
    get: (path: string) => {
      if (path.endsWith("/auth/me")) return Promise.resolve({ role: "admin" });
      if (path.endsWith("/activation")) return Promise.resolve(NOTHING_DONE);
      return hang();
    },
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

describe("the post-build checklist is gone", () => {
  it("opens the ordinary console for a company that has completed none of its steps", async () => {
    await act(async () => {
      root.render(
        createElement(HostsProvider, {
          value: HOSTS,
          children: createElement(ConnectionScopeProvider, {
            scope: SCOPE,
            children: createElement(AppShell, {
              client: buildClient(),
              company: null,
              initialStatus: STATUS,
              companies: [STATUS],
              onSwitchCompany: () => {},
            }),
          }),
        }),
      );
    });

    expect(container.querySelector("#main-content")).not.toBeNull();
    const text = container.textContent ?? "";
    expect(text).not.toContain("get your company running");
    expect(text).not.toContain("Skip setup");
    expect(text).not.toContain("Name your company");
    expect(text).not.toContain("Connect an integration");
    expect(text).not.toContain("Run an automation");
  });
});
