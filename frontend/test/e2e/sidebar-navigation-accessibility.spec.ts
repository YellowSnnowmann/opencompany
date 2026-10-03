import { expect, test } from "@playwright/test";

// The first-run tour is modal and correctly receives focus while it is open;
// skip it here so this spec can exercise the shell's ordinary tab order.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

test("the skip link reaches main content and the sidebar is the primary navigation", async ({
  page,
}) => {
  await page.goto("/#/company");

  const skip = page.getByRole("link", { name: "Skip to content", exact: true });
  const main = page.getByRole("main");

  // The console boots through a "Connecting…" phase that has no shell and so
  // no skip link; a Tab pressed against that phase moves focus nowhere. The
  // skip link exists only once the shell (and its sidebar) has mounted, so
  // waiting for it is the app-ready signal — and the sidebar's chrome renders
  // in the same commit, so nothing focusable appears between them.
  await skip.waitFor();

  // This is the first tab stop, ahead of the sidebar's conversations and its
  // tabs, even though the sidebar renders before main.
  await page.keyboard.press("Tab");
  await expect(skip).toBeFocused();
  await expect(skip).toBeVisible();

  // Hash routing owns `window.location.hash`; the skip link must focus main
  // without turning its conventional fragment into a route change.
  await page.keyboard.press("Enter");
  await expect(main).toBeFocused();
  await expect(main).toHaveAttribute("id", "main-content");
  await expect(page).toHaveURL(/#\/company$/);

  // The sidebar is the conversation list over one row of icon tabs: every
  // place an operator reaches from anywhere, as named glyphs in the foot's
  // "Sections" strip — Search, Company, Connections, Notifications, Settings
  // and Discord. Each is icon-only, so its accessible name is the whole of
  // what makes it a destination, and that is what this asserts.
  await expect(page.getByRole("navigation", { name: "Main navigation", exact: true })).toBeVisible();
  const sections = page.getByRole("navigation", { name: "Sections", exact: true });
  await expect(sections).toBeVisible();
  for (const name of ["Company", "Connections", "Settings"]) {
    await expect(sections.getByRole("button", { name, exact: true })).toBeVisible();
  }
  await expect(page.getByTestId("title-bar-search")).toBeVisible();
  await expect(page.getByTestId("title-bar-settings")).toBeVisible();
  await expect(page.getByTestId("title-bar-discord")).toBeVisible();

  // And nothing else is a destination there. Room is the conversation list
  // itself; Automations is a row on Company's rail; Overview was removed;
  // Observatory is filed under Settings (`settings-pages.ts`); Approvals is a
  // tab of the Notifications page; Feedback is a row on the Settings rail.
  const sidebar = page.locator("[data-slot=sidebar]");
  for (const name of ["Room", "Automations", "Overview", "Observatory", "Approvals", "Feedback"]) {
    await expect(sidebar.getByRole("button", { name, exact: true })).toHaveCount(0);
    await expect(sidebar.getByRole("link", { name, exact: true })).toHaveCount(0);
  }

  // The bell, which replaced the Approvals row. An icon-only control is a
  // destination only if it has an accessible name, which is this spec's whole
  // subject. The name is "Notifications" at rest and
  // "Notifications — N approvals need you" once something is waiting
  // (`notifications-button.tsx`), so it is matched on the destination's name
  // leading it rather than on a count this fixture does not fix.
  const bell = page.getByTestId("title-bar-notifications");
  await expect(bell).toBeVisible();
  await expect(bell).toHaveAccessibleName(/^Notifications/);
  await bell.click();
  await expect(page).toHaveURL(/#\/notifications$/);
  await expect(page.getByRole("heading", { name: "Notifications", level: 1 })).toBeVisible();
});

/**
 * At phone widths the sidebar is a sheet, and the page heads itself with the
 * one bar that opens it. Nothing else is in that bar, so nothing can be pushed
 * under the shell's `overflow-hidden`; the controls that used to crowd a title
 * row live in the sheet's foot, which is the sheet's own width. So the claims
 * are that the bar's trigger is on screen, the page grows no horizontal
 * scrollbar, every foot tab is wholly inside the viewport once the sheet is
 * open, and the desktop column comes back at its default 288px.
 */
for (const width of [480, 390]) {
  test(`the sidebar's controls stay inside a ${width}px viewport`, async ({ page }) => {
    await page.setViewportSize({ width, height: 800 });
    await page.goto("/#/company");

    const trigger = page.getByRole("button", { name: "Toggle sidebar" });
    await expect(trigger).toBeInViewport({ timeout: 30_000 });
    const scrollWidth = await page.evaluate(() => document.documentElement.scrollWidth);
    expect(scrollWidth, "the shell does not scroll horizontally").toBeLessThanOrEqual(width);

    await trigger.click();
    const sheet = page.getByRole("dialog", { name: "Sidebar" });
    await expect(sheet).toBeVisible();
    for (const id of ["title-bar-search", "title-bar-notifications", "title-bar-settings", "profile-row"]) {
      const box = await sheet.getByTestId(id).first().boundingBox();
      expect(box, `${id} should have a box`).not.toBeNull();
      expect(box!.x, `${id} starts inside the viewport`).toBeGreaterThanOrEqual(0);
      expect(Math.round(box!.x + box!.width), `${id} ends inside the viewport`).toBeLessThanOrEqual(width);
    }
    await page.keyboard.press("Escape");

    await page.setViewportSize({ width: 1280, height: 800 });
    const column = await page.locator('[data-slot="sidebar-container"]').boundingBox();
    expect(Math.round(column!.width), "the desktop sidebar column").toBe(288);
  });
}
