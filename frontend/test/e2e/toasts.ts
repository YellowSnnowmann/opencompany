import { expect, type Locator } from "@playwright/test";

/**
 * Click a control without racing the toast stack for it.
 *
 * The notification region is `position: fixed` in the bottom-right corner, over
 * whatever the page has scrolled under it. Playwright scrolls a click target
 * into view *minimally*, so a control near the bottom of a scrolled pane can
 * land right there — and then the click deadlocks rather than merely waiting:
 * sonner PAUSES a toast's auto-dismiss timer while the toaster is hovered (see
 * `toast-dismissal.spec.ts`), and so does the console's own dismissal ceiling
 * (`components/ui/sonner.tsx`, issue #933). Playwright's retry loop hovers the
 * point it is trying to click — which is the toast — so the thing in the way is
 * held in place by the attempt to reach past it. The CI log reads "subtree
 * intercepts pointer events" on every attempt until the test times out.
 *
 * Two cheaper fixes were tried against CI and neither holds. Waiting for the
 * stack to empty deadlocks for the reason above, and a live agent puts up a
 * fresh "An action is waiting for your approval." while you wait. Re-centring
 * the target in its scrollport, so the corner is somewhere it is not, still
 * landed it under an approval notice.
 *
 * So take the toasts down first, through the × an operator would use, and
 * retry — the live-brain lane runs a real agent that is free to raise another
 * one at any moment. Nothing here depends on a notice being up: the approvals
 * this spec makes are decided over the API, not from the toast.
 *
 * The product hazard underneath — an actionable control a toast can cover — is
 * issue #1303, and is not a test's to fix.
 */
export async function clickClearOfToasts(target: Locator): Promise<void> {
  // `:not([data-removed="true"])` because sonner keeps a dismissed toast
  // mounted for its exit animation, flipping that attribute rather than
  // unmounting. Without the filter the front × stays the one already clicked
  // for those frames, and the sweep spends its budget re-closing it.
  const closers = target
    .page()
    .locator('[data-sonner-toast]:not([data-removed="true"]) [data-close-button]');

  // Separate from the retry below so that "the control never rendered" and "a
  // toast held the corner" fail with different messages.
  await target.waitFor({ state: "visible", timeout: 15_000 });

  await expect(async () => {
    // Always the FRONT ×, re-queried every time. A snapshot of the stack
    // (`closers.all()`) hands back index-bound handles, and closing one
    // renumbers the rest — the second handle would then address a toast that
    // moved, or nothing, and leave one standing. Bounded by the count this pass
    // started with: a toast that arrives mid-sweep is the outer retry's, not an
    // excuse for an unbounded loop against an agent that keeps raising them.
    for (let left = await closers.count(); left > 0; left--) {
      await closers
        .first()
        .click({ timeout: 1_000 })
        .catch(() => {
          /* it left on its own, which is the outcome this loop wanted */
        });
    }
    await target.click({ timeout: 2_000 });
  }).toPass({ timeout: 15_000, intervals: [250] });
}
