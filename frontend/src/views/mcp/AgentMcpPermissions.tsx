import { useEffect, useState } from "react";
import { AlertTriangle, Info, Loader2 } from "lucide-react";

import type { OpenCompanyClient } from "@/api/client";
import {
  readAgentMcpPermissions,
  type AgentMcpPermissions as Picture,
  type AgentServerPermissions,
} from "@/api/team-mcp-permissions";
import type { McpServer } from "@/api/types";
import { TeammateAvatar } from "@/components/teammate-avatar";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { avatarFor } from "@/lib/team";
import { McpServerIcon } from "@/views/connections/McpServerTable";
import {
  SECTION_ORDER,
  TierSection,
  type PolicyLens,
} from "@/views/mcp/tool-policy-rows";

/**
 * What one teammate can actually call.
 *
 * A teammate is granted a **server**: `mcp:<name>` names a server and there is
 * no per-tool grant form. The server's tool modes are the baseline every
 * teammate reaching it gets, and a per-teammate layer may then narrow any tool
 * further.
 *
 * Every configured server is listed, reached or not.
 */

type State =
  | { kind: "loading" }
  | { kind: "ready"; picture: Picture }
  | { kind: "absent" }
  | { kind: "failed"; message: string };

/** Whether any grant this teammate holds reaches an MCP server. */
function holdsMcp(grants: string[]): boolean {
  return grants.some((g) => g === "mcp:*" || g.startsWith("mcp:"));
}

export function AgentMcpPermissions({
  client,
  company,
  agentId,
  agentName,
  /** So a server block can link back to the row it came from. */
  servers = [],
  onOpenServer,
}: {
  client: OpenCompanyClient;
  company: string | null;
  agentId: string;
  agentName: string;
  servers?: McpServer[];
  /**
   * Opens that server's own page with this teammate's lens already applied, so
   * one click lands on the rows being read here rather than on the company
   * document they are resolved against.
   */
  onOpenServer?: (name: string) => void;
}) {
  const [state, setState] = useState<State>({ kind: "loading" });

  useEffect(() => {
    let live = true;
    setState({ kind: "loading" });
    void (async () => {
      try {
        const picture = await readAgentMcpPermissions(client, company, agentId);
        if (live) setState({ kind: "ready", picture });
      } catch (err) {
        if (!live) return;
        // A host with no such route answers 404: a fact about the build, not a
        // failure.
        const status = (err as { status?: number }).status;
        setState(
          status === 404
            ? { kind: "absent" }
            : {
                kind: "failed",
                message:
                  err instanceof Error ? err.message : "Couldn't read this.",
              },
        );
      }
    })();
    return () => {
      live = false;
    };
  }, [client, company, agentId]);

  if (state.kind === "loading") {
    return (
      <p className="flex items-center gap-1 text-xs text-muted-foreground">
        <Loader2 className="size-3 animate-spin" /> Reading what {agentName} can
        call…
      </p>
    );
  }

  if (state.kind === "absent") {
    return (
      <p
        className="flex items-start gap-2 rounded-md bg-muted/40 p-2 text-xs text-muted-foreground"
        data-testid="agent-mcp-absent"
      >
        <Info className="mt-px size-3 shrink-0" />
        <span>
          This host serves no per-teammate MCP permissions read, so what{" "}
          {agentName} can call cannot be resolved here. Each server&apos;s own
          page still holds its tool permissions.
        </span>
      </p>
    );
  }

  if (state.kind === "failed") {
    return (
      <p className="text-xs text-destructive" data-testid="agent-mcp-failed">
        {state.message}
      </p>
    );
  }

  const { picture } = state;
  const reached = picture.servers.filter((s) => s.reached).length;
  const total = picture.servers.length;
  const hasMcp = holdsMcp(picture.effectiveGrants);
  // `requested` absent means this teammate inherits the company's standard
  // grant, so an absent `mcp:` there is the company's gap and not a narrowing.
  const companyGrantsNoMcp = !hasMcp && picture.requested === undefined;
  const catchAllOnly =
    !hasMcp && picture.effectiveGrants.includes("*") && !companyGrantsNoMcp;

  return (
    <div className="space-y-4" data-testid="agent-mcp-permissions">
      <Card>
        <CardContent className="space-y-2">
          <div className="flex items-center gap-2">
            <TeammateAvatar
              name={agentName}
              avatar={avatarFor(agentId)}
              className="size-8"
            />
            <p className="text-sm font-medium" data-testid="agent-mcp-headline">
              Reaches {reached} of {total} MCP server{total === 1 ? "" : "s"}
            </p>
          </div>
          <p className="text-xs text-muted-foreground">
            {picture.requested === undefined
              ? "This teammate lists no tools of its own, so it holds everything the company allows."
              : picture.requested.length === 0
                ? "This teammate holds a deliberately empty tool grant, so it holds nothing — which is not the same as listing none."
                : `Narrowed to ${picture.requested.join(", ")}.`}
          </p>
          <p className="text-xs text-muted-foreground">
            A teammate is granted a <em>server</em>; its tool modes can then be
            narrowed for that teammate alone. A per-teammate rule may only make a
            tool stricter — never looser — and a setting that would widen is shown
            as overridden rather than silently discarded. Modes are edited on each
            server; reach is edited on the Tools tab.
          </p>
        </CardContent>
      </Card>

      {companyGrantsNoMcp && (
        <p
          className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-2 py-1 text-xs text-destructive"
          data-testid="agent-mcp-no-namespace"
        >
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
          <span>
            <strong className="font-medium">
              This company grants no MCP namespace at all
            </strong>
            , so no teammate reaches any server however its own tools are set.{" "}
            <code className="font-mono">mcp:</code> is not in the set the console
            may widen, so this is added in{" "}
            <code className="font-mono">company.toml</code> — and on a hosted
            tenant that file is a read-only boot snapshot. Every permission set on
            the servers below is currently inert.
          </span>
        </p>
      )}

      {catchAllOnly && (
        <p
          className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-2 py-1 text-xs text-destructive"
          data-testid="agent-mcp-catch-all"
        >
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
          <span>
            <strong className="font-medium">
              <code className="font-mono">*</code> does not include MCP.
            </strong>{" "}
            A catch-all is deliberately excluded when MCP servers are matched, so
            this teammate reaches none of them.{" "}
            <code className="font-mono">mcp:*</code> is the grant that reaches
            every server; <code className="font-mono">mcp:&lt;name&gt;</code>{" "}
            reaches one. Both are set on the Tools tab, so there is one write path
            for a grant.
          </span>
        </p>
      )}

      {/* A function of the host's own flag, never a constant: when approvals park
          again this disappears on its own. */}
      {!picture.approvalsPark && (
        <p
          className="flex items-start gap-2 rounded-md border border-status-blocked-text/30 bg-status-blocked-text/10 px-2 py-1 text-xs text-status-blocked-text"
          data-testid="agent-mcp-approvals-inert"
        >
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
          <span>
            <strong className="font-medium">
              &ldquo;Needs approval&rdquo; does not stop anything on this build.
            </strong>{" "}
            Policy-generated approvals are off, so a tool that asks runs exactly
            as if it were allowed. Only Allow and Block differ today.
          </span>
        </p>
      )}

      {picture.sharedToolNames.length > 0 && (
        <Card data-testid="agent-mcp-shared-tools">
          <CardContent className="space-y-2">
            <div className="flex flex-wrap items-center gap-2">
              <p className="text-sm font-medium">Reachable more than one way</p>
              <Badge variant="outline">
                {picture.sharedToolNames.length} tool
                {picture.sharedToolNames.length === 1 ? "" : "s"}
              </Badge>
            </div>
            <p className="text-xs text-muted-foreground">
              Blocking a tool on one server does not stop this teammate if another
              granted server offers the same name. Matching is on the tool&apos;s{" "}
              <strong className="font-medium">name</strong>, not on what it does —
              two servers&apos; <code className="font-mono">search</code> may be
              unrelated; two servers&apos;{" "}
              <code className="font-mono">delete_page</code> probably are not.
              This flags; it does not block.
            </p>
            <ul className="space-y-1">
              {picture.sharedToolNames.map((shared) => (
                <li key={shared.tool} className="text-xs">
                  <span className="font-mono">{shared.tool}</span>
                  <span className="text-muted-foreground">
                    {" "}
                    — on {shared.servers.join(", ")}
                  </span>
                </li>
              ))}
            </ul>
          </CardContent>
        </Card>
      )}

      {total === 0 ? (
        <p
          className="text-xs text-muted-foreground"
          data-testid="agent-mcp-empty"
        >
          This company has no MCP servers configured, so there is nothing for any
          teammate to reach.
        </p>
      ) : (
        picture.servers.map((block) => (
          <ServerBlock
            key={block.server}
            block={block}
            agentId={agentId}
            agentName={agentName}
            row={servers.find((s) => s.name === block.server)}
            onOpenServer={onOpenServer}
          />
        ))
      )}
    </div>
  );
}

/**
 * One server, as it stands for this teammate.
 *
 * Its own block, so one damaged document degrades this server and nothing else.
 */
function ServerBlock({
  block,
  agentId,
  agentName,
  row,
  onOpenServer,
}: {
  block: AgentServerPermissions;
  agentId: string;
  agentName: string;
  row?: McpServer;
  onOpenServer?: (name: string) => void;
}) {
  // The lens is the agent's, so every row names which rule won. The rows are
  // drawn, not edited, so no server-mode floors are needed.
  const lens: PolicyLens = {
    kind: "agent",
    name: agentName,
    floors: {},
  };

  return (
    <Card data-testid="agent-mcp-server-block">
      <CardContent className="space-y-2">
        <div className="flex flex-wrap items-center gap-2">
          {/* The mark is the way back to the server. The two pages describe one
              relation from two ends, and navigating it should not mean returning
              to a list in between. An icon alone carries no promise about where
              it goes, so the labelled link below names the same destination. */}
          {onOpenServer ? (
            <button
              type="button"
              onClick={() => onOpenServer(block.server)}
              aria-label={`Open the ${block.server} server`}
              className="rounded-md focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              data-testid="agent-mcp-open-server"
            >
              <McpServerIcon
                iconUrl={row?.iconUrl}
                name={block.server}
                className="size-6"
              />
            </button>
          ) : (
            <McpServerIcon
              iconUrl={row?.iconUrl}
              name={block.server}
              className="size-6"
            />
          )}
          <p className="text-sm font-medium">{block.server}</p>
          {block.reached ? (
            <Badge variant="outline" data-testid="agent-mcp-reached">
              reached
            </Badge>
          ) : (
            <Badge variant="destructive" data-testid="agent-mcp-not-reached">
              not reached
            </Badge>
          )}
          {!block.enabled && <Badge variant="outline">off</Badge>}
          {onOpenServer && (
            <button
              type="button"
              onClick={() => onOpenServer(block.server)}
              className="ml-auto rounded-sm text-xs font-medium text-muted-foreground underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
              data-testid="agent-mcp-edit-on-server"
            >
              Edit on the {block.server} page →
            </button>
          )}
        </div>

        {!block.reached && (
          <p className="text-xs text-muted-foreground">
            Would need{" "}
            <code className="font-mono">
              {block.grantNeeded ?? `mcp:${block.server}`}
            </code>{" "}
            — or <code className="font-mono">mcp:*</code>. Every permission below
            is inert for {agentName} until a grant covers this server.
          </p>
        )}

        {!block.enabled && (
          <p className="text-xs text-muted-foreground">
            This server is turned off, so no teammate receives its tools whatever
            the grants say.
          </p>
        )}

        {block.unreadable ? (
          <p
            className="text-xs text-destructive"
            data-testid="agent-mcp-unreadable"
          >
            This server&apos;s stored tool permissions cannot be read, so no rows
            are shown for it — an empty list would read as &ldquo;every tool runs
            on whatever the tier says&rdquo;. Clear them on the server&apos;s own
            page. Nothing else on this page is affected.
          </p>
        ) : block.tools.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            {block.discoveredAtMillis === 0
              ? "This server has never been listed from here, so there is nothing to group yet."
              : "This server exposed no tools."}
          </p>
        ) : (
          <>
            {block.fullyRefused && (
              <p
                className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-2 py-1 text-xs font-medium text-destructive"
                data-testid="agent-mcp-fully-refused"
              >
                <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
                <span>
                  {agentName} reaches this server but can call nothing on it.
                </span>
              </p>
            )}
            <div className="space-y-2">
              {SECTION_ORDER.map((tier) => {
                const rows = block.tools.filter(
                  (r) => r.effectiveTier === tier,
                );
                if (rows.length === 0) return null;
                return (
                  <TierSection
                    key={`${agentId}-${block.server}-${tier}`}
                    tier={tier}
                    rows={rows}
                    // Read-only here: the tier's bulk default is not rendered
                    // and this value is not used.
                    bulk={{ mode: "needs_approval", stored: false }}
                    lens={lens}
                    canManage={false}
                    busy={false}
                    open
                    showAll
                    onToggleOpen={() => {}}
                    onToggleShowAll={() => {}}
                    apply={() => {}}
                    controls={false}
                    elsewhere={`the ${block.server} page`}
                  />
                );
              })}
            </div>
          </>
        )}
      </CardContent>
    </Card>
  );
}
