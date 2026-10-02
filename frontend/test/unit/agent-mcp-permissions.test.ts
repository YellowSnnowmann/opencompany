// @vitest-environment jsdom

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { OpenCompanyClient } from "@/api/client";
import type { ToolPolicyRow } from "@/api/mcp-tool-policy";
import type {
  AgentMcpPermissions as Picture,
  AgentServerPermissions,
} from "@/api/team-mcp-permissions";

/**
 * What one teammate can actually call.
 *
 * A teammate is granted a **server**, and the server's tool modes are the
 * baseline every teammate reaching it gets. Nothing in the console composed the
 * two, so the Tools tab's glob list was the whole answer — and a glob cannot say
 * what happens when a tool is called. What this pins is mostly what the page
 * must NOT do: hide a server the teammate cannot reach, offer a button for a
 * grant the console may not write, hardcode the approvals notice, or let one
 * damaged document answer for the other five.
 */

const api = vi.hoisted(() => ({ readAgentMcpPermissions: vi.fn() }));

vi.mock("@/api/team-mcp-permissions", async () => {
  const actual = await vi.importActual<
    typeof import("@/api/team-mcp-permissions")
  >("@/api/team-mcp-permissions");
  return { ...actual, ...api };
});

const { AgentMcpPermissions } = await import(
  "@/views/mcp/AgentMcpPermissions"
);

function tool(over: Partial<ToolPolicyRow> & { tool: string }): ToolPolicyRow {
  return {
    effectiveTier: "read_only",
    mode: "always_allow",
    isOverride: false,
    source: "server_inherited",
    differingAgents: [],
    ...over,
  };
}

function block(
  over: Partial<AgentServerPermissions> & { server: string },
): AgentServerPermissions {
  return {
    reached: true,
    enabled: true,
    tools: [],
    discoveredAtMillis: 1,
    fullyRefused: false,
    unreadable: false,
    ...over,
  };
}

function picture(over: Partial<Picture> = {}): Picture {
  return {
    agent: "engineer",
    effectiveGrants: ["mcp:*"],
    servers: [],
    sharedToolNames: [],
    approvalsPark: true,
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

/**
 * Mount fresh.
 *
 * A second `render` into the same root would not re-read — the effect's deps have
 * not moved — so a test comparing two host answers has to start again rather than
 * silently assert against the first one.
 */
async function mount(over: { onOpenServer?: (name: string) => void } = {}) {
  act(() => root.unmount());
  root = createRoot(container);
  await act(async () => {
    root.render(
      createElement(AgentMcpPermissions, {
        client,
        company: "acme",
        agentId: "engineer",
        agentName: "Engineer",
        ...over,
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
});

describe("the headline", () => {
  it("shows its working, because a bare count reads as a broken counter", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({
        servers: [
          block({ server: "notion" }),
          block({ server: "deepwiki", reached: false, grantNeeded: "mcp:deepwiki" }),
        ],
      }),
    );

    await mount();

    expect(el("agent-mcp-headline")?.textContent).toBe(
      "Reaches 1 of 2 MCP servers",
    );
    // Both servers are listed. A page that hides what a teammate cannot reach
    // cannot answer the question it exists for.
    expect(all('[data-testid="agent-mcp-server-block"]')).toHaveLength(2);
  });

  it("distinguishes a standard grant from a deliberately empty one", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({ servers: [block({ server: "notion" })] }),
    );
    await mount();
    expect(container.textContent).toContain(
      "lists no tools of its own, so it holds everything the company allows",
    );

    api.readAgentMcpPermissions.mockResolvedValue(
      picture({
        requested: [],
        effectiveGrants: [],
        servers: [block({ server: "notion", reached: false })],
      }),
    );
    await mount();
    expect(container.textContent).toContain(
      "deliberately empty tool grant, so it holds nothing",
    );
  });
});

describe("a server this teammate does not reach", () => {
  it("names the exact grant it would need", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({
        requested: ["composio", "workspace.read"],
        effectiveGrants: ["composio", "workspace.read"],
        servers: [
          block({ server: "notion", reached: false, grantNeeded: "mcp:notion" }),
        ],
      }),
    );

    await mount();

    expect(el("agent-mcp-not-reached")).not.toBeNull();
    expect(container.textContent).toContain("mcp:notion");
    expect(container.textContent).toContain("Every permission below is inert");
  });
});

describe("the two grants that confer nothing", () => {
  it("says a catch-all does not include MCP", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({
        requested: ["*"],
        effectiveGrants: ["*"],
        servers: [block({ server: "notion", reached: false })],
      }),
    );

    await mount();

    const notice = el("agent-mcp-catch-all");
    expect(notice?.textContent).toContain("does not include MCP");
    expect(notice?.textContent).toContain("mcp:*");
  });

  it("names the file when the company grants no MCP namespace, and offers no button", async () => {
    // `requested` absent means this teammate inherits the company's standard
    // grant, so an absent `mcp:` there is the company's gap and not a narrowing.
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({
        effectiveGrants: ["composio", "workspace.read"],
        servers: [
          block({ server: "notion", reached: false }),
          block({ server: "deepwiki", reached: false }),
        ],
      }),
    );

    await mount();

    const notice = el("agent-mcp-no-namespace");
    expect(notice?.textContent).toContain("grants no MCP namespace at all");
    expect(notice?.textContent).toContain("company.toml");
    expect(notice?.textContent).toContain("read-only boot snapshot");
    // The host's grantable set does not include `mcp`, so there is no button
    // that could work, and one that 403s would be worse.
    expect(all("button")).toHaveLength(0);
  });

  it("does not blame the company when the teammate narrowed it away itself", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({
        requested: ["composio"],
        effectiveGrants: ["composio"],
        servers: [block({ server: "notion", reached: false })],
      }),
    );

    await mount();

    expect(el("agent-mcp-no-namespace")).toBeNull();
  });
});

describe("the approvals notice", () => {
  it("is a function of the host's flag", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({ approvalsPark: false, servers: [block({ server: "notion" })] }),
    );
    await mount();
    expect(el("agent-mcp-approvals-inert")).not.toBeNull();

    api.readAgentMcpPermissions.mockResolvedValue(
      picture({ approvalsPark: true, servers: [block({ server: "notion" })] }),
    );
    await mount();
    expect(el("agent-mcp-approvals-inert")).toBeNull();
  });
});

describe("one server's failure", () => {
  it("degrades that block and nothing else", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({
        servers: [
          block({ server: "broken", unreadable: true }),
          block({
            server: "notion",
            tools: [tool({ tool: "search_pages" })],
          }),
        ],
      }),
    );

    await mount();

    expect(all('[data-testid="agent-mcp-unreadable"]')).toHaveLength(1);
    // The other server still answers.
    expect(container.textContent).toContain("search_pages");
    expect(el("agent-mcp-failed")).toBeNull();
  });

  it("reads a whole-page failure as a failure, not as an empty roster", async () => {
    api.readAgentMcpPermissions.mockRejectedValue(new Error("gateway timeout"));

    await mount();

    expect(el("agent-mcp-failed")?.textContent).toContain("gateway timeout");
    expect(el("agent-mcp-headline")).toBeNull();
  });

  it("reads a host without the route as a fact about the build", async () => {
    api.readAgentMcpPermissions.mockRejectedValue(
      Object.assign(new Error("not found"), { status: 404 }),
    );

    await mount();

    expect(el("agent-mcp-absent")).not.toBeNull();
    expect(el("agent-mcp-failed")).toBeNull();
  });
});

describe("a tool two granted servers both offer", () => {
  it("is flagged as a name match, never as the same capability", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({
        servers: [block({ server: "notion" }), block({ server: "deepwiki" })],
        sharedToolNames: [
          { tool: "search", servers: ["notion", "deepwiki"] },
        ],
      }),
    );

    await mount();

    const card = el("agent-mcp-shared-tools");
    expect(card?.textContent).toContain("search");
    expect(card?.textContent).toContain("Blocking a tool on one server does not");
    expect(card?.textContent).toContain("This flags; it does not block.");
  });
});

describe("a row on this page", () => {
  it("says what happens and where it came from, and refuses the control in place", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({
        servers: [
          block({
            server: "notion",
            tools: [
              tool({
                tool: "archive_database",
                effectiveTier: "write_delete",
                mode: "blocked",
                source: "server_pinned",
              }),
            ],
          }),
        ],
      }),
    );

    await mount();

    expect(el("mcp-permission-effect")?.textContent).toContain(
      "refused — pinned on this server",
    );

    // Modes are edited on the server, so there is one write path per document —
    // but the control stays on screen, refused and reasoned, because a row whose
    // control is missing reads as a row nobody has configured. Same rule the
    // clamp already applies to a mode a teammate's own layer may not loosen.
    const chosen = el("mcp-mode-blocked") as HTMLButtonElement | null;
    expect(chosen).not.toBeNull();
    expect(chosen?.disabled).toBe(true);
    expect(chosen?.getAttribute("aria-checked")).toBe("true");
    expect(chosen?.title).toBe("Set on the notion page.");
    expect((el("mcp-mode-always_allow") as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it("links to the page the mode is set on, labelled and not only as a mark", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({ servers: [block({ server: "notion" })] }),
    );

    const opened: string[] = [];
    await mount({ onOpenServer: (name: string) => opened.push(name) });

    // An icon carries no promise about where it goes. Both affordances name the
    // same server, so the labelled one is a second route to it rather than a
    // second destination.
    expect(el("agent-mcp-open-server")).not.toBeNull();
    const link = el("agent-mcp-edit-on-server");
    expect(link?.textContent).toBe("Edit on the notion page →");
    link?.click();
    expect(opened).toEqual(["notion"]);
  });

  it("offers neither affordance when nothing can be opened", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({ servers: [block({ server: "notion" })] }),
    );

    await mount();

    expect(el("agent-mcp-open-server")).toBeNull();
    expect(el("agent-mcp-edit-on-server")).toBeNull();
  });

  it("is loud when the teammate reaches a server and can call nothing on it", async () => {
    api.readAgentMcpPermissions.mockResolvedValue(
      picture({
        servers: [
          block({
            server: "notion",
            fullyRefused: true,
            tools: [tool({ tool: "search_pages", mode: "blocked" })],
          }),
        ],
      }),
    );

    await mount();

    expect(el("agent-mcp-fully-refused")?.textContent).toContain(
      "Engineer reaches this server but can call nothing on it",
    );
  });
});
