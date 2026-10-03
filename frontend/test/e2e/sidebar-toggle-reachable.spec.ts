import { expect, test } from "@playwright/test";

/**
 * However the sidebar is hidden, there is always a way back to it — and where
 * it cannot be hidden, nothing offers to.
 *
 * On mobile the sidebar is a sheet that closes entirely, taking its own
 * controls with it, so the way back is a button docked in its own chrome bar
 * above the page. On desktop it no longer collapses at all: it is a resizable
 * column flush to the window's left edge, so a collapse control would be a
 * button for a state that does not exist. The desktop half pins that absence.
 *
 * The mobile trigger used to be `position: fixed`, floating over whatever
 * content happened to scroll into the same corner and winning every hit-test
 * there (issue #1265). It is a normal-flow bar that reserves its own row
 * instead of overlaying one, which is what the overlap test below pins down.
 */

/** The sheet's Connections tab — an icon button, named by its label. */
const connectionsTab = (page: import("@playwright/test").Page) =>
  page.getByRole("dialog", { name: "Sidebar" }).getByRole("button", { name: "Connections", exact: true });

/** The tour can cover the fixed trigger while it is showing. */
async function dismissTour(page: import("@playwright/test").Page) {
  const skip = page.getByRole("button", { name: "Skip for now" });
  try {
    await skip.waitFor({ state: "visible", timeout: 10_000 });
  } catch {
    // The signed-in browser profile may already have completed the tour.
    return;
  }
  await skip.click();
  // The welcome dialog's backdrop is `fixed inset-0`, so it covers the WHOLE
  // viewport — not just the card it frames. Base UI runs a close animation
  // before unmounting it (`data-closed` + `data-ending-style`, `duration-100`),
  // and a click resolving does not wait for that: the backdrop is still in the
  // DOM, still hit-testable, for up to ~100ms after "Skip for now" is clicked.
  // A later `elementFromPoint` call anywhere on screen — including at a target
  // scrolled to the bottom of an unrelated page — can land on that fading
  // backdrop instead of the real content under it. Wait for the overlay itself
  // to detach, not just for the click to resolve.
  await expect(page.locator('[data-slot="dialog-overlay"]')).toHaveCount(0);
}

test.describe("sidebar toggle reachability", () => {
  test("the mobile sheet has an in-viewport way back", async ({ page }) => {
    await page.setViewportSize({ width: 700, height: 800 });
    await page.goto("/#/company");
    await dismissTour(page);

    const trigger = page.getByRole("button", { name: "Toggle sidebar" });
    await expect(trigger).toBeInViewport();
    await trigger.click();
    await expect(connectionsTab(page)).toBeVisible();
  });

  test("the sheet's trigger is the one sidebar control, and only below md", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/#/company");
    await dismissTour(page);

    // Nothing claims to collapse a column that is a sheet here.
    await expect(page.getByTestId("sidebar-collapse")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^(Collapse|Expand) sidebar$/ })).toHaveCount(0);

    // One control, and it is the one that means what it says.
    const trigger = page.getByRole("button", { name: "Toggle sidebar" });
    await expect(trigger).toHaveCount(1);
    await expect(trigger).toBeInViewport();
    await trigger.click();
    await expect(page.getByRole("dialog", { name: "Sidebar" })).toBeVisible();

    // At `md` (768, exactly where `useIsMobile` flips) the column is inline
    // and always open, so the trigger's bar goes with the sheet — `toBeHidden`,
    // since the gate is CSS (`md:hidden`).
    await page.setViewportSize({ width: 1024, height: 800 });
    await expect(trigger).toBeHidden();
  });

  test("the mobile sheet closes after selecting a destination", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/#/company");
    await dismissTour(page);

    await page.getByRole("button", { name: "Toggle sidebar" }).click();
    const sheet = page.getByRole("dialog", { name: "Sidebar" });
    await expect(sheet).toBeVisible();
    await expect(sheet).toHaveAttribute("aria-modal", "true");

    // A channel, not a section's child row. Since #2130 the sheet holds the four
    // sections and the Room rail — a section's own pages are a content rail, so
    // "Work" is not in here to pick any more. The channel list is the thing this
    // sheet now has that nothing else does, and it is the harder case: it is
    // portalled in from `RoomView` rather than rendered by the sidebar, so a
    // dismiss that only fired for the sidebar's own rows would miss it (which is
    // what `room-rail.tsx`'s `dismiss` exists for). Picking one still closes the
    // sheet behind it, which is the pattern under test.
    await sheet.getByRole("button", { name: "engineering-desk", exact: true }).click();
    await expect(page).toHaveURL(/#\/chat\//);
    await expect(sheet).toBeHidden();
  });

  test("Escape closes the mobile sheet after focus moves inside it", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/#/company");
    await dismissTour(page);

    await page.getByRole("button", { name: "Toggle sidebar" }).click();
    const sheet = page.getByRole("dialog", { name: "Sidebar" });
    const destination = sheet.getByRole("button", { name: "Company", exact: true });
    await destination.focus();
    await expect(destination).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(sheet).toBeHidden();
  });

  test("the mobile trigger does not overlap scrollable page content", async ({ page }) => {
    // The issue's own repro viewport (iPhone 12-class).
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/#/settings/general");
    await dismissTour(page);

    // Settings' General page is a single `flex-1 overflow-y-auto` column
    // (`SettingsView.tsx`) ending in a "Something off?" card — scrolling it to
    // the bottom is what used to land that card's button under the fixed
    // corner.
    const flagButton = page.getByRole("button", { name: "Flag something" });
    await flagButton.scrollIntoViewIfNeeded();

    const trigger = page.getByRole("button", { name: "Toggle sidebar" });
    await expect(trigger).toBeInViewport();

    const triggerBox = await trigger.boundingBox();
    const flagBox = await flagButton.boundingBox();
    expect(triggerBox, "the trigger should have a box").not.toBeNull();
    expect(flagBox, "the flag button should have a box").not.toBeNull();

    // No shared pixels in either axis: the trigger's row is reserved chrome,
    // not an overlay, so scrolled-to-the-end content and the trigger cannot
    // occupy the same screen space.
    const overlapsX = triggerBox!.x < flagBox!.x + flagBox!.width && flagBox!.x < triggerBox!.x + triggerBox!.width;
    const overlapsY = triggerBox!.y < flagBox!.y + flagBox!.height && flagBox!.y < triggerBox!.y + triggerBox!.height;
    expect(overlapsX && overlapsY, "the trigger and the scrolled-to content must not overlap").toBe(
      false,
    );

    // And the corner it used to cover hit-tests as the content now, not the
    // trigger — the concrete symptom from the issue's repro. Assert the hit
    // POSITIVELY resolves to the flag button, not just that it misses the
    // trigger: a hit-test landing on neither would satisfy the weaker check.
    const flagCenterX = flagBox!.x + flagBox!.width / 2;
    const flagCenterY = flagBox!.y + flagBox!.height / 2;
    const hit = await page.evaluate(
      ([x, y]) => {
        const el = document.elementFromPoint(x, y);
        return el instanceof Element ? (el.closest("button")?.textContent?.trim() ?? null) : null;
      },
      [flagCenterX, flagCenterY],
    );
    expect(hit, "the flag button's own point hits the flag button").toBe("Flag something");

    // Still reachable and still functional in its own right.
    await trigger.click();
    await expect(connectionsTab(page)).toBeVisible();
  });

  test("the inline sidebar has no collapse control and stays expanded", async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 800 });
    await page.goto("/#/company");
    await dismissTour(page);

    const sidebar = page.locator("[data-slot=sidebar]");
    await expect(sidebar).toHaveAttribute("data-state", "expanded");
    await expect(page.getByTestId("sidebar-collapse")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^(Collapse|Expand) sidebar$/ })).toHaveCount(0);

    // The shadcn shortcut (Cmd/Ctrl+B) is the other way a column collapses;
    // it must not reach a state no control could undo.
    await page.keyboard.press("ControlOrMeta+b");
    await expect(sidebar).toHaveAttribute("data-state", "expanded");

    // What it has instead of collapsing: a resize handle on its right edge.
    await expect(page.getByRole("separator", { name: "Resize sidebar" })).toBeVisible();
  });
});
