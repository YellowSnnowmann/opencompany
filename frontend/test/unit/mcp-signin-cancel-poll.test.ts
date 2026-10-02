// @vitest-environment jsdom

// Cancelling a browser sign-in must stop the poll it started, not just hide
// the flight panel. `poll` clears its own `pollTimers` entry before it awaits
// `testMcpServer`, so a cancel that runs during that await finds nothing to
// clear and only removes the `signIns` row. Without a marker the poll's own
// code checks after the await, the cancelled check still lands: an eventual
// `ok` shows a "Signed in" toast for a sign-in the operator gave up on, and a
// non-`ok` result re-arms a fresh timer that blocks the next "Sign in" click
// for up to two minutes.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { McpHealth, McpServer, McpSource } from "@/api/types";

const api = vi.hoisted(() => ({
  listMcpServers: vi.fn(),
  testMcpServer: vi.fn(),
  discoverMcpTools: vi.fn(),
  addMcpServer: vi.fn(),
  removeMcpServer: vi.fn(),
  updateMcpServer: vi.fn(),
  startMcpOAuth: vi.fn(),
}));

const registryApi = vi.hoisted(() => ({
  connectMcpRegistryServer: vi.fn(),
  disconnectMcpRegistryServer: vi.fn(),
  getMcpRegistryEntry: vi.fn(),
  installMcpRegistryEntry: vi.fn(),
  searchMcpRegistry: vi.fn(),
  uninstallMcpRegistryServer: vi.fn(),
  updateMcpRegistryEnv: vi.fn(),
}));

const toasts = vi.hoisted(() => ({
  base: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
  warning: vi.fn(),
  info: vi.fn(),
}));

vi.mock("@/api/mcp", () => api);
vi.mock("@/api/mcp-registry", () => registryApi);
vi.mock("sonner", () => ({
  toast: Object.assign(toasts.base, {
    success: toasts.success,
    error: toasts.error,
    message: toasts.message,
    warning: toasts.warning,
    info: toasts.info,
  }),
}));
vi.mock("@/views/connections/McpRegistryBrowser", () => ({
  useMcpDirectorySearch: () => ({ kind: "idle" }),
}));

const { McpServersSection } = await import(
  "@/views/connections/McpServersSection"
);

function row(over: Partial<McpServer> & { source: McpSource }): McpServer {
  return {
    name: "notion",
    endpoint: "https://mcp.notion.com/mcp",
    enabled: true,
    allowedTools: [],
    disallowedTools: [],
    readOnlyTools: [],
    timeoutSecs: 30,
    authConfigured: false,
    ...over,
  };
}

const NEEDS_SIGN_IN: McpHealth = {
  status: "needs_config",
  authHint: "oauth_required",
  message: "needs a browser sign-in",
  toolCount: 0,
  checkedAtMillis: 1,
};

const client = {
  capabilityStatus: () => Promise.resolve({ mcpInBuild: true }),
} as unknown as OpenCompanyClient;

let container: HTMLDivElement;
let root: Root;

async function mount(servers: McpServer[]) {
  api.listMcpServers.mockResolvedValue(servers);
  await act(async () => {
    root.render(
      createElement(McpServersSection, {
        client,
        company: "acme",
        canManage: true,
        chrome: "standalone" as const,
      }),
    );
  });
}

function testId(id: string) {
  return document.body.querySelector(`[data-testid="${id}"]`);
}

async function click(node: Element | null | undefined) {
  if (!node) throw new Error("nothing to click");
  await act(async () => {
    node.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

/** A `testMcpServer` call whose resolution the test drives by hand. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.clearAllMocks();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.useRealTimers();
});

describe("cancelling a sign-in poll while a check is in flight", () => {
  it("does not report success for a check the operator already gave up on", async () => {
    vi.useFakeTimers();
    api.startMcpOAuth.mockResolvedValue({
      authorizeUrl: "https://auth.example.com/authorize",
    });
    const inFlight = deferred<McpHealth>();
    api.testMcpServer.mockReturnValueOnce(inFlight.promise);

    await mount([{ ...row({ source: "runtime" }), health: NEEDS_SIGN_IN }]);

    await click(testId("mcp-sign-in"));
    expect(api.startMcpOAuth).toHaveBeenCalledTimes(1);
    expect(testId("mcp-signin-flight")).not.toBeNull();

    // Fires the first poll tick; it is now awaiting `testMcpServer`.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });

    // The operator gives up before that check answers.
    await click(testId("mcp-signin-cancel"));
    expect(testId("mcp-signin-flight")).toBeNull();

    // The check the operator already walked away from comes back healthy.
    await act(async () => {
      inFlight.resolve({ ...NEEDS_SIGN_IN, status: "ok", message: "" });
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(toasts.success).not.toHaveBeenCalled();
    expect(testId("mcp-signin-flight")).toBeNull();
  });

  it("does not leave a poll armed that blocks the next Sign in click", async () => {
    vi.useFakeTimers();
    api.startMcpOAuth.mockResolvedValue({
      authorizeUrl: "https://auth.example.com/authorize",
    });
    const inFlight = deferred<McpHealth>();
    api.testMcpServer.mockReturnValueOnce(inFlight.promise);

    await mount([{ ...row({ source: "runtime" }), health: NEEDS_SIGN_IN }]);

    await click(testId("mcp-sign-in"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });

    await click(testId("mcp-signin-cancel"));

    // Still not signed in — the poll's own re-arm, past the cancel, is the
    // thing under test.
    await act(async () => {
      inFlight.resolve({ ...NEEDS_SIGN_IN, status: "needs_config" });
      await Promise.resolve();
      await Promise.resolve();
    });

    // A fresh click must reach the host again, rather than being swallowed by
    // a `pollTimers` entry the cancelled poll re-armed behind the operator's
    // back.
    await click(testId("mcp-sign-in"));
    expect(api.startMcpOAuth).toHaveBeenCalledTimes(2);
  });
});
