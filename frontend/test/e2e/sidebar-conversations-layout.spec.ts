import { expect, test, type Page } from "@playwright/test";

/**
 * The sidebar is the conversation list on top and Company / Connections pinned
 * at the foot, and the foot lines up with the console card.
 *
 * Measured in a real browser, because both claims are about layout: the list
 * must scroll INSIDE itself (a flex item defaults to `min-height: auto`, so one
 * missing `min-h-0` and the rows are pushed off the bottom instead), and the
 * last row's bottom edge must be the card's bottom edge (the shell's
 * `--frame-inset` gap, reused rather than a second number).
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

test.describe("sidebar conversations layout", () => {
  test("keeps Company and Connections in view when the list overflows", async ({ page }) => {
    // Short enough that the harness company's rows cannot fit above the pair.
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

    const viewport = page.viewportSize()!;
    for (const name of ["Company", "Connections"]) {
      const row = page.locator(`[data-tour="nav-${name.toLowerCase()}"]`);
      await expect(row).toBeInViewport({ ratio: 1 });
      const box = (await row.boundingBox())!;
      expect(box.y + box.height, `${name} is above the window's foot`).toBeLessThanOrEqual(
        viewport.height,
      );
    }
    // Company first, Connections last: the order the operator reads them in.
    const company = (await page.locator('[data-tour="nav-company"]').boundingBox())!;
    const connections = (await page.locator('[data-tour="nav-connections"]').boundingBox())!;
    expect(company.y).toBeLessThan(connections.y);
  });

  test("lands Connections' bottom edge on the console card's bottom edge", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto("/#/company");
    await dismissTour(page);

    const edges = await page.evaluate(() => {
      const bottom = (el: Element | null) => el!.getBoundingClientRect().bottom;
      const card = document.querySelector("[data-slot=sidebar-inset]")!.firstElementChild;
      const button = document.querySelector('[data-tour="nav-connections"] button');
      return { card: bottom(card), connections: bottom(button) };
    });
    expect(edges.connections).toBeCloseTo(edges.card, 0);
  });

  test("renders no Room or Automations row and no Channels / Direct messages caption", async ({
    page,
  }) => {
    await page.goto("/#/company");
    await dismissTour(page);
    const nav = page.getByRole("navigation", { name: "Main navigation", exact: true });
    await expect(nav.getByText("Conversations", { exact: true })).toBeVisible();
    await expect(nav.getByText("Channels", { exact: true })).toHaveCount(0);
    await expect(nav.getByText("Direct messages", { exact: true })).toHaveCount(0);
    for (const name of ["Room"]) {
      await expect(nav.getByRole("button", { name, exact: true })).toHaveCount(0);
    }
  });

  test("opens Automations from Company's rail with Company lit in the sidebar", async ({
    page,
  }) => {
    await page.goto("/#/company");
    await dismissTour(page);
    await page.locator('[data-tour="nav-workflows"]').getByRole("button").first().click();
    await expect(page).toHaveURL(/#\/workflows/);
    await expect(
      page.locator('[data-tour="nav-company"] [data-sidebar=menu-button]'),
    ).toHaveAttribute("data-active", "");
  });
});
