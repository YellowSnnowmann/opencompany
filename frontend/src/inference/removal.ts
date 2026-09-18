// What removing, clearing, or switching a provider costs — as facts rather
// than prose — and the confirmation sentences built from them.
//
// Replaces the routing-era `removalImpact`/`removalWarnings` (moved out of the
// deleted `routing.ts`, keys rework, issue #2306, phase 5b). Per-workload
// routing is gone, so there is nothing here about routes resetting; what a
// removal can now strand is the company **default** and any **agent** pinned
// to this row — both carried on `Provider.usedBy`, the in-use guard contract
// every write in this rework refuses against (see
// `docs/key-reworks/README.md`'s confirmation contract).
//
// Decision X14 (orchestrator, 2026-09-15, confirmed over an earlier draft of
// `docs/key-reworks/in-use-guards.md` that carved out an exception for
// deleting the row): the default and every agent pair are never cleared by a
// delete, a disable, or a key clear — not even by deleting the row. Status
// flags a default or pair left pointing at a provider that is gone or turned
// off, and the console shows the banner. These sentences say that plainly,
// rather than the old "moves to <provider>" claim, which is true for none of
// the four intents.

import type { UsedBy } from "@/api/types";
import { hasUsedBy, usedBySentence } from "@/lib/used-by";
import type { Provider } from "./types";

// Re-exported for this file's existing callers — moved to `@/lib/used-by.ts`
// (round-3 review) so Search and Composio's own guarded dialogs can share the
// same sentence instead of each composing their own wording.
export { hasUsedBy, usedBySentence };

/** The four things that can be done to a provider from its row. */
export type ProviderIntent = "disable" | "enable" | "key" | "provider";

/** What acting on a provider would cost, read straight off the row the host sent. */
export interface RemovalImpact {
  /** Whether it is the last provider this company has switched on. */
  lastEnabled: boolean;
  /** What the host says still depends on this row. Absent means nothing does. */
  usedBy?: UsedBy;
}

export function removalImpact(
  provider: Pick<Provider, "slug" | "enabled" | "usedBy">,
  providers: readonly Provider[],
): RemovalImpact {
  const remaining = providers.filter((p) => p.slug !== provider.slug);
  return {
    lastEnabled: provider.enabled && !remaining.some((p) => p.enabled),
    usedBy: provider.usedBy,
  };
}

/**
 * The sentences a confirmation says, in the order they matter.
 *
 * A confirmation that only asks "are you sure?" is a speed bump, not a
 * safeguard: it tells the operator nothing they did not already know and
 * trains them to click through. These name what is actually about to change,
 * one sentence per fact, so the one an operator cares about is visible at a
 * glance rather than buried in a paragraph.
 *
 * `usedBy` is the host's own answer (issue #2306's in-use guard contract), so
 * these sentences cannot disagree with what the backend is about to refuse (or
 * has already refused, on a `409 in_use` reopening this same dialog with a
 * fresh `usedBy`).
 */
export function removalWarnings(intent: ProviderIntent, label: string, impact: RemovalImpact): string[] {
  if (intent === "enable") return enableWarnings(label);
  if (intent === "disable") return disableWarnings(label, impact);
  const lines: string[] =
    intent === "key"
      ? [`${label} stays on this page, keeping its endpoint. It just has no credential, so it cannot answer until you add one.`]
      : [`This deletes ${label} and clears its stored key in the same operation. Adding it again means entering the credential again.`];
  appendUsedByLines(lines, impact, intent);
  if (impact.lastEnabled && intent === "provider") {
    lines.push("It is the only provider switched on. Nothing will be able to think until another is added or switched back on.");
  }
  return lines;
}

/**
 * What switching a provider **off** costs — the reversible one of the four,
 * and deliberately different language from the other three: disabling keeps
 * the endpoint, the key and the row itself, and the host refuses to strand
 * anything silently — it names what depends on this row the same as a remove
 * would.
 */
function disableWarnings(label: string, impact: RemovalImpact): string[] {
  const lines: string[] = [
    `${label} keeps its endpoint and its key. Switching it back on restores both — nothing is deleted.`,
  ];
  appendUsedByLines(lines, impact, "disable");
  if (impact.lastEnabled) {
    lines.push("It is the only provider switched on. Nothing will be able to think while it is off.");
  }
  return lines;
}

/** Turning a provider back on. Light — nothing it depends on can be stranded by this. */
function enableWarnings(label: string): string[] {
  return [`This switches ${label} back on. It becomes reachable for the company default and any agent pinned to it.`];
}

/**
 * Appends the `usedBy`-derived lines shared by every destructive intent.
 *
 * The naming sentence — "Used by the company default and 2 agents: ..." —
 * comes first (round-2 review, P1-5), with the X14 consequence lines after
 * it: none of the four intents ever clears the default or an agent's pin, so
 * these say what actually happens rather than a "moves to" claim that is true
 * for none of them.
 */
function appendUsedByLines(lines: string[], impact: RemovalImpact, intent: ProviderIntent): void {
  const sentence = usedBySentence(impact.usedBy);
  if (sentence) lines.push(sentence);
  const verb = intent === "disable" ? "Switching it off" : intent === "key" ? "Clearing its key" : "Removing it";
  if (impact.usedBy?.default) {
    // Decision X14: none of the four intents clears the default, ever. It
    // just stops resolving, and the company sees a banner until an admin
    // picks a new one — said here so the confirmation cannot promise a
    // fallback that does not happen.
    lines.push(`${verb} does not change the default — turns will fail until you choose a new one.`);
  }
  if (impact.usedBy?.agents?.length) {
    lines.push("Their turns will fail until you give them a different pair or clear their pin.");
  }
}
