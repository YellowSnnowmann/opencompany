import { expect, test, type Locator, type Page } from "@playwright/test";

/**
 * Regression coverage for #1387.
 *
 * Base UI owns the mechanics of a modal: it hides the application behind the
 * portal and restores focus to the element that was active before opening.
 * The console's wrappers must expose that modality to assistive technology too.
 * This is a browser test because both the portal and the active element only
 * exist in a real document.
 */

async function dismissTour(page: Page) {
  const skip = page.getByRole("button", { name: "Skip for now" });
  try {
    await skip.waitFor({ state: "visible", timeout: 10_000 });
  } catch {
    return;
  }
  await skip.click();
  await expect(page.locator('[data-slot="dialog-overlay"]')).toHaveCount(0);
}

async function openTaskDialog(page: Page): Promise<Locator> {
  const addTask = page.getByRole("button", { name: "Add task" });
  await addTask.focus();
  await page.keyboard.press("Enter");

  const dialog = page.locator('[data-slot="dialog-content"]');
  await expect(dialog).toBeVisible();
  return dialog;
}

async function expectModal(dialog: Locator, backgroundControl: Locator) {
  await expect(dialog).toHaveAttribute("role", "dialog");
  await expect(dialog).toHaveAttribute("aria-modal", "true");
  // Base UI applies `aria-hidden` to the smallest outside subtree that can
  // hide the page without hiding the dialog's trigger. Assert the resulting
  // accessibility tree, not its private marker attribute or a fixed DOM shape.
  await expect(backgroundControl).toHaveCount(0);
}

test("the task dialog is modal and restores keyboard focus for every close path", async ({
  page,
}) => {
  await page.goto("/#/company/work/tasks");
  await dismissTour(page);

  const addTask = page.getByRole("button", { name: "Add task" });

  let dialog = await openTaskDialog(page);
  await expectModal(dialog, addTask);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(addTask).toBeFocused();

  dialog = await openTaskDialog(page);
  await expectModal(dialog, addTask);
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(addTask).toBeFocused();

  dialog = await openTaskDialog(page);
  await expectModal(dialog, addTask);
  await dialog.getByRole("button", { name: "Close" }).click();
  await expect(dialog).toHaveCount(0);
  await expect(addTask).toBeFocused();
});

test("the mobile sidebar sheet is modal", async ({ page }) => {
  await page.setViewportSize({ width: 700, height: 800 });
  await page.goto("/#/chat");
  await dismissTour(page);

  await page.getByRole("button", { name: "Toggle sidebar" }).click();

  const sheet = page.getByRole("dialog", { name: "Sidebar" });
  await expect(sheet).toBeVisible();
  await expectModal(sheet, page.getByRole("main"));
});

test("the mobile sidebar sheet returns focus to its toggle, not to an earlier control", async ({
  page,
}) => {
  // The sidebar's `Sheet` stays mounted while closed, so a return target
  // captured on first render would be whatever happened to be focused during
  // an earlier rerender — the tour's dismiss button, which by then no longer
  // exists — rather than the toggle the operator actually pressed.
  await page.setViewportSize({ width: 700, height: 800 });
  await page.goto("/#/chat");
  await dismissTour(page);

  const toggle = page.getByRole("button", { name: "Toggle sidebar" });
  await toggle.click();

  const sheet = page.getByRole("dialog", { name: "Sidebar" });
  await expect(sheet).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(sheet).toHaveCount(0);
  await expect(toggle).toBeFocused();
});
