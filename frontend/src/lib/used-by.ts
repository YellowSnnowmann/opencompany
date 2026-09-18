// The shared `usedBy` sentence, for every guarded confirm dialog on the host
// (keys rework, issue #2306): the LLM page, Composio, and Search.
//
// Moved here (round-3 review) from `@/inference/removal.ts`, which built it
// first for the LLM page — but `UsedBy` itself is not an LLM concept (it is
// `@/api/types`'s shared shape for anything a mutation would remove, clear,
// disable or switch), and the same wording belongs on every surface that
// carries one. `@/inference/removal.ts` re-exports these two for its
// existing callers rather than having every one of them re-import from here.

import type { UsedBy } from "@/api/types";

/** "2 agents: Researcher, Web search" / "1 agent: Researcher". */
function describeAgents(agents: readonly { id: string; name: string }[]): string {
  const names = agents.map((a) => a.name).join(", ");
  return `${agents.length} ${agents.length === 1 ? "agent" : "agents"}: ${names}`;
}

/** Display names for `UsedBy.surfaces` — never the raw wire id (X7). */
const SURFACE_LABELS: Record<"llm" | "composio" | "search", string> = {
  llm: "LLM",
  composio: "Composio",
  search: "Search",
};

/** "a, b and c" — the join every combined `usedBy` sentence below shares. */
function joinParts(parts: readonly string[]): string {
  if (parts.length === 0) return "";
  if (parts.length === 1) return parts[0];
  if (parts.length === 2) return `${parts[0]} and ${parts[1]}`;
  return `${parts.slice(0, -1).join(", ")}, and ${parts[parts.length - 1]}`;
}

/** Whether `usedBy` names anything at all — the same check a confirm needs to decide whether to send `confirmInUse`. */
export function hasUsedBy(usedBy: UsedBy | undefined | null): boolean {
  return Boolean(usedBy?.default || usedBy?.agents?.length || usedBy?.surfaces?.length);
}

/**
 * One sentence naming everything a row's `usedBy` says still depends on it —
 * "Used by the company default and 2 agents: Researcher, Web search." — built
 * once so every confirm dialog on every surface says the same thing instead
 * of composing its own wording per field (round-2 review, P1-5). `null` when
 * nothing is set.
 *
 * `ownSurface` excludes this row's own surface from the "other surfaces"
 * naming — a provider's `usedBy.surfaces` lists the *other* things its
 * credential also backs, never itself.
 */
export function usedBySentence(
  usedBy: UsedBy | undefined | null,
  ownSurface: "llm" | "composio" | "search" = "llm",
): string | null {
  const parts: string[] = [];
  if (usedBy?.default) parts.push("the company default");
  if (usedBy?.agents?.length) parts.push(describeAgents(usedBy.agents));
  const otherSurfaces = (usedBy?.surfaces ?? []).filter((s) => s !== ownSurface);
  if (otherSurfaces.length) parts.push(otherSurfaces.map((s) => SURFACE_LABELS[s]).join(" and "));
  if (!parts.length) return null;
  return `Used by ${joinParts(parts)}.`;
}
