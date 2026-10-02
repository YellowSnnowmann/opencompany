import { ChevronDown, ChevronRight, RotateCcw } from "lucide-react";

import type {
  ApprovalMode,
  PolicySource,
  ToolPolicyPatch,
  ToolPolicyRow,
  ToolTier,
} from "@/api/mcp-tool-policy";
import { EFFECT_WORDS } from "@/api/team-mcp-permissions";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { MODE_LABELS, ModeChoice } from "@/views/mcp/McpToolPermissionsControl";

/**
 * One tier section and one tool row, for both surfaces that draw them: a
 * server's own permissions panel and a teammate's Permissions tab.
 */

/** The value the tier control carries when nothing is stored for that tier. */
export const UNSET = "unset";

/** The tier control's own vocabulary: the three modes, plus "nothing set". */
export const TIER_DEFAULT_LABELS: Record<string, string> = {
  [UNSET]: "Not set",
  ...MODE_LABELS,
};

/** The tiers, in the order they escalate. */
export const TIERS: readonly ToolTier[] = [
  "read_only",
  "interactive",
  "write_delete",
];

/** The order the sections are read in: what can do the most damage, first. */
export const SECTION_ORDER: readonly ToolTier[] = [
  "write_delete",
  "interactive",
  "read_only",
];

export const TIER_LABELS: Record<ToolTier, string> = {
  read_only: "Read-only",
  interactive: "Interactive",
  write_delete: "Write & delete",
};

/** How a suggestion is written on a row, in the brief's shorthand. */
export const SUGGESTION_LABELS: Record<ToolTier, string> = {
  read_only: "read-only",
  interactive: "interactive",
  write_delete: "write/delete",
};

export const SECTION_TITLES: Record<ToolTier, string> = {
  read_only: "Read-only tools",
  interactive: "Interactive tools",
  write_delete: "Write & delete tools",
};

/** How many unremarkable rows a section shows before it offers the rest. */
export const VISIBLE_CAP = 8;

/**
 * How restrictive a mode is, as a number the clamp can compare. A per-teammate
 * rule may only make a tool stricter.
 */
const RESTRICTION: Record<ApprovalMode, number> = {
  always_allow: 0,
  needs_approval: 1,
  blocked: 2,
};

/** Whether `mode` is less restrictive than `floor`, and so would be discarded. */
export function widens(mode: ApprovalMode, floor: ApprovalMode): boolean {
  return RESTRICTION[mode] < RESTRICTION[floor];
}

/**
 * Whose permissions a row is showing: what every teammate reaching this server
 * gets, or one teammate's narrowing resolved on top of it.
 */
export type PolicyLens =
  | { kind: "company" }
  | {
      kind: "agent";
      /** The teammate's display name, for every sentence that names them. */
      name: string;
      /** The server's own mode per tool — what a per-teammate rule may not loosen. */
      floors: Readonly<Record<string, ApprovalMode>>;
    };

/**
 * Which rule decided a row's mode. `agent_clamped` names a setting the host
 * discarded rather than one it honoured.
 */
export function sourceLabel(
  row: ToolPolicyRow,
  lens: PolicyLens,
): string | null {
  if (lens.kind !== "agent") return null;
  const who = lens.name;
  switch (row.source) {
    case "server_inherited":
      return `from the ${TIER_LABELS[row.effectiveTier]} default`;
    case "server_pinned":
      return "pinned on this server";
    case "agent_pinned":
      return `set for ${who}`;
    case "agent_clamped":
      return row.agentMode
        ? `${who}'s ${MODE_LABELS[row.agentMode]} was discarded — less restrictive than this server's`
        : `${who}'s own setting was discarded — less restrictive than this server's`;
  }
}

/** The badge tone a source reads in. */
function sourceTone(source: PolicySource): "outline" | "destructive" {
  return source === "agent_clamped" ? "destructive" : "outline";
}

/** What `allowedTools` / `disallowedTools` a row is up against, when known. */
export type ToolGate = Pick<
  { allowedTools: string[]; disallowedTools: string[] },
  "allowedTools" | "disallowedTools"
>;

/**
 * Why a tool the panel lists is unreachable regardless of what its row says.
 *
 * `allowedTools` / `disallowedTools` are a separate gate at attachment, so a row
 * can read "Needs approval" while the transport refuses the call outright.
 */
export function exclusion(
  gate: ToolGate | undefined,
  tool: string,
): string | null {
  if (!gate) return null;
  if (gate.disallowedTools.includes(tool)) return "Not sent — on the deny list";
  if (gate.allowedTools.length > 0 && !gate.allowedTools.includes(tool)) {
    return "Not sent — off the allow list";
  }
  return null;
}

/**
 * The patch a choice in the tier control means on the wire.
 *
 * A tier cleared back to unset arrives as `null`; the host reads a missing key
 * as "leave it alone".
 */
export function tierPatch(tier: ToolTier, value: string): ToolPolicyPatch {
  return {
    tierDefaults: { [tier]: value === UNSET ? null : (value as ApprovalMode) },
  };
}

/**
 * Which rows a section shows. The cap applies to the unremarkable rows only: a
 * row an operator decided, or one the transport will never send, is always
 * shown.
 */
export function visibleRows(
  gate: ToolGate | undefined,
  rows: ToolPolicyRow[],
  showAll: boolean,
): { shown: ToolPolicyRow[]; hidden: number } {
  if (showAll) return { shown: rows, hidden: 0 };
  const pinned = (row: ToolPolicyRow) =>
    row.isOverride || exclusion(gate, row.tool) !== null;
  let budget = VISIBLE_CAP;
  const shown = rows.filter((row) => {
    if (pinned(row)) return true;
    if (budget === 0) return false;
    budget -= 1;
    return true;
  });
  return { shown, hidden: rows.length - shown.length };
}

export function TierSection({
  tier,
  gate,
  rows,
  bulk,
  lens,
  canManage,
  busy,
  open,
  showAll,
  onToggleOpen,
  onToggleShowAll,
  apply,
  onClearRow,
  /** Draw what happens instead of the controls that decide it. */
  controls = true,
  elsewhere,
}: {
  tier: ToolTier;
  gate?: ToolGate;
  rows: ToolPolicyRow[];
  bulk: { mode: ApprovalMode; stored: boolean };
  lens: PolicyLens;
  canManage: boolean;
  busy: boolean;
  open: boolean;
  showAll: boolean;
  onToggleOpen: () => void;
  onToggleShowAll: () => void;
  apply: (patch: ToolPolicyPatch) => void;
  onClearRow?: (tool: string) => void;
  controls?: boolean;
  /** Where a row without controls is decided, named so the row can say it. */
  elsewhere?: string;
}) {
  const bodyId = `tier-body-${tier}`;
  const { shown, hidden } = visibleRows(gate, rows, showAll);
  // Tiers are never per-teammate: the host refuses a scoped tier write.
  const perAgent = lens.kind === "agent";
  const lensName = lens.kind === "agent" ? lens.name : "";

  return (
    <section
      className="space-y-2 rounded-md border border-border p-2"
      data-testid={`mcp-tier-section-${tier}`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={onToggleOpen}
          aria-expanded={open}
          aria-controls={bodyId}
          className="flex flex-1 items-center gap-1 text-left text-xs font-medium"
          data-testid={`mcp-tier-toggle-${tier}`}
        >
          {open ? (
            <ChevronDown className="size-3.5" />
          ) : (
            <ChevronRight className="size-3.5" />
          )}
          {SECTION_TITLES[tier]}
          <span
            className="text-muted-foreground"
            data-testid={`mcp-tier-count-${tier}`}
          >
            ({rows.length})
          </span>
        </button>
        {controls && perAgent && (
          // A teammate lens gets the value, not a control that cannot be used:
          // the tier default is the company's, and three disabled selects
          // repeating that made the one live control harder to find.
          <span
            className="text-xs text-muted-foreground"
            data-testid={`mcp-tier-company-only-${tier}`}
            title={`A tier default applies to every teammate, so it is set on the Everyone view. ${lensName} can still be narrowed tool by tool below.`}
          >
            {TIER_DEFAULT_LABELS[bulk.stored ? bulk.mode : UNSET]} · for everyone
          </span>
        )}
        {controls && !perAgent && (
          <Select
            value={bulk.stored ? bulk.mode : UNSET}
            onValueChange={(v) => v && apply(tierPatch(tier, v))}
            items={TIER_DEFAULT_LABELS}
            disabled={!canManage || busy || perAgent}
          >
            <SelectTrigger
              id={`tier-${tier}`}
              aria-label={`Default for ${SECTION_TITLES[tier].toLowerCase()}`}
              className="w-40"
              data-testid={`mcp-permissions-tier-default-${tier}`}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={UNSET}>Not set</SelectItem>
              {(Object.keys(MODE_LABELS) as ApprovalMode[]).map((mode) => (
                <SelectItem key={mode} value={mode}>
                  {MODE_LABELS[mode]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      </div>

      {open && (
        <div id={bodyId} className="space-y-2">
          {rows.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              No tool here yet. The default above still applies to any this
              server turns out to have.
            </p>
          ) : (
            <ul className="space-y-2">
              {shown.map((row) => (
                <ToolRow
                  key={row.tool}
                  row={row}
                  gate={gate}
                  lens={lens}
                  canManage={canManage}
                  busy={busy}
                  apply={apply}
                  onClearRow={onClearRow}
                  controls={controls}
                  elsewhere={elsewhere}
                />
              ))}
            </ul>
          )}
          {(hidden > 0 || showAll) && rows.length > 0 && (
            <button
              type="button"
              onClick={onToggleShowAll}
              className="text-xs text-muted-foreground underline"
              data-testid={`mcp-tier-more-${tier}`}
            >
              {showAll ? "Show fewer" : `${hidden} more`}
            </button>
          )}
        </div>
      )}
    </section>
  );
}

export function ToolRow({
  row,
  gate,
  lens,
  canManage,
  busy,
  apply,
  onClearRow,
  /** Draw what happens instead of the controls that decide it. */
  controls = true,
  elsewhere,
}: {
  row: ToolPolicyRow;
  gate?: ToolGate;
  lens: PolicyLens;
  canManage: boolean;
  busy: boolean;
  apply: (patch: ToolPolicyPatch) => void;
  onClearRow?: (tool: string) => void;
  controls?: boolean;
  /** Where this row is decided, when it is not decided here. */
  elsewhere?: string;
}) {
  const note = exclusion(gate, row.tool);
  const source = sourceLabel(row, lens);
  const floor =
    lens.kind === "agent" ? (lens.floors[row.tool] ?? row.mode) : null;
  const setOn = `Set on ${elsewhere ?? "this server's own page"}.`;

  return (
    <li className="space-y-1" data-testid="mcp-permission-row" data-tool={row.tool}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-mono text-xs">{row.tool}</span>
        {row.suggestedTier && (
          <span className="text-3xs text-muted-foreground">
            suggested: {SUGGESTION_LABELS[row.suggestedTier]}
          </span>
        )}
        {source && (
          <Badge
            variant={sourceTone(row.source)}
            className="text-3xs font-normal"
            data-testid="mcp-permission-source"
          >
            {source}
          </Badge>
        )}
        {/* The company view is true about the document and silent about its
            exceptions without this: a row reading Allow while three teammates
            are refused it invites an auditor to believe the page. */}
        {lens.kind === "company" && row.differingAgents.length > 0 && (
          <span
            className="text-3xs text-muted-foreground"
            data-testid="mcp-permission-differing"
          >
            {row.differingAgents.length === 1
              ? "1 teammate differs"
              : `${row.differingAgents.length} teammates differ`}
          </span>
        )}
        {note && (
          <Badge variant="outline" className="text-3xs text-muted-foreground">
            {note}
          </Badge>
        )}
      </div>
      {controls ? (
        <div className="flex flex-wrap items-center gap-2">
          <ModeChoice
            value={row.mode}
            label={`What happens when ${row.tool} is called`}
            disabled={!canManage}
            // A per-teammate rule may only narrow; an option the host would
            // discard is offered disabled.
            disabledModes={
              floor === null
                ? undefined
                : Object.fromEntries(
                    (Object.keys(MODE_LABELS) as ApprovalMode[])
                      .filter((mode) => widens(mode, floor))
                      .map((mode) => [
                        mode,
                        `This server already ${EFFECT_WORDS[floor]} ${row.tool}. A teammate's own rule can only be stricter.`,
                      ]),
                  )
            }
            onChange={(mode) => {
              if (!busy) apply({ tools: [{ tool: row.tool, mode }] });
            }}
          />
          <Select
            value={row.effectiveTier}
            onValueChange={(v) =>
              v && apply({ tools: [{ tool: row.tool, tier: v as ToolTier }] })
            }
            items={TIER_LABELS}
            disabled={!canManage || busy || lens.kind === "agent"}
          >
            <SelectTrigger
              aria-label={`Tier for ${row.tool}`}
              className="h-7 w-36"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {TIERS.map((tier) => (
                <SelectItem key={tier} value={tier}>
                  {TIER_LABELS[tier]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {canManage && clearable(row, lens) && (
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              aria-label={
                lens.kind === "agent"
                  ? `Clear ${lens.name}'s decision on ${row.tool}`
                  : `Clear the decision on ${row.tool}`
              }
              data-testid="mcp-permission-clear-row"
              onClick={() =>
                lens.kind === "agent" && onClearRow
                  ? onClearRow(row.tool)
                  : apply({ tools: [{ tool: row.tool }] })
              }
            >
              <RotateCcw className="size-3.5" />
            </Button>
          )}
        </div>
      ) : (
        // Disabled rather than drawn as prose, for the reason a mode a
        // per-teammate rule may not loosen is offered disabled rather than
        // hidden: a control that is present and refused says where the boundary
        // is, while a sentence in its place reads as a row that was never
        // configured.
        <div
          className="flex flex-wrap items-center gap-2"
          data-testid="mcp-permission-effect"
        >
          <ModeChoice
            value={row.mode}
            label={`What happens when ${row.tool} is called`}
            disabled
            disabledModes={Object.fromEntries(
              (Object.keys(MODE_LABELS) as ApprovalMode[]).map((mode) => [
                mode,
                setOn,
              ]),
            )}
            onChange={() => {}}
          />
          <span className="text-3xs text-muted-foreground">
            <span className="font-medium text-foreground">
              {EFFECT_WORDS[row.mode]}
            </span>
            {source ? ` — ${source}` : null} · {setOn}
          </span>
        </div>
      )}
    </li>
  );
}

/**
 * Whether this row has a decision to undo in this lens. In an agent lens that is
 * the teammate's own stored mode, present even when the clamp discarded it.
 */
function clearable(row: ToolPolicyRow, lens: PolicyLens): boolean {
  return lens.kind === "agent" ? row.agentMode !== undefined : row.isOverride;
}
