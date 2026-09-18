// The pure decision logic behind the Account page's Remove-key confirm dialog
// (keys rework, issue #2306; docs/key-reworks/in-use-guards.md; bug KR-L3-01).
//
// PURE. No React, no fetch. This project has no component-test harness (no
// `@testing-library/react` anywhere under `test/` — see
// `test/unit/budget-pause-notice.test.ts`'s header), so anything worth a test
// has to be a function a dialog's state derives from, not markup a render
// would exercise. `ApiKeyView.tsx` calls these; it does not reimplement the
// decisions inline.
//
// `isInUseRefusal`/`GuardedOutcome`/`guardedOutcome`/`confirmInUseFor` are a
// deliberate copy of `@/composio/in-use` (also mirrored, independently, by
// `@/search-providers/in-use`) rather than a shared import: each of those
// modules is its own page's file, out of the others' dispatch scope
// (`docs/key-reworks/README.md`'s per-agent ownership), and importing across
// that boundary would couple pages that otherwise share nothing but this one
// small, already-tiny piece of logic. Keep any change to the shared *contract*
// (in-use-guards.md) in sync across all three copies by hand.
//
// This page differs from Composio's and Search's in one respect: it also
// knows the reason **before** the first attempt, because `GET …/credential`
// already carries a computed `usedBy` for the account key
// (`CompanyCredentialStatus.usedBy`, `src/server/ops/company_key.rs`'s
// `effective_status`) — the page has already fetched it to render the row, so
// showing it costs no extra request. `accountKeyUsedByMessage` builds the same
// dialog text from that upfront read that a 409 would otherwise supply after
// an uninformed first attempt refused it.

import { ApiError, type UsedBy } from "@/api/types";

/**
 * Whether a caught error is the in-use-guards refusal (409 `in_use`) the
 * guarded Remove-key dialog exists to recover from, rather than an ordinary
 * failure the dialog should just report and close on.
 */
export function isInUseRefusal(err: unknown): err is ApiError {
  return err instanceof ApiError && err.code === "in_use";
}

/** What the Remove-key dialog does after an attempt settles. */
export type GuardedOutcome = { action: "close" } | { action: "reopen"; message: string };

/**
 * What the Remove-key dialog should do with the result of one attempt.
 *
 * Mirrors `@/composio/in-use`'s `guardedOutcome` exactly (see that module for
 * the full reasoning): an attempt that already knew a reason — because the
 * status read at dialog-open time carried `usedBy`, or because a prior
 * attempt was already refused once — sends `confirmInUse: true` and any
 * further refusal on THAT attempt is a real failure, not this recoverable
 * case. An attempt that did not yet know a reason and gets refused `409
 * in_use` reopens with the host's own sentence, so a second, now-informed
 * press can resend confirmed.
 */
export function guardedOutcome(err: unknown, alreadyConfirmed: boolean): GuardedOutcome {
  if (!alreadyConfirmed && isInUseRefusal(err)) {
    return { action: "reopen", message: err.message };
  }
  return { action: "close" };
}

/**
 * Whether the Remove-key dialog's next attempt should carry
 * `confirmInUse: true` — exactly when the dialog is already showing a reason,
 * whether that came from the upfront status read or from a prior refusal.
 *
 * `reason` is the dialog's own state: `null` before anything has said the key
 * is in use, and a sentence once either source has.
 */
export function confirmInUseFor(reason: string | null): boolean {
  return reason !== null;
}

/** Display phrases for `UsedBy.surfaces` — never the raw wire id (in-use-guards.md §2, X7). */
const SURFACE_PHRASES: Record<"llm" | "composio" | "search", string> = {
  llm: "TinyHumans on the LLM page",
  composio: "Composio",
  search: "Search",
};

/**
 * The account key's own in-use sentence, for the one shape its `usedBy` ever
 * takes: `surfaces` only (`account_key_used_by`, `src/server/ops/
 * company_key.rs`, never populates `default` or `agents` — those describe an
 * inference *provider* row, which the account key is not). One plain
 * sentence naming every surface, e.g. "Used by TinyHumans on the LLM page and
 * by Composio." or, with one surface, "Used by Composio." (operator pattern,
 * 2026-09-15 review). Mirrors that function's own `account_key_in_use_message`
 * wording exactly, so the text shown from the upfront status read cannot
 * disagree with the text a later `409` would answer with for the same
 * `usedBy`.
 *
 * `null` when nothing depends on the key — the dialog's cue to fall back to
 * the page's generic {@link REMOVAL_CONSEQUENCE} text (`./account.ts`).
 */
export function accountKeyUsedByMessage(usedBy: UsedBy | undefined | null): string | null {
  const surfaces = usedBy?.surfaces;
  if (!surfaces?.length) return null;
  const phrases = surfaces.map((surface) => SURFACE_PHRASES[surface]);
  const last = phrases[phrases.length - 1];
  const rest = phrases.slice(0, -1);
  return rest.length === 0 ? `Used by ${last}.` : `Used by ${rest.join(", ")} and by ${last}.`;
}
