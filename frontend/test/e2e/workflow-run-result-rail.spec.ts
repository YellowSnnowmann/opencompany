import { expect, test, type Page } from "@playwright/test";

import { openWorkflow } from "./workflows";

import { LIVE_BRAIN, LIVE_BRAIN_REASON } from "./capabilities";

/**
 * Issue #1205: the run-result drawer is a RIGHT RAIL beside the canvas on a
 * wide viewport, and the bottom strip it has always been below `xl` (1280px) —
 * the same fix issue #1107 made for run history on the left, mirrored.
 *
 * Pinned as geometry rather than as class names, for the same reason
 * `workflow-run-history-rail.spec.ts` is: `RunResultPanel`'s content stacks
 * vertically (a delivery block, a Steps list, one card per node), so a
 * horizontal strip shows very little of it while spending the canvas's
 * scarcest dimension.
 *
 * The third test is the one that keeps the two-rail arithmetic honest: with
 * BOTH the history rail (left) and the run result rail (right) open at once,
 * the canvas is squeezed from both sides — see `CanvasShell.tsx`'s header
 * comment for the arithmetic this pins.
 *
 * These specs need a real run (a run needs a brain to produce per-node text),
 * so they gate on `LIVE_BRAIN` like `workflow-run-result.spec.ts` does.
 */

/** Dismisses the first-run tour if it is up; tolerates its absence. */
async function dismissTour(page: Page) {
  const skip = page.getByRole("button", { name: "Skip for now" });
  try {
    await skip.waitFor({ state: "visible", timeout: 10_000 });
  } catch {
    return;
  }
  await skip.click();
  await expect(skip).toBeHidden();
}

/** The canvas ReactFlow paints into — the surface the rail is competing with. */
function canvas(page: Page) {
  return page.locator(".react-flow").first();
}

/** Runs the committed fixture workflow and waits for the result drawer. */
async function runAndAwaitResult(page: Page) {
  await page.getByRole("button", { name: "Run", exact: true }).click();
  const panel = page.getByTestId("workflow-run-result");
  await expect(panel).toBeVisible({ timeout: 60_000 });
  return panel;
}

test("at xl the run result is a right rail, and the canvas keeps a usable width", async ({
  page,
}) => {
  test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/#/workflows");
  await dismissTour(page);
  await openWorkflow(page, "Committed flow");

  // Issue #1683 opens the History rail on select. "Only one rail open" is the
  // scenario this case is about, so close it — the two-rail case is the third
  // test in this file.
  const historyToggle = page.getByTestId("workflow-history-toggle");
  if (await historyToggle.isVisible().catch(() => false)) {
    await historyToggle.click();
    await expect(page.getByTestId("workflow-run-history")).toBeHidden();
  }

  const flow = canvas(page);
  await expect(flow).toBeVisible();

  const panel = await runAndAwaitResult(page);
  const rail = (await panel.boundingBox())!;
  const graph = (await flow.boundingBox())!;

  // Over the canvas's right edge, not beside it: the rail floats, so it sits
  // inside the canvas box and costs it no width.
  expect(rail.x, "the rail floats inside the canvas").toBeGreaterThanOrEqual(graph.x);
  expect(
    rail.x + rail.width,
    "the rail's right edge is the canvas's, less its 12px inset",
  ).toBeLessThanOrEqual(graph.x + graph.width);
  // Full height of the canvas region, less its 12px inset top and bottom.
  expect(
    Math.abs(graph.height - rail.height - 24),
    "the rail runs the canvas's full height, less its inset",
  ).toBeLessThan(4);
  // Only one rail open here, so the floor matches the left rail's own case.
  expect(graph.width, "the canvas keeps a usable width beside the rail").toBeGreaterThan(
    640,
  );
});

test("below xl the run result falls back to a strip under the canvas", async ({
  page,
}) => {
  test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);

  await page.setViewportSize({ width: 1024, height: 800 });
  await page.goto("/#/workflows");
  await dismissTour(page);
  await openWorkflow(page, "Committed flow");

  const flow = canvas(page);
  await expect(flow).toBeVisible();

  const panel = await runAndAwaitResult(page);
  const strip = (await panel.boundingBox())!;
  const graph = (await flow.boundingBox())!;

  // Under the canvas, full width — same shape as the left rail's fallback.
  expect(strip.y, "the strip must sit below the canvas").toBeGreaterThanOrEqual(
    graph.y + graph.height - 1,
  );
  expect(
    Math.abs(strip.width - graph.width),
    "the strip spans the same width as the canvas",
  ).toBeLessThan(4);
});

test("both rails open at once: history left, run result right, canvas squeezed but usable", async ({
  page,
}) => {
  test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/#/workflows");
  await dismissTour(page);
  await openWorkflow(page, "Committed flow");

  const flow = canvas(page);
  await expect(flow).toBeVisible();

  // Issue #1683: index select already opens the History rail, so the toggle
  // needs waiting on (host-support check) but not clicking.
  const historyToggle = page.getByTestId("workflow-history-toggle");
  await historyToggle.waitFor({ state: "visible", timeout: 30_000 });
  const history = page.getByTestId("workflow-run-history");
  await expect(history).toBeVisible();

  const widthWithHistoryOnly = (await flow.boundingBox())!.width;
  const result = await runAndAwaitResult(page);

  const left = (await history.boundingBox())!;
  const graph = (await flow.boundingBox())!;
  const right = (await result.boundingBox())!;

  // Both rails float over the canvas, one on each edge, and neither costs it
  // any width: the canvas is as wide with both open as with history alone, and
  // above the same 640px floor the single-rail cases defend. (This asserted a
  // 500-650px band when the rails were in-flow and squeezed it.)
  expect(left.x, "history floats over the canvas's left edge").toBeGreaterThanOrEqual(graph.x);
  expect(
    right.x + right.width,
    "run result floats inside the canvas's right edge",
  ).toBeLessThanOrEqual(graph.x + graph.width);
  expect(
    left.x + left.width,
    "the two rails do not overlap each other",
  ).toBeLessThanOrEqual(right.x);
  expect(graph.width, "the canvas keeps a usable width with both rails open").toBeGreaterThan(640);
  expect(graph.width, "opening the result rail takes no width from the canvas").toBeCloseTo(
    widthWithHistoryOnly,
    0,
  );
});
