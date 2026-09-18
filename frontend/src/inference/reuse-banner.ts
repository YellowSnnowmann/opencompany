// The account-key reuse banner's shared decision logic (keys rework, issue
// #2306, slice 4c; docs/key-reworks/phase-4c-reuse-banner.md §3.4). PURE — no
// React, no fetch — mirroring `@/composio/in-use`'s own reasoning: this
// project has no component-test harness, so anything worth a test has to be a
// function a component's state derives from, not markup a render would
// exercise. `ComposioSection.tsx` and (once wired) `ProvidersTab.tsx` call
// these; neither reimplements a decision inline.
//
// Moved here from `@/composio/reuse-banner` (round-3b review, item 6): that
// module's own header argued the LLM half could never share this file because
// its visibility also depends on `defaultChoice`/agent-pair references
// Composio has no analogue for. That is still true of `showsInferenceReuseBanner`
// itself — the two predicates below take different, non-overlapping
// arguments and neither calls the other — but a shared *file* costs nothing
// once both live here, and `reuseDismissKey` was already parametrised by
// `page` to avoid the two halves importing each other. One file, two
// unrelated functions.
//
// `showsInferenceReuseBanner` is not implemented yet: it needs
// `InferenceStatus.accountKeyAvailable` / `.tinyhumansReferenced`, which land
// on the host with the `POST …/inference/tinyhumans/key/from-account` route
// (phase-4c-reuse-banner.md §3.3-3.4) — not yet on `origin/feat/key-reworks-impl`
// as of this file. Add it here, beside `showsComposioReuseBanner`, once that
// route and its status fields land; `ProvidersTab.tsx`'s render wiring is the
// same follow-up.

import type { ComposioMode } from "@/api/composio";

/**
 * Whether the Composio page should offer "use the same key for Composio?".
 *
 * `canManage` gates it exactly as every other Composio write is gated
 * (members never see it; the host would refuse the write anyway). The other
 * four conditions:
 *
 * - `accountConfigured` — the company has an account key at all
 *   (`GET …/credential`'s existing `configured` field). Nothing to reuse
 *   otherwise.
 * - `mode === "managed"` — the banner only ever offers to fill the *managed*
 *   TinyHumans slot; a BYOK company's own Composio account has nothing to do
 *   with the account key, and offering this there would read as though it
 *   could switch the route, which it cannot ({@link copyAccountKeyToComposio}
 *   is a single-slot copy, never a mode switch).
 * - `managedCredentialSource !== "static"` — `static` means the managed slot
 *   already has its own key (a token pasted directly, or a static instance
 *   key): nothing is missing, so there is nothing to offer. Every other
 *   tier — `none` (nothing resolves) and `company`/`attested`
 *   (already resolving through *something else*, i.e. still not this
 *   company's own pasted managed token) — is a state where filling the
 *   managed slot from the account key is a real, useful action.
 * - `!dismissed` — the operator already said "not now" this session/company.
 */
export function showsComposioReuseBanner(a: {
  canManage: boolean;
  accountConfigured: boolean;
  mode: ComposioMode | undefined;
  managedCredentialSource: string | undefined;
  dismissed: boolean;
}): boolean {
  return (
    a.canManage &&
    a.accountConfigured &&
    a.mode === "managed" &&
    a.managedCredentialSource !== "static" &&
    !a.dismissed
  );
}

/**
 * The `localStorage` key a "Not now" dismissal is recorded under, namespaced
 * per page and per company so dismissing one page's banner never hides the
 * other page's, or another company's.
 */
export function reuseDismissKey(
  page: "inference" | "composio",
  company: string | null,
): string {
  return `oc.reuse-account-key.${page}.${company ?? "_"}`;
}

/**
 * Whether `key` was dismissed. `false` on any read failure — Safari private
 * mode and a "block all cookies" setting both make `localStorage` itself
 * throw rather than return a dud object, and a banner that cannot remember a
 * dismissal must still render correctly (shown, not stuck hidden).
 */
export function readDismissed(key: string): boolean {
  try {
    return window.localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

/**
 * Record a dismissal. A failure to write is not worth failing a render over
 * — the banner simply reappears next time, which is the safe direction.
 */
export function writeDismissed(key: string): void {
  try {
    window.localStorage.setItem(key, "1");
  } catch {
    // A full, blocked, or read-only store is not worth surfacing here.
  }
}
