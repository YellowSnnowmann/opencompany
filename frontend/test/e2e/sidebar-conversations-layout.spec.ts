import { expect, test, type Page } from "@playwright/test";

/**
 * The sidebar is the conversation list over one row of icon tabs and you,
 * running the window's full height against its left edge.
 *
 * Measured in a real browser, because the claims are about layout: the list
 * must scroll INSIDE itself (a flex item defaults to `min-height: auto`, so one
 * missing `min-h-0` and the rows push the foot off the bottom instead), the
 * foot must stay on screen however long the list is, and the column must be
 * flush to the window — not a card floating in a gutter, which it was for a
 * while.
 */

async function dismissTour(page: Page) {
  const skip = page.getByRole("button", { name: "Skip for now" });
  try {
    await skip.waitFor({ state: "visible", timeout: 10_000 });
    await skip.click();
  } catch {
    // The profile may already have completed the tour.
  }
  await expect(page.locator('[data-slot="dialog-overlay"]')).toHaveCount(0);
}

/** The foot's icon tabs, in the order they are drawn. */
const TABS = ["Company", "Connections", "Settings"];

test.describe("sidebar conversations layout", () => {
  test("keeps the foot's tabs in view when the list overflows", async ({ page }) => {
    // Short enough that the harness company's rows cannot fit above the foot.
    await page.setViewportSize({ width: 1280, height: 420 });
    await page.goto("/#/company");
    await dismissTour(page);

    const slot = page.getByTestId("room-rail-slot");
    await expect(slot.locator("li").first()).toBeVisible();
    const overflow = await slot.evaluate((el) => ({
      scroll: el.scrollHeight,
      client: el.clientHeight,
      overflowY: getComputedStyle(el).overflowY,
    }));
    expect(overflow.scroll, "the list overflows its slot").toBeGreaterThan(overflow.client);
    expect(overflow.overflowY).toBe("auto");

    const tabs = page.getByTestId("sidebar-shell-tabs");
    for (const name of TABS) {
      await expect(tabs.getByRole("button", { name, exact: true })).toBeInViewport({ ratio: 1 });
    }
    // One row, read left to right: Company before Connections.
    const company = (await page.locator('[data-tour="nav-company"]').boundingBox())!;
    const connections = (await page.locator('[data-tour="nav-connections"]').boundingBox())!;
    expect(Math.abs(company.y - connections.y)).toBeLessThanOrEqual(1);
    expect(company.x).toBeLessThan(connections.x);
  });

  test("runs the window's full height, flush to its left edge", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto("/#/company");
    await dismissTour(page);

    const column = (await page.locator("[data-slot=sidebar-container]").boundingBox())!;
    expect(column.x).toBe(0);
    expect(column.y).toBe(0);
    expect(column.y + column.height).toBeCloseTo(900, 0);
    // The foot sits on the window's bottom edge, not above a gutter.
    const foot = (await page.getByTestId("sidebar-shell-footer").boundingBox())!;
    expect(foot.y + foot.height).toBeCloseTo(900, 0);
  });

  test("renders no captions and no new-conversation doors", async ({ page }) => {
    // Channels are made on the org chart and agents on Company → Agents, so
    // the list carries neither a Conversations heading nor its + and pencil
    // menus — just the conversations.
    await page.goto("/#/company");
    await dismissTour(page);
    const sidebar = page.locator("[data-slot=sidebar]");
    await expect(sidebar.getByTestId("room-rail-slot").locator("li").first()).toBeVisible();
    for (const caption of ["Conversations", "Channels", "Direct messages"]) {
      await expect(sidebar.getByText(caption, { exact: true })).toHaveCount(0);
    }
    for (const door of ["New", "Start a conversation", "Room"]) {
      await expect(sidebar.getByRole("button", { name: door, exact: true })).toHaveCount(0);
    }
  });

  test("opens Automations from Company's rail with the Company tab lit", async ({ page }) => {
    await page.goto("/#/company");
    await dismissTour(page);
    await page.locator('[data-tour="nav-workflows"]').getByRole("button").first().click();
    await expect(page).toHaveURL(/#\/workflows/);
    await expect(page.locator('[data-tour="nav-company"]')).toHaveAttribute("aria-current", "page");
  });
});
