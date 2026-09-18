// The "routing is going away" banner's copy (keys rework, issue #2306, phase
// 5a). Pure and self-contained: it must not import from a routing module,
// because per-workload routing itself is deleted in the same PR (phase 5b) —
// this is purely a report of what a stored routing table used to say, read
// only for the one release this banner exists to bridge.

import type { InferenceStatus } from "@/api/inference";

type RouteNotCarried = NonNullable<InferenceStatus["routesNotCarried"]>[number];

const TIER_WORD: Record<string, string> = {
  "chat-v1": "chat",
  "reasoning-v1": "reasoning",
  "agentic-v1": "agentic",
  "vision-v1": "vision",
};

/** `acme:test-model` → `acme · test-model`; `managed` → `Managed`; `acme` → `acme`. */
export function describeRoute(route: string): string {
  const raw = route.trim();
  if (raw === "managed") return "Managed";
  const colon = raw.indexOf(":");
  if (colon < 0) return raw;
  const slug = raw.slice(0, colon).trim();
  const model = raw.slice(colon + 1).trim();
  return model ? `${slug} · ${model}` : slug;
}

/** `chat → acme · test-model, reasoning → Managed`. */
export function describeRows(rows: readonly RouteNotCarried[]): string {
  return rows.map((row) => `${TIER_WORD[row.tier] ?? row.tier} → ${describeRoute(row.route)}`).join(", ");
}

/** The banner sentence, or `null` when the host sent nothing to say. */
export function routesNotCarriedCopy(rows: readonly RouteNotCarried[] | null | undefined): string | null {
  if (!rows || rows.length === 0) return null;
  return (
    `Routing is going away. Each workload used: ${describeRows(rows)}. ` +
    "Choose one default provider and model, and pin agents that need something else."
  );
}
