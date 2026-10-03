import { expect, test } from "@playwright/test";

/**
 * Issue #583 — every tour stop's card must be reachable, at any window size.
 *
 * The reported symptom was "the tour is stuck at step 2", and the issue blamed
 * the step's target never mounting. It does mount. The card is rendered,
 * `visible`, `opacity: 1`, with working Skip/Next buttons — it is positioned
 * **off the top of the viewport**, measured on staging at `y = -238` for a card
 * 222px tall. Nothing about it is reachable, so the tour reads as vanished.
 *
 * The cause is placement, not lifecycle. Step 2 anchors to the Overview graph,
 * which is `h-svh` — a full-viewport-height element whose top edge sits at
 * `y = 0`. Asking for `placement: "top"` puts the card at `0 - height - offset`.
 * Neither of react-joyride's positioning middlewares can recover it:
 *
 * - `flip` (top → bottom) compares overflow both ways. Against a full-height
 *   anchor, *bottom* overflows by exactly as much as *top*, so floating-ui's
 *   best-fit tie-break keeps the original side.
 * - `shift` clamps only its main axis, which for a `top`/`bottom` placement is
 *   the **horizontal** one. The broken axis is unclamped.
 *
 * So this asserts the property the tour actually needs — every stop's card lies
 * fully inside the viewport — rather than the mechanism, which leaves the fix
 * free to change without the test rotting. Step 1 (a sidebar anchor) and the
 * later nav-row stops pass on both HEAD and the fix; step 2 is the one that
 * fails before it.
 *
 * Two window sizes because the bug is a function of the anchor's size relative
 * to the viewport: a stop that fits at one height can overflow at another.
 */

type Page = import("@playwright/test").Page;

/**
 * How many stops `src/tour/steps.ts` declares.
 *
 * Hard-coded because the e2e project has no `@/*` alias and cannot import app
 * modules (see `tsconfig.e2e.json`). It does not silently rot: the per-step
 * assertion below reads the card's own "Step N of M" chip and fails if M ever
 * stops matching, which is the signal to widen this constant rather than let a
 * new stop go uncovered.
 */
const TOUR_STOPS = 7;

/** Window sizes to walk the tour at. The first is the one measured on staging. */
const VIEWPORTS = [
  { width: 1512, height: 772 },
  { width: 1280, height: 800 },
] as const;

/**
 * Start a fresh tour from the welcome dialog.
 *
 * The suite's usual `oc-tour:` localStorage shim (which marks the tour skipped
 * so it stays out of other specs' way) is deliberately NOT installed here —
 * this spec is the one that wants the tour.
 */
async function startTour(page: Page): Promise<void> {
  await page.goto("/#/chat");
  const start = page.getByRole("button", { name: "Take the tour" });
  await expect(start, "first run should offer the tour").toBeVisible();
  await start.click();
}

for (const viewport of VIEWPORTS) {
  test(`every tour stop's card is inside a ${viewport.width}x${viewport.height} viewport`, async ({
    page,
  }) => {
    await page.setViewportSize({ ...viewport });
    await startTour(page);

    // `tooltipProps` gives the card `role="alertdialog"`. The welcome dialog is
    // a `dialog`, so this cannot match it — and by here it is closed anyway.
    const card = page.getByRole("alertdialog");

    for (let step = 1; step <= TOUR_STOPS; step += 1) {
      // Each stop switches the console's view first, so wait for THIS step's
      // chip rather than for any card: a stale card from the previous stop is
      // still mounted for a frame or two while the next view mounts.
      await expect(
        card.getByText(`Step ${step} of ${TOUR_STOPS}`),
        `step ${step} should render (and the tour should still have ${TOUR_STOPS} stops)`,
      ).toBeVisible();

      // Let placement settle: floating-ui positions on a later frame than the
      // one that mounts the card, and `autoUpdate` re-runs it when the target
      // finishes laying out. Measuring the first frame would flake in both
      // directions.
      await page.waitForTimeout(300);

      const box = await card.boundingBox();
      expect(box, `step ${step}'s card should have a box`).not.toBeNull();
      const { x, y, width, height } = box!;

      // The whole assertion. `toBeGreaterThanOrEqual(0)` on `y` is what fails on
      // HEAD for step 2 — measured `-238`.
      const where = `step ${step} at ${Math.round(x)},${Math.round(y)} ${Math.round(
        width,
      )}x${Math.round(height)} in ${viewport.width}x${viewport.height}`;
      expect(y, `${where}: card starts above the top edge`).toBeGreaterThanOrEqual(0);
      expect(x, `${where}: card starts left of the viewport`).toBeGreaterThanOrEqual(0);
      expect(y + height, `${where}: card runs past the bottom edge`).toBeLessThanOrEqual(
        viewport.height,
      );
      expect(x + width, `${where}: card runs past the right edge`).toBeLessThanOrEqual(
        viewport.width,
      );

      // And it is genuinely operable, not merely on-screen: a card the user
      // cannot advance is the same stall by another route. Playwright's
      // actionability check covers hit-testing, so this also catches a card
      // painted under the overlay.
      //
      // Matched on the rendered label rather than the accessible name: joyride
      // puts its own locale string on `primaryProps`' `aria-label` (`"Next"`,
      // and `"Last"` on the final stop), which wins over the text `TourTooltip`
      // renders. The label the operator reads is the one worth asserting.
      const advance = card.locator("button", { hasText: step === TOUR_STOPS ? "Done" : "Next" });
      await expect(advance, `step ${step}: the advance button should be usable`).toBeEnabled();
      await advance.click();
    }

    // The run finished rather than dead-ending on a stop.
    await expect(card).toHaveCount(0);
  });
}
