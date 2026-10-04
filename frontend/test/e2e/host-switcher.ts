import type { Locator, Page } from "@playwright/test";

/**
 * The host switcher, wherever it lives now.
 *
 * It used to head the window's title row on every page. The title row is gone,
 * and the switcher moved to the top of the Settings rail — where hosts are
 * managed — so a spec that reads it has to be on a Settings page first. The
 * move is a hash change, not a reload: the connection state the spec set up
 * (and any init script it registered) carries over exactly as it would for an
 * operator clicking Settings.
 */
export async function hostSwitcher(page: Page): Promise<Locator> {
  await page.evaluate(() => {
    if (!window.location.hash.startsWith("#/settings")) window.location.hash = "#/settings/general";
  });
  // `:visible`: Settings mounts the switcher twice — atop the rail at desktop
  // widths and above the page's chip row below `lg` — with CSS showing one.
  return page.locator('[data-testid="host-switcher"]:visible');
}
