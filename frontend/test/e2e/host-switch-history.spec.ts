import { expect, test } from "@playwright/test";
import { hostSwitcher } from "./host-switcher";

/**
 * Switching hosts is a navigation, and how that reads with exactly one host.
 *
 * It covered issue #1358: with two hosts and the second one down, picking the
 * dead host from the switcher pushed a history entry, so Back undid the switch
 * instead of silently spending the working host's route stack; a copied
 * address reopened the host it named, failure and all; and a host that could
 * not be reached could be forgotten from the failure screen itself.
 *
 * `HOSTS_HIDDEN` (`product-scope.ts`) is `false` again — the switcher's menu
 * opens on any host at all (`hostSwitcherMenu`'s own doc: "any host at all
 * opens a menu... a menu of one is not furniture once one is a menu of
 * something to *do*") — so this file is no longer about an inert trigger. The
 * two-host history cases above are restorable; this one case is what a single
 * connection — the shared E2E company's actual shape — still proves: the
 * menu names the host you are already on rather than offering nothing at all,
 * and selecting the row you are already on is a no-op, not a switch to undo.
 */

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

test("with one host, its own row is already the selection, so opening the menu changes nothing", async ({
  page,
}) => {
  await page.goto("/#/settings/general");

  const trigger = await hostSwitcher(page);
  await expect(trigger).toBeVisible({ timeout: 30_000 });
  await trigger.click();

  // One host, one row, and it already names where the operator is — not a
  // roster with nothing to distinguish, and not the inert nameplate the
  // hidden-roster era left this file asserting.
  const rows = page.locator('[data-testid^="host-row-"]');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toHaveAttribute("aria-current", "true");

  // Selecting the row you are already on is a no-op: no navigation follows,
  // so there is nothing for Back to undo.
  await rows.first().click();
  await expect(page).toHaveURL(/#\/settings\/general/);
});
