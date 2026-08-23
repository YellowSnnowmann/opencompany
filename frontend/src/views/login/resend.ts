/**
 * The "check your email" screen's resend clock (issue #1333).
 *
 * The host rate-limits `auth/request` to one mail per address per minute and
 * says nothing about it: a throttled request returns the *same* `202` as a sent
 * one, deliberately, so that the throttle cannot become the membership oracle
 * the rest of that route refuses to be
 * (`RESEND_INTERVAL_MILLIS` in `src/server/users/routes.rs`).
 *
 * That silence is what forces the countdown to live here. The console cannot
 * ask whether a resend would land, and it cannot tell a throttled `202` from a
 * delivered one — so the only honest resend button is one that will not fire
 * until the window it can measure itself has passed. Mirroring the host's
 * constant is not duplication for its own sake; it is the only signal there is.
 */

/**
 * How long the host makes an address wait between mails.
 *
 * Must equal `RESEND_INTERVAL_MILLIS` on the host. Too short and the button
 * fires into a throttle and reports a send that never happened; too long and it
 * withholds a resend the host would have honoured.
 */
export const RESEND_INTERVAL_MILLIS = 60 * 1000;

/**
 * Whole seconds left before another link may be asked for, `0` once it may.
 *
 * Rounded **up**, so the last fractional second still reads as `1` rather than
 * as a ready button that is not: the host measures from the moment it minted
 * the code, which is fractionally before the moment the response reached us,
 * and rounding down would let the click land inside a window that is still
 * open at the other end.
 *
 * Clamped at both ends. A `now` behind `sentAtMillis` is not a 40-minute wait
 * to be rendered — it is a clock that moved (a laptop resumed, an NTP step),
 * and the interval is the most it can ever legitimately be.
 */
export function secondsUntilResend(sentAtMillis: number, nowMillis: number): number {
  const remaining = sentAtMillis + RESEND_INTERVAL_MILLIS - nowMillis;
  if (remaining <= 0) return 0;
  return Math.min(Math.ceil(remaining / 1000), RESEND_INTERVAL_MILLIS / 1000);
}

/**
 * What the resend control says.
 *
 * The countdown is in the label rather than beside it because the label is what
 * a screen reader announces for a disabled button: a bare "Resend link" that
 * refuses to be pressed, with the reason rendered as adjacent text, is the
 * disabled-affordance-that-will-not-explain-itself this console keeps filing
 * bugs about.
 */
export function resendLabel(secondsLeft: number): string {
  return secondsLeft > 0 ? `Resend link in ${secondsLeft}s` : "Resend link";
}
