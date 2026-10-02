// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { McpServer, McpSource } from "@/api/types";
import type {
  ApprovalMode,
  PolicySource,
  ToolPolicyDocument,
  ToolPolicyRow,
} from "@/api/mcp-tool-policy";

/**
 * The per-teammate lens on a server's tool permissions.
 *
 * Two properties carry the weight. The first is that the **Everyone** lens is
 * unchanged — that is the back-compatibility promise made visible, and the only
 * thing added to it is a count of the teammates whose resolved mode differs,
 * without which the page is true about the document and silent about its
 * exceptions. The second is that the agent lens tells the truth about a
 * narrow-only layer: which rule won, that a stored setting was *discarded* rather
 * than honoured, and that the options the host would refuse are shown disabled
 * with the reason rather than quietly removed.
 */

const api = vi.hoisted(() => ({
  readToolPolicy: vi.fn(),
  writeToolPolicy: vi.fn(),
  resetToolPolicy: vi.fn(),
}));

vi.mock("@/api/mcp-tool-policy", async () => {
  const actual = await vi.importActual<typeof import("@/api/mcp-tool-policy")>(
    "@/api/mcp-tool-policy",
  );
  return { ...actual, ...api };
});

const { McpToolPermissions } = await import("@/views/mcp/McpToolPermissions");

const AGENTS = [
  { id: "engineer", name: "Engineer" },
  { id: "writer", name: "Writer" },
];

function server(over: Partial<McpServer> & { source: McpSource }): McpServer {
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

function tool(
  over: Partial<ToolPolicyRow> & { tool: string },
): ToolPolicyRow {
  return {
    effectiveTier: "read_only",
    mode: "always_allow",
    isOverride: false,
    source: "server_inherited",
    differingAgents: [],
    ...over,
  };
}

function doc(over: Partial<ToolPolicyDocument> = {}): ToolPolicyDocument {
  return {
    server: "notion",
    tierDefaults: {
      read_only: { mode: "always_allow", stored: false },
      interactive: { mode: "needs_approval", stored: false },
      write_delete: { mode: "needs_approval", stored: false },
    },
    tools: [],
    discoveredAtMillis: 1,
    ...over,
  };
}

const client = {} as unknown as OpenCompanyClient;

let container: HTMLDivElement;
let root: Root;

function el(testId: string): HTMLElement | null {
  return container.querySelector(`[data-testid="${testId}"]`);
}

function all(selector: string): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>(selector)];
}

/** Mount with the lens the address names, which is where the lens lives. */
async function mount(showing: string | null, canManage = true) {
  window.history.replaceState(
    null,
    "",
    showing === null
      ? "#/connections/mcp?server=notion"
      : `#/connections/mcp?server=notion&showing=${showing}`,
  );
  await act(async () => {
    root.render(
      createElement(McpToolPermissions, {
        client,
        company: "acme",
        server: server({ source: "runtime" }),
        canManage,
        agents: AGENTS,
      }),
    );
  });
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
  window.history.replaceState(null, "", "#/");
});

describe("the Everyone lens", () => {
  it("reads the company document, and names nobody", async () => {
    api.readToolPolicy.mockResolvedValue(
      doc({ tools: [tool({ tool: "get_page" })] }),
    );

    await mount(null);

    // A blank scope is the company document, and it is the only read: the agent
    // baseline is not fetched when there is no agent to measure against.
    expect(api.readToolPolicy).toHaveBeenCalledTimes(1);
    expect(api.readToolPolicy).toHaveBeenCalledWith(
      client,
      "acme",
      { kind: "declared", name: "notion" },
      null,
    );
    // No source chip: the company view is byte-identical to what it always was.
    expect(el("mcp-permission-source")).toBeNull();
    expect(el("mcp-permissions-summary")).toBeNull();
  });

  it("says how many teammates a row's mode is not true for", async () => {
    api.readToolPolicy.mockResolvedValue(
      doc({
        tools: [
          tool({ tool: "get_page", differingAgents: ["writer", "engineer"] }),
          tool({ tool: "list_users" }),
        ],
      }),
    );

    await mount(null);

    const counts = all('[data-testid="mcp-permission-differing"]');
    expect(counts).toHaveLength(1);
    expect(counts[0]?.textContent).toBe("2 teammates differ");
  });

  it("counts one exception in the singular", async () => {
    api.readToolPolicy.mockResolvedValue(
      doc({ tools: [tool({ tool: "get_page", differingAgents: ["writer"] })] }),
    );

    await mount(null);

    expect(el("mcp-permission-differing")?.textContent).toBe(
      "1 teammate differs",
    );
  });
});

describe("a teammate's lens", () => {
  /** The company's baseline, then the same server as it stands for Engineer. */
  function scoped(rows: ToolPolicyRow[], baseline: ToolPolicyRow[]) {
    api.readToolPolicy.mockImplementation(
      (
        _client: unknown,
        _company: unknown,
        _target: unknown,
        agent: string | null,
      ) =>
        Promise.resolve(
          agent === null
            ? doc({ tools: baseline })
            : doc({ agent: "engineer", tools: rows }),
        ),
    );
  }

  it("reads both scopes, because the server's mode is the floor a rule may not loosen", async () => {
    scoped(
      [tool({ tool: "get_page", mode: "blocked", source: "agent_pinned", agentMode: "blocked" })],
      [tool({ tool: "get_page", mode: "always_allow" })],
    );

    await mount("engineer");

    expect(api.readToolPolicy).toHaveBeenCalledWith(
      client,
      "acme",
      { kind: "declared", name: "notion" },
      "engineer",
    );
    expect(api.readToolPolicy).toHaveBeenCalledWith(
      client,
      "acme",
      { kind: "declared", name: "notion" },
      null,
    );
  });

  it("names which rule won", async () => {
    scoped(
      [tool({ tool: "get_page", mode: "blocked", source: "agent_pinned", agentMode: "blocked" })],
      [tool({ tool: "get_page", mode: "always_allow" })],
    );

    await mount("engineer");

    expect(el("mcp-permission-source")?.textContent).toBe("set for Engineer");
  });

  it("says a stored setting was discarded rather than rendering it as live", async () => {
    scoped(
      [
        tool({
          tool: "get_page",
          mode: "blocked",
          source: "agent_clamped",
          agentMode: "always_allow",
        }),
      ],
      [tool({ tool: "get_page", mode: "blocked" })],
    );

    await mount("engineer");

    const chip = el("mcp-permission-source");
    expect(chip?.textContent).toContain("Engineer's Allow was discarded");
    expect(chip?.textContent).toContain("less restrictive");
  });

  it("disables the options the host would refuse, and says why on each", async () => {
    scoped(
      [tool({ tool: "get_page", mode: "needs_approval", source: "server_pinned" })],
      [tool({ tool: "get_page", mode: "needs_approval" })],
    );

    await mount("engineer");

    // The server already asks, so a teammate's own rule may only ask or refuse.
    const allow = el("mcp-mode-always_allow");
    expect(allow?.hasAttribute("disabled")).toBe(true);
    expect(allow?.getAttribute("title")).toContain(
      "A teammate's own rule can only be stricter",
    );
    // Not hidden: an absent control cannot explain itself.
    expect(allow).not.toBeNull();
    expect(el("mcp-mode-blocked")?.hasAttribute("disabled")).toBe(false);
  });

  it("writes and clears in the teammate's scope only", async () => {
    scoped(
      [
        tool({
          tool: "get_page",
          mode: "blocked",
          source: "agent_pinned",
          agentMode: "blocked",
        }),
      ],
      [tool({ tool: "get_page", mode: "always_allow" })],
    );
    api.writeToolPolicy.mockResolvedValue(doc({ agent: "engineer" }));

    await mount("engineer");

    await act(async () => {
      el("mcp-permission-clear-row")?.dispatchEvent(
        new MouseEvent("click", { bubbles: true }),
      );
    });

    expect(api.writeToolPolicy).toHaveBeenCalledWith(
      client,
      "acme",
      { kind: "declared", name: "notion" },
      { tools: [{ tool: "get_page" }] },
      "engineer",
    );
  });

  it("offers the clear on a row whose stored setting was discarded", async () => {
    // Clearing a discarded setting is what stops it reappearing the next time the
    // server's own mode moves.
    scoped(
      [
        tool({
          tool: "get_page",
          mode: "blocked",
          source: "agent_clamped",
          agentMode: "always_allow",
          isOverride: false,
        }),
      ],
      [tool({ tool: "get_page", mode: "blocked" })],
    );

    await mount("engineer");

    expect(el("mcp-permission-clear-row")).not.toBeNull();
  });

  it("says what the narrowing costs", async () => {
    scoped(
      [
        tool({ tool: "get_page", mode: "always_allow" }),
        tool({ tool: "list_users", mode: "needs_approval" }),
        tool({ tool: "delete_page", mode: "blocked" }),
      ],
      [tool({ tool: "get_page" }), tool({ tool: "list_users" }), tool({ tool: "delete_page" })],
    );

    await mount("engineer");

    expect(el("mcp-permissions-summary")?.textContent).toContain(
      "can call 2 of 3 tools — 1 refused, 1 asks",
    );
  });

  it("is loud when the teammate reaches the server and can call nothing", async () => {
    scoped(
      [
        tool({ tool: "get_page", mode: "blocked" }),
        tool({ tool: "list_users", mode: "blocked" }),
      ],
      [tool({ tool: "get_page" }), tool({ tool: "list_users" })],
    );

    await mount("engineer");

    expect(el("mcp-permissions-fully-refused")?.textContent).toContain(
      "Engineer reaches this server but can call nothing on it",
    );
  });

  it("withholds the tier default, which is never per-teammate", async () => {
    scoped([tool({ tool: "get_page" })], [tool({ tool: "get_page" })]);

    await mount("engineer");

    const bulk = container.querySelector<HTMLElement>(
      '[aria-label="Default for read-only tools"]',
    );
    expect(bulk).toBeNull();
    expect(el("mcp-tier-company-only-read_only")?.textContent).toContain(
      "for everyone",
    );
  });

  it("offers a reset for the whole of one teammate's layer", async () => {
    scoped([tool({ tool: "get_page" })], [tool({ tool: "get_page" })]);
    api.resetToolPolicy.mockResolvedValue(doc({ agent: "engineer" }));

    await mount("engineer");

    const clear = el("mcp-permissions-clear-agent");
    expect(clear?.textContent).toContain("Clear every rule set for Engineer");
    await act(async () => {
      clear?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(api.resetToolPolicy).toHaveBeenCalledWith(
      client,
      "acme",
      { kind: "declared", name: "notion" },
      "engineer",
    );
  });
});

describe("a damaged document, read for one teammate", () => {
  it("says only the company-wide reset repairs it", async () => {
    const { ApiError } = await import("@/api/types");
    api.readToolPolicy.mockRejectedValue(
      new ApiError(409, "policy_unreadable", "cannot be read.", true),
    );

    await mount("engineer");

    expect(el("mcp-permissions-unreadable")?.textContent).toContain(
      "Only the company-wide reset repairs it",
    );
    // And the repair it offers is the company one, not the teammate's.
    api.resetToolPolicy.mockResolvedValue(doc());
    await act(async () => {
      el("mcp-permissions-clear")?.dispatchEvent(
        new MouseEvent("click", { bubbles: true }),
      );
    });
    expect(api.resetToolPolicy).toHaveBeenCalledWith(
      client,
      "acme",
      { kind: "declared", name: "notion" },
      null,
    );
  });
});

describe("a member reading a teammate's lens", () => {
  it("can read every decision and change none", async () => {
    api.readToolPolicy.mockResolvedValue(
      doc({ tools: [tool({ tool: "get_page" })] }),
    );

    await mount("engineer", false);

    expect(el("mcp-permissions-read-only")).not.toBeNull();
    expect(all('[data-testid="mcp-permission-row"]')).toHaveLength(1);
    expect(el("mcp-mode-blocked")?.hasAttribute("disabled")).toBe(true);
    expect(el("mcp-permissions-clear-agent")).toBeNull();
  });
});

describe("a teammate lens", () => {
  beforeEach(() => {
    api.readToolPolicy.mockResolvedValue(
      doc({ tools: [tool({ tool: "get_page" })] }),
    );
  });

  it("claims no scope while the company document is showing", async () => {
    await mount(null);

    expect(el("mcp-permissions-scope-notice")).toBeNull();
    // A tier belongs to the tool, and on the company view it is the operator's.
    expect(
      (el("mcp-permissions-tier-default-read_only") as HTMLButtonElement)
        .disabled,
    ).toBe(false);
  });

  it("names whose permissions a click changes, and says the default is untouched", async () => {
    await mount("engineer");

    const notice = el("mcp-permissions-scope-notice");
    expect(notice?.textContent).toContain("Engineer");
    expect(notice?.textContent).toContain(
      "leaves the company default as it is",
    );
  });

  it("states the tier default's value rather than offering a dead control", async () => {
    await mount("engineer");

    expect(el("mcp-permissions-tier-default-read_only")).toBeNull();
    expect(el("mcp-tier-company-only-read_only")?.textContent).toContain(
      "for everyone",
    );
  });
});

describe("the approvals notice", () => {
  const modes: ApprovalMode[] = ["always_allow"];
  const sources: PolicySource[] = ["server_inherited"];

  it("is a function of the host's flag, not a constant", async () => {
    api.readToolPolicy.mockResolvedValue(
      doc({
        tools: [tool({ tool: "get_page", mode: modes[0], source: sources[0] })],
      }),
    );

    await mount(null);
    // Nothing on this surface answers the flag, so nothing is claimed.
    expect(el("mcp-approvals-inert")).toBeNull();

    await act(async () => {
      root.render(
        createElement(McpToolPermissions, {
          client,
          company: "acme",
          server: server({ source: "runtime" }),
          canManage: true,
          agents: AGENTS,
          approvalsPark: false,
        }),
      );
    });
    expect(el("mcp-approvals-inert")).not.toBeNull();

    await act(async () => {
      root.render(
        createElement(McpToolPermissions, {
          client,
          company: "acme",
          server: server({ source: "runtime" }),
          canManage: true,
          agents: AGENTS,
          approvalsPark: true,
        }),
      );
    });
    expect(el("mcp-approvals-inert")).toBeNull();
  });
});

describe("a fast lens switch", () => {
  it("never paints the scope the operator has left", async () => {
    // A lens switch moves this panel from one document to another exactly as a
    // server switch does, so an answer for the previous teammate arriving last
    // would paint the wrong teammate's permissions under the right teammate's
    // name.
    const pending: Record<string, (d: ToolPolicyDocument) => void> = {};
    api.readToolPolicy.mockImplementation(
      (
        _client: unknown,
        _company: unknown,
        _target: unknown,
        agent: string | null,
      ) =>
        agent === null
          ? Promise.resolve(doc({ tools: [tool({ tool: "get_page" })] }))
          : new Promise<ToolPolicyDocument>((resolve) => {
              pending[agent] = resolve;
            }),
    );

    await mount("engineer");

    await act(async () => {
      window.location.hash = "#/connections/mcp?server=notion&showing=writer";
      // jsdom delivers `hashchange` as a task, so the read for the new scope has
      // not started until this settles.
      await new Promise((r) => setTimeout(r, 0));
    });

    // Writer answers, then Engineer's older read lands.
    await act(async () => {
      pending.writer?.(
        doc({ agent: "writer", tools: [tool({ tool: "writer_only" })] }),
      );
      await new Promise((r) => setTimeout(r, 0));
    });
    await act(async () => {
      pending.engineer?.(
        doc({ agent: "engineer", tools: [tool({ tool: "engineer_only" })] }),
      );
      await new Promise((r) => setTimeout(r, 0));
    });

    expect(container.textContent).toContain("writer_only");
    expect(container.textContent).not.toContain("engineer_only");
  });
});
