import { expect, test, type Page } from "@playwright/test";

import { openFirstWorkflow } from "./workflows";

/**
 * The console's shell: a sidebar column flush to the window's left edge, and
 * the page running edge to edge beside it, divided by one border.
 *
 * It was two layers for a while (issue #1178): chrome painted once on the shell
 * root, showing through as both the sidebar and a margin around one inset,
 * rounded card. That framing went with the floating sidebar — the page is the
 * whole window right of the column now, and the column says where it ends
 * with a border in the composer's style rather than with a band of chrome.
 *
 * Every geometric assertion below names a quantity rather than a class: an
 * edge at the window's, a border with a width, a page whose fill differs from
 * the sidebar's — because "the class is present" passes a layout that draws
 * nothing. Both themes, because the sidebar's tint and the page's fill are
 * separate tokens in each and a regression that flattens one theme is the
 * likely one.
 */

/** The first-run tour opens a modal over a fresh console and eats every click. */
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

/** Pin the theme before the app boots, so the first paint is the one under test. */
async function open(page: Page, theme: "dark" | "light", hash: string) {
  await page.addInitScript((value) => {
    window.localStorage.setItem("theme", value);
  }, theme);
  await page.goto(hash);
  await expect(page.locator('[data-testid="content-surface"]')).toBeVisible({ timeout: 30_000 });
  await expect(page.locator("html")).toHaveClass(new RegExp(`\\b${theme}\\b`));
}

/** What the shell actually lays out and paints, read off the live document. */
async function shell(page: Page) {
  return page.evaluate(() => {
    const q = (selector: string) => document.querySelector<HTMLElement>(selector);
    const box = (el: Element | null) => {
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
    };
    const column = q('[data-slot="sidebar-container"]');
    const sidebar = q('[data-slot="sidebar-inner"]');
    const page = q('[data-testid="content-surface"]');
    return {
      viewport: { width: window.innerWidth, height: window.innerHeight },
      column: box(column),
      page: box(page),
      borderRight: column ? Number.parseFloat(getComputedStyle(column).borderRightWidth) : null,
      sidebarColor: sidebar ? getComputedStyle(sidebar).backgroundColor : null,
      pageColor: page ? getComputedStyle(page).backgroundColor : null,
      pageRadius: page ? Number.parseFloat(getComputedStyle(page).borderTopLeftRadius) : null,
    };
  });
}

for (const theme of ["light", "dark"] as const) {
  test(`the sidebar is flush to the window and the page runs beside it (${theme})`, async ({ page }) => {
    await open(page, theme, "/#/settings");
    const s = await shell(page);

    // The column owns the window's left edge, top to bottom — no gutter, no
    // floating card.
    expect(s.column).not.toBeNull();
    expect(s.column!.left).toBe(0);
    expect(s.column!.top).toBe(0);
    expect(s.column!.bottom).toBeCloseTo(s.viewport.height, 0);

    // One border divides them; it is the seam, and there is exactly one.
    expect(s.borderRight).toBeGreaterThanOrEqual(1);

    // The page starts where the column ends and takes the rest of the window,
    // square-cornered: nothing frames it any more.
    expect(s.page).not.toBeNull();
    expect(s.page!.left).toBeCloseTo(s.column!.right, 0);
    expect(s.page!.top).toBe(0);
    expect(s.page!.right).toBeCloseTo(s.viewport.width, 0);
    expect(s.page!.bottom).toBeCloseTo(s.viewport.height, 0);
    expect(s.pageRadius).toBe(0);

    // And the two read as two surfaces: the sidebar's tint is not the page's.
    expect(s.pageColor).not.toBeNull();
    expect(s.sidebarColor).not.toBe(s.pageColor);
  });
}

test("the automation canvas fills the card and keeps its minimap inside it", async ({ page }) => {
  await open(page, "light", "/#/workflows");
  await openFirstWorkflow(page);

  // Issue #1683 opens the History rail on select. That rail is real,
  // in-flow layout — it is what this spec's crop-bug class (#1259/#1261) is
  // NOT about — so close it and measure the canvas against the bare card it
  // was written against.
  const historyToggle = page.getByTestId("workflow-history-toggle");
  if (await historyToggle.isVisible().catch(() => false)) {
    await historyToggle.click();
    await expect(page.getByTestId("workflow-run-history")).toBeHidden();
  }

  const canvas = await page.evaluate(() => {
    const card = document.querySelector('[data-testid="content-surface"]')!;
    const flow = document.querySelector(".react-flow");
    const mini = document.querySelector(".react-flow__minimap");
    const box = card.getBoundingClientRect();
    // The card's inner box — its hairline is part of the border box, so the
    // content it clips starts one pixel in on every side.
    const inner = {
      left: box.left + card.clientLeft,
      right: box.left + card.clientLeft + card.clientWidth,
      bottom: box.top + card.clientTop + card.clientHeight,
    };
    const rect = (el: Element | null) => {
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { left: r.left, right: r.right, bottom: r.bottom, width: r.width, height: r.height };
    };
    // Automations is a row on Company's rail now, so `#/workflows` holds the
    // 240px navigation column before the page — as `#/company/graph` does. Read
    // from the DOM, because below `lg` the rail is a chip row and the canvas
    // runs to the card's own edge.
    const rail = card.querySelector("nav[aria-label]");
    const railBox = rail && getComputedStyle(rail).display !== "none" ? rail.getBoundingClientRect() : null;
    return { inner, rail: railBox ? { right: railBox.right } : null, flow: rect(flow), mini: rect(mini) };
  });

  // React Flow computes its viewport transform and its minimap viewbox from the
  // container's measured rect, so the thing to prove is that the container it
  // measures is the card — not a box wider than the one it is clipped to. That
  // is the crop class of bug #1259 and #1261 were filed for.
  expect(canvas.flow).not.toBeNull();
  expect(canvas.flow!.left).toBeCloseTo(canvas.rail?.right ?? canvas.inner.left, 0);
  expect(canvas.flow!.right).toBeCloseTo(canvas.inner.right, 0);

  // And the minimap, pinned to the canvas's bottom-right, is inside the card
  // rather than under its rounded corner or past its edge.
  expect(canvas.mini).not.toBeNull();
  expect(canvas.mini!.right).toBeLessThanOrEqual(canvas.inner.right);
  expect(canvas.mini!.bottom).toBeLessThanOrEqual(canvas.inner.bottom);
  expect(canvas.mini!.width).toBeGreaterThan(0);
  expect(canvas.mini!.height).toBeGreaterThan(0);
});
