// The pure decision logic behind the search page's in-use confirm dialogs
// (keys rework, issue #2306; docs/key-reworks/in-use-guards.md).
//
// PURE. No React, no fetch. Mirrors `@/composio/in-use` — the same two
// decisions (`isInUseRefusal`, `guardedOutcome`) apply to every guarded
// mutation on this host, and Composio's own module says why they live as
// plain functions here rather than inline in a component: this project has no
// component-test harness (no `@testing-library/react` anywhere under `test/`
// — see `test/unit/budget-pause-notice.test.ts`'s header), so anything worth
// a test has to be a function a dialog's state derives from, not markup a
// render would exercise.
//
// Kept as its own copy rather than imported from `@/composio/in-use`: that
// module is Composio's own page's file (out of this dispatch's scope — see
// `docs/key-reworks/README.md`'s per-agent ownership), and importing across
// that boundary would couple two pages that otherwise share nothing but this
// one small, already-tiny piece of logic. `SearchView.tsx` calls these; it
// does not reimplement the decisions inline.

import { ApiError, type UsedBy } from "@/api/types";

/**
 * Whether a caught error is the in-use-guards refusal (409 `in_use`) a
 * guarded confirm dialog exists to recover from, rather than an ordinary
 * failure the dialog should just report and close on.
 */
export function isInUseRefusal(err: unknown): err is ApiError {
  return err instanceof ApiError && err.code === "in_use";
}

/** What a guarded confirm dialog does after an attempt settles. */
export type GuardedOutcome =
  | { action: "close" }
  | { action: "reopen"; message: string; usedBy?: UsedBy };

/**
 * What a guarded confirm dialog should do with the result of one attempt.
 *
 * Round-3 review, P1-2: a guarded dialog now shows the row's own `usedBy`
 * on open (read fresh, from the already-loaded list or a re-fetch — see
 * `SearchView.tsx`'s `openConfirm`), so `alreadyConfirmed` is no longer only
 * "did a prior 409 already inform this dialog" — it is "did the operator see
 * a reason before clicking", which the FIRST click can already satisfy. The
 * `409` path below still exists for a stale read: the row changed between the
 * dialog opening and the click landing.
 *
 * - it lands (nothing was in use, or `alreadyConfirmed` was already true) →
 *   `"close"`;
 * - it fails for any reason OTHER than a fresh, unconfirmed in-use refusal →
 *   `"close"` — the caller shows the real error via the ordinary error path,
 *   never a generic toast (in-use-guards.md's own UI convention: always the
 *   host's message);
 * - it is refused `409 in_use` and this attempt had not yet confirmed →
 *   `"reopen"` with the host's own sentence and its own fresher `usedBy`, so a
 *   SECOND, now-informed click can resend with `confirmInUse: true`.
 *
 * `alreadyConfirmed` distinguishes a stale-UI first attempt from a confirmed
 * attempt that still failed: once a dialog is showing a host-provided notice,
 * the caller's next send already carries `confirmInUse: true`, so a second
 * `in_use` on THAT attempt would mean the confirmed write itself was refused
 * again — not the case this function exists to recover from — and closes like
 * any other failure.
 */
export function guardedOutcome(
  err: unknown,
  alreadyConfirmed: boolean,
): GuardedOutcome {
  if (!alreadyConfirmed && isInUseRefusal(err)) {
    return { action: "reopen", message: err.message, usedBy: err.usedBy };
  }
  return { action: "close" };
}

/**
 * Whether a guarded confirm dialog's next attempt should carry
 * `confirmInUse: true` — exactly when the dialog is already showing a
 * host-reported reason from a prior refusal.
 *
 * `notice` is the dialog's own state: `null` before any attempt has been
 * refused, and the host's sentence after {@link guardedOutcome} said
 * `"reopen"`.
 */
export function confirmInUseFor(notice: string | null): boolean {
  return notice !== null;
}
