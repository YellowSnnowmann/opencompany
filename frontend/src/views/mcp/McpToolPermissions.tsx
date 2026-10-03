import { useCallback, useEffect, useRef, useState } from "react";
import { AlertTriangle, Info, Loader2 } from "lucide-react";

import type { OpenCompanyClient } from "@/api/client";
import {
  type ApprovalMode,
  type PolicyScope,
  type ToolPolicyDocument,
  type ToolPolicyPatch,
  type ToolTier,
  policyTarget,
  readToolPolicy,
  resetToolPolicy,
  writeToolPolicy,
} from "@/api/mcp-tool-policy";
import { EFFECT_WORDS } from "@/api/team-mcp-permissions";
import { ApiError, type McpServer, type RosterAgent } from "@/api/types";
import { TeammateAvatar } from "@/components/teammate-avatar";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useHashParam } from "@/hooks/use-hash-param";
import { avatarFor } from "@/lib/team";
import {
  type PolicyLens,
  SECTION_ORDER,
  TierSection,
  tierPatch,
} from "@/views/mcp/tool-policy-rows";

export { tierPatch };

/** The lens value that means the company document rather than a teammate. */
const EVERYONE = "everyone";

interface Props {
  client: OpenCompanyClient;
  company: string | null;
  server: McpServer;
  /** Writes are an admin's. The host answers 403 whatever this says. */
  canManage: boolean;
  /**
   * Bumped by the page when a probe re-ran against this server.
   *
   * A probe rewrites the stored inventory, and the inventory is what a tier
   * default resolves against.
   */
  reloadKey?: number;
  /** Scroll this into view once it has something to show. */
  focus?: boolean;
  /** The teammates the scope lens may show. An empty list renders no lens. */
  agents?: RosterAgent[];
  /**
   * Whether approval parking is live on this host. `false` means a
   * `needs_approval` mode behaves as allow; `undefined` means no read answers it,
   * and nothing is claimed either way.
   */
  approvalsPark?: boolean;
}

type State =
  | { kind: "loading" }
  | { kind: "ready"; doc: ToolPolicyDocument }
  /** The stored document cannot be parsed. Clearing it is the way back. */
  | { kind: "unreadable"; message: string }
  | { kind: "failed"; message: string };

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** What one scope's rows add up to, in the sentence an operator is checking. */
function tally(modes: ApprovalMode[]): {
  total: number;
  callable: number;
  refused: number;
  asks: number;
} {
  const refused = modes.filter((m) => m === "blocked").length;
  const asks = modes.filter((m) => m === "needs_approval").length;
  return {
    total: modes.length,
    callable: modes.length - refused,
    refused,
    asks,
  };
}

export function McpToolPermissions({
  client,
  company,
  server,
  canManage,
  reloadKey = 0,
  focus = false,
  agents = [],
  approvalsPark,
}: Props) {
  const [state, setState] = useState<State>({ kind: "loading" });
  const [busy, setBusy] = useState(false);
  const [writeError, setWriteError] = useState<string | null>(null);
  const [opened, setOpened] = useState<Partial<Record<ToolTier, boolean>>>({});
  const [showAll, setShowAll] = useState<Partial<Record<ToolTier, boolean>>>(
    {},
  );
  // Which teammate the panel is resolved for, or `EVERYONE` for the company
  // document. Kept in the address, so the lens is linkable and Back undoes a
  // switch.
  const [showing, setShowing] = useHashParam("showing");
  const lensValue = showing ?? EVERYONE;
  /**
   * The company's own mode per tool, which a per-teammate rule may narrow but
   * never loosen.
   *
   * Read alongside the agent document: for a row the teammate has pinned, the
   * server's mode is not recoverable from the resolved one.
   */
  const [floors, setFloors] = useState<Record<string, ApprovalMode>>({});
  const root = useRef<HTMLDivElement | null>(null);

  const agent = agents.find((a) => a.id === lensValue) ?? null;
  const scope: PolicyScope = agent?.id ?? null;
  const lens: PolicyLens = agent
    ? { kind: "agent", name: agent.name, floors }
    : { kind: "company" };

  const tools = state.kind === "ready" ? state.doc.tools : [];
  const openByDefault =
    SECTION_ORDER.find((tier) =>
      tools.some((row) => row.effectiveTier === tier),
    ) ?? SECTION_ORDER[0];

  const target = policyTarget(server);
  const targetKey = target
    ? target.kind === "registry"
      ? `registry:${target.serverId}`
      : `declared:${target.name}`
    : null;
  // Read by `apply`/`reset` after their await resolves, so a write started
  // against one server never lands on another's panel if the selection moves to
  // a different server while the request is in flight, and a write or a read for
  // one scope cannot paint another's answer.
  const scopeKey = `${targetKey ?? ""}|${scope ?? ""}`;
  const scopeKeyRef = useRef(scopeKey);
  scopeKeyRef.current = scopeKey;

  useEffect(() => {
    if (!target) {
      setState({
        kind: "failed",
        message:
          "This row has no install behind it, so it carries no permissions.",
      });
      return;
    }
    let live = true;
    setState({ kind: "loading" });
    setWriteError(null);
    void (async () => {
      try {
        const [companyDoc, doc] = scope
          ? await Promise.all([
              readToolPolicy(client, company, target, null),
              readToolPolicy(client, company, target, scope),
            ])
          : await (async () => {
              const only = await readToolPolicy(client, company, target, null);
              return [only, only] as const;
            })();
        if (!live) return;
        setFloors(
          Object.fromEntries(companyDoc.tools.map((r) => [r.tool, r.mode])),
        );
        setState({ kind: "ready", doc });
      } catch (err) {
        if (!live) return;
        setState(
          err instanceof ApiError && err.code === "policy_unreadable"
            ? { kind: "unreadable", message: err.message }
            : { kind: "failed", message: message(err) },
        );
      }
    })();
    return () => {
      live = false;
    };
    // `target` is rebuilt every render from the row; `targetKey` is the value
    // this effect actually depends on.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, company, targetKey, scope, reloadKey]);

  useEffect(() => {
    if (focus && state.kind !== "loading")
      root.current?.scrollIntoView({ block: "nearest" });
  }, [focus, state.kind]);

  const apply = useCallback(
    async (patch: ToolPolicyPatch) => {
      if (!target) return;
      const key = scopeKey;
      setBusy(true);
      setWriteError(null);
      try {
        const doc = await writeToolPolicy(
          client,
          company,
          target,
          patch,
          scope,
        );
        if (scopeKeyRef.current === key) setState({ kind: "ready", doc });
      } catch (err) {
        if (scopeKeyRef.current === key) setWriteError(message(err));
      } finally {
        setBusy(false);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [client, company, scopeKey],
  );

  const reset = useCallback(
    async (of: PolicyScope) => {
      if (!target) return;
      const key = scopeKey;
      setBusy(true);
      setWriteError(null);
      try {
        const doc = await resetToolPolicy(client, company, target, of);
        if (scopeKeyRef.current === key) setState({ kind: "ready", doc });
      } catch (err) {
        if (scopeKeyRef.current === key) setWriteError(message(err));
      } finally {
        setBusy(false);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [client, company, scopeKey],
  );

  // One teammate's row, cleared without touching the company document. The
  // per-tool reset in the agent lens is a patch naming neither field.
  const clearRow = useCallback(
    (tool: string) => {
      void apply({ tools: [{ tool }] });
    },
    [apply],
  );

  const counts = tally(tools.map((row) => row.mode));

  return (
    <div className="space-y-3" data-testid="mcp-tool-permissions" ref={root}>
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="space-y-0.5">
          <p className="text-sm font-medium">Tool permissions</p>
          <p className="text-xs text-muted-foreground">
            Suggested tiers below are a starting guess, not enforced — set your
            own to override. &ldquo;Block&rdquo; refuses the call outright: no
            approver can wave it through.
          </p>
        </div>
        {agents.length > 0 && (
          <div className="flex items-center gap-2">
            <span className="text-xs text-muted-foreground">Showing:</span>
            <Select
              value={lensValue}
              onValueChange={(v) =>
                v && setShowing(v === EVERYONE ? null : v)
              }
              items={{
                [EVERYONE]: "Everyone (company default)",
                ...Object.fromEntries(agents.map((a) => [a.id, a.name])),
              }}
            >
              <SelectTrigger
                aria-label="Whose tool permissions to show"
                className="h-7 max-w-56"
                data-testid="mcp-permissions-lens"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={EVERYONE}>
                  Everyone (company default)
                </SelectItem>
                {agents.map((a) => (
                  <SelectItem key={a.id} value={a.id}>
                    <span className="flex items-center gap-2">
                      <TeammateAvatar
                        name={a.name}
                        avatar={avatarFor(a.id)}
                        className="size-4"
                      />
                      {a.name}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        )}
      </div>

      {/* Selecting a teammate makes every control on this page write to that
          teammate's document instead of the company's. That is the one change
          here whose cost is paid by someone the operator is not looking at, so
          it is stated rather than left to the value in the lens. */}
      {agent !== null && (
        <p
          className="flex items-start gap-2 rounded-md border border-border bg-muted/40 px-2 py-1 text-xs text-muted-foreground"
          data-testid="mcp-permissions-scope-notice"
        >
          <Info className="mt-0.5 size-3.5 shrink-0" />
          <span>
            Scoped to{" "}
            <strong className="font-medium text-foreground">
              {agent.name}
            </strong>
            : a change on this page applies to {agent.name} alone and leaves the
            company default as it is. Switch back to Everyone to edit what every
            teammate gets.
          </span>
        </p>
      )}

      {/* A function of the host's own flag, never a constant: when approvals
          park again this disappears on its own rather than needing a release to
          take a sentence back out. */}
      {approvalsPark === false && (
        <p
          className="flex items-start gap-2 rounded-md border border-status-blocked-text/30 bg-status-blocked-text/10 px-2 py-1 text-xs text-status-blocked-text"
          data-testid="mcp-approvals-inert"
        >
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
          <span>
            <strong className="font-medium">
              &ldquo;Needs approval&rdquo; does not stop anything on this build.
            </strong>{" "}
            Policy-generated approvals are off, so a tool set to Needs approval
            runs exactly as if it were set to Allow. Only Allow and Block differ
            today. An agent can still ask for approval itself.
          </span>
        </p>
      )}

      {state.kind === "loading" && (
        <p className="flex items-center gap-1 text-xs text-muted-foreground">
          <Loader2 className="size-3 animate-spin" /> Reading this server&apos;s
          permissions…
        </p>
      )}

      {state.kind === "failed" && (
        <p
          className="text-xs text-destructive"
          data-testid="mcp-permissions-failed"
        >
          {state.message}
        </p>
      )}

      {state.kind === "unreadable" && (
        <div className="space-y-2" data-testid="mcp-permissions-unreadable">
          <p className="text-xs text-destructive">{state.message}</p>
          <p className="text-xs text-muted-foreground">
            No rows are shown: an empty list would read as &ldquo;every tool runs
            on whatever the tier says&rdquo;, and a save from that view would
            make it true. Clearing drops the stored document and leaves the
            declaration as the policy.
            {agent !== null &&
              " Only the company-wide reset repairs it — one teammate's layer cannot be removed from a document that will not parse."}
          </p>
          {canManage && (
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void reset(null)}
              data-testid="mcp-permissions-clear"
            >
              {busy ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                "Clear stored permissions"
              )}
            </Button>
          )}
        </div>
      )}

      {state.kind === "ready" && (
        <>
          {!canManage && (
            <p
              className="text-xs text-muted-foreground"
              data-testid="mcp-permissions-read-only"
            >
              Changing what agents may call is an admin&apos;s. You can read
              every decision on this page.
            </p>
          )}

          {state.doc.tools.length === 0 && (
            <p
              className="text-xs text-muted-foreground"
              data-testid="mcp-permissions-empty"
            >
              No tools are listed for this server yet. A tier default below
              still applies to every tool in it; re-check the server to list
              what it actually has.
            </p>
          )}

          {agent !== null && counts.total > 0 && (
            <p
              className="text-xs text-muted-foreground"
              data-testid="mcp-permissions-summary"
            >
              <span className="font-medium text-foreground">{agent.name}</span>{" "}
              can call {counts.callable} of {counts.total} tools —{" "}
              {counts.refused} refused, {counts.asks} asks.
            </p>
          )}

          {agent !== null && counts.total > 0 && counts.callable === 0 && (
            <p
              className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-2 py-1 text-xs font-medium text-destructive"
              data-testid="mcp-permissions-fully-refused"
            >
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
              <span>
                {agent.name} reaches this server but can call nothing on it.{" "}
                {canManage && (
                  // The way out, stated where the state is announced: the same
                  // reset otherwise sits below every tier group, off-screen.
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void reset(agent.id)}
                    data-testid="mcp-permissions-fully-refused-clear"
                    className="font-medium underline underline-offset-2 disabled:opacity-60"
                  >
                    Clear every rule set for {agent.name}
                  </button>
                )}
              </span>
            </p>
          )}

          <div className="space-y-3">
            {SECTION_ORDER.map((tier) => (
              <TierSection
                key={tier}
                tier={tier}
                gate={server}
                rows={state.doc.tools.filter(
                  (row) => row.effectiveTier === tier,
                )}
                bulk={state.doc.tierDefaults[tier]}
                lens={lens}
                canManage={canManage}
                busy={busy}
                open={opened[tier] ?? tier === openByDefault}
                showAll={showAll[tier] === true}
                onToggleOpen={() =>
                  setOpened((was) => ({
                    ...was,
                    [tier]: !(was[tier] ?? tier === openByDefault),
                  }))
                }
                onToggleShowAll={() =>
                  setShowAll((was) => ({
                    ...was,
                    [tier]: !(was[tier] ?? false),
                  }))
                }
                apply={apply}
                onClearRow={clearRow}
              />
            ))}
          </div>

          {agent !== null && canManage && (
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => void reset(agent.id)}
              data-testid="mcp-permissions-clear-agent"
            >
              Clear every rule set for {agent.name}
            </Button>
          )}

          {writeError && (
            <p
              className="text-xs text-destructive"
              data-testid="mcp-permissions-write-error"
            >
              {writeError}
            </p>
          )}
        </>
      )}
    </div>
  );
}

/** What a resolved mode does. */
export { EFFECT_WORDS };
