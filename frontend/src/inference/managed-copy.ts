// Copy about the legacy Managed fallback chain — moved out of the deleted
// `routing.ts` (keys rework, issue #2306, phase 5b: routing is removed).
//
// @deprecated keys-rework #2306: everything in this module describes the
// **legacy, pre-row** state — a company whose managed chain resolves through
// its account or the instance identity with no `tinyhumans` row yet (see
// `showsLegacyManagedRow` in `ProviderList.tsx`). Once such a company adds
// TinyHumans (the ordinary catalogue row, slice 2a) none of this renders for
// it again. Removable once item 10 (dropping the fallback chains) makes the
// legacy row unreachable — see `docs/key-reworks/not-handled.md`.

import type { Provider } from "./types";

/** @deprecated keys-rework #2306: see the module doc. */
export const MANAGED_NOT_SET_UP = "Managed is not set up on this company, so it is not a fallback.";

/** @deprecated keys-rework #2306: see the module doc. */
export const MANAGED_SWITCHED_OFF =
  "Managed is switched off, so it is not a fallback. Its credential is untouched.";

/**
 * What to say under the Connected list about the legacy managed chain being a
 * fallback, or `null` when the row above has already said everything true.
 *
 * @deprecated keys-rework #2306: see the module doc.
 */
export function managedFallbackNote(
  managed: { configured?: boolean; enabled?: boolean } | undefined,
): string | null {
  if (!managed) return null;
  if (managed.configured === false) return MANAGED_NOT_SET_UP;
  if (managed.enabled === false) return MANAGED_SWITCHED_OFF;
  return null;
}

/** @deprecated keys-rework #2306: see the module doc. */
export const MANAGED_TARGET_LABEL = "Managed";

/**
 * Whether this company has nothing at all that can serve a turn: every
 * provider switched off (or none added) and the legacy managed chain
 * resolving to nothing.
 *
 * @deprecated keys-rework #2306: see the module doc. Unlike the routing-era
 * version, there is no "Managed is the floor" special case here any more —
 * TinyHumans is an ordinary provider once it has a row, and this only speaks
 * to the pre-row legacy chain.
 */
export function nothingCanAnswer(providers: readonly Provider[], managedConfigured?: boolean): boolean {
  return managedConfigured === false && !providers.some((p) => p.enabled);
}
