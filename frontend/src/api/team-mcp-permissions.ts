// What one teammate can actually call on this company's MCP servers, over
// `GET {scope}/team/{agentId}/mcp/permissions`.
//
// Declared servers only — a directory install's per-teammate rules are read
// through the registry route with `?agent=`.

import type { OpenCompanyClient } from "./client";
import type { ApprovalMode, ToolPolicyRow } from "./mcp-tool-policy";

/** One configured server, as it stands for one teammate. */
export interface AgentServerPermissions {
  server: string;
  /** Whether this teammate's grants reach the server at all. */
  reached: boolean;
  /** The grant that would make it reachable, when it is not. */
  grantNeeded?: string;
  enabled: boolean;
  /** Every tool a decision can be stated about. Empty when the document is unreadable. */
  tools: ToolPolicyRow[];
  /** When discovery last succeeded. `0` reads as never. */
  discoveredAtMillis: number;
  /**
   * Whether every tool this server is known to offer is refused this teammate.
   * Never true for a server no probe has reached.
   */
  fullyRefused: boolean;
  /**
   * Whether this server's stored document could not be read. Degrades this
   * block only.
   */
  unreadable: boolean;
}

/** A tool name more than one reached server offers. */
export interface SharedToolName {
  tool: string;
  servers: string[];
}

/** One teammate's whole MCP picture. */
export interface AgentMcpPermissions {
  agent: string;
  /**
   * The grant this teammate asks for: absent inherits the company's standard
   * grant, `[]` is a no-tools grant, and a list narrows.
   */
  requested?: string[];
  /** The grants this teammate is actually built with. */
  effectiveGrants: string[];
  servers: AgentServerPermissions[];
  /**
   * Tool names reachable on more than one server this teammate holds. Matching
   * is on the name, not the capability.
   */
  sharedToolNames: SharedToolName[];
  /**
   * Whether approval parking is live on this host. `false` means a
   * `needs_approval` mode behaves as allow.
   */
  approvalsPark: boolean;
}

/** What a tool does, once the mode has been resolved for a teammate. */
export const EFFECT_WORDS: Record<ApprovalMode, string> = {
  always_allow: "runs",
  needs_approval: "asks",
  blocked: "refused",
};

/** Read one teammate's whole MCP picture. */
export function readAgentMcpPermissions(
  client: OpenCompanyClient,
  company: string | null,
  agentId: string,
): Promise<AgentMcpPermissions> {
  return client.get<AgentMcpPermissions>(
    `${client.scopeFor(company)}/team/${encodeURIComponent(agentId)}/mcp/permissions`,
  );
}
