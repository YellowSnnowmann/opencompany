import { expect, test } from "@playwright/test";
import { hostSwitcher } from "./host-switcher";

/**
 * Hosts on this computer, and how much of this file's original coverage is
 * reachable again.
 *
 * It covered the desktop's local-instance roster end to end, through the UI
 * that fronts the shell commands: a stopped instance listed and startable
 * rather than shown as a broken row, a host started here stopped again
 * without losing the others, a second company created on this computer, a
 * desktop-created company deleted after an explicit confirmation, and the
 * default instance refusing deletion because its root is the application
 * data directory itself.
 *
 * `HOSTS_HIDDEN` (`product-scope.ts`) is `false` again, which restores the
 * one entry point all of that coverage depends on — the switcher's "Add a
 * host" item — but not the rest: the local tab those deeper cases drive is
 * still desktop-only (`isDesktopRuntime()`), and this suite runs in an
 * ordinary browser context. So this file proves what an ordinary browser
 * console can prove — the entry point genuinely opens the screen now — and
 * leaves the local-tab cases to a desktop-scoped run, same as before.
 *
 * The Tauri commands underneath (`oc_local_instances`, `oc_create_local_instance`,
 * `oc_start_local_instance`, `oc_stop_local_instance`, `oc_delete_local_instance`)
 * are untouched and still unit-tested in Rust regardless.
 */

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

test("the host switcher's Add a host item opens the add-a-host screen", async ({ page }) => {
  await page.goto("/#/company");

  const trigger = await hostSwitcher(page);
  await expect(trigger).toBeVisible({ timeout: 30_000 });
  await trigger.click();

  await page.getByTestId("host-switcher-add").click();
  await expect(page.getByTestId("add-host")).toBeVisible();
});
