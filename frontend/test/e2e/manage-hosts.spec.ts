import { expect, test } from "@playwright/test";
import { hostSwitcher } from "./host-switcher";

/**
 * The "Manage hosts" page, and how much of this file's original coverage is
 * reachable again.
 *
 * It used to cover the wiring that page is: the menu item that opens it, the
 * roster it draws from context, renaming a host, re-addressing one that
 * moved, refusing an address with no scheme, refusing a move onto a host
 * this console already holds, and forgetting a host — including the property
 * underneath all of it, that a connection id is the namespace every
 * browser-local key hangs off (`scopedKey`), so "this host moved" must be
 * expressible without minting a new one.
 *
 * `HOSTS_HIDDEN` (`product-scope.ts`) is `false` again — the switcher's menu
 * opens on any host at all (`hostSwitcherMenu`'s own doc), and "Manage hosts"
 * is one of its two standing items — so the entry point this file used to
 * assert was gone is back. This is the one case restored so far: opening the
 * page. The deeper roster/rename/re-address/refuse/forget cases above are
 * restorable the same way and are a follow-up, not reconstructed here.
 */

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

test("the host switcher's Manage hosts item opens the manage-hosts page", async ({ page }) => {
  await page.goto("/#/company");

  const trigger = await hostSwitcher(page);
  await expect(trigger).toBeVisible({ timeout: 30_000 });
  await trigger.click();

  await page.getByTestId("host-switcher-manage").click();
  await expect(page.getByTestId("manage-hosts")).toBeVisible();
});
