// The pure decision logic behind the Composio in-use confirm dialogs
// (keys rework, issue #2306; docs/key-reworks/in-use-guards.md).
//
// PURE. No React, no fetch. This project has no component-test harness (no
// `@testing-library/react` anywhere under `test/` — see
// `test/unit/budget-pause-notice.test.ts`'s header), so anything worth a test
// has to be a function a dialog's state derives from, not markup a render
// would exercise. `ComposioSection.tsx` calls these; it does not reimplement
// the decisions inline.

import { ApiError } from "@/api/types";

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
  | { action: "reopen"; message: string };

/**
 * What a guarded confirm dialog should do with the result of one attempt.
 *
 * Every one of these dialogs opens with a plain, generic question — the
 * client does not know in advance what depends on the credential, and
 * computing that eagerly would cost every dialog open a round trip to find
 * out something that is usually "nothing" (in-use-guards.md §1: the field is
 * omitted when nothing uses the thing). So the first confirm click sends the
 * mutation WITHOUT `confirmInUse`, and:
 *
 * - it lands (nothing was in use, or `alreadyConfirmed` was already true) →
 *   `"close"`;
 * - it fails for any reason OTHER than a fresh, unconfirmed in-use refusal →
 *   `"close"` — the caller shows the real error via the ordinary error path,
 *   never a generic toast (in-use-guards.md's own UI convention: always the
 *   host's message);
 * - it is refused `409 in_use` and this attempt had not yet confirmed →
 *   `"reopen"` with the host's own sentence, so a SECOND, now-informed click
 *   can resend with `confirmInUse: true` (in-use-guards.md §2: "A 409
 *   response from a stale UI re-opens the dialog with the server's
 *   usedBy/message rather than a generic error toast").
 *
 * `alreadyConfirmed` distinguishes a stale-UI first attempt from a confirmed
 * attempt that still failed: once a dialog is showing a host-provided notice,
 * the caller's next send already carries `confirmInUse: true` (see
 * {@link confirmInUseFor}), so a second `in_use` on THAT attempt would mean
 * the confirmed write itself was refused again — not the case this function
 * exists to recover from — and closes like any other failure.
 */
export function guardedOutcome(
  err: unknown,
  alreadyConfirmed: boolean,
): GuardedOutcome {
  if (!alreadyConfirmed && isInUseRefusal(err)) {
    return { action: "reopen", message: err.message };
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
