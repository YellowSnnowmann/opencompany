import { expect, test } from "@playwright/test";

/**
 * The host switcher, on the Settings rail, on a console holding two hosts.
 *
 * This spec used to guard the opposite arrangement: an icon rail down the left
 * edge, and a `position: fixed` sidebar that had to be offset by 56px or it
 * slid underneath and clipped every nav label — "Company" read as "mpany".
 * Issue #1142 removed the rail and moved the choice into the sidebar's own
 * header, so the offset is gone and the assertion inverts: the sidebar now
 * starts at the left edge of the window, and nothing stands in front of it.
 * The switcher has since moved again, out of the sidebar (which lost its
 * header) to the top of the Settings rail, beside the pages that manage hosts.
 *
 * What carries over is why the spec exists at all. The broken state needed two
 * connections, and nothing else in the suite creates them — a design-system
 * migration and two contrast audits all passed straight over it, because every
 * one of them ran against the default single-host console. `HOSTS_HIDDEN`
 * (`product-scope.ts`) is `false`, so even one host now opens the switcher's
 * menu; two hosts is what still lets this spec see the menu carry more than
 * one row and a "second host" to render as unreachable — the shape a
 * single-host console cannot exercise.
 */

// The first-run tour opens a dialog over the console and `aria-hidden`s
// everything beneath it. Same shim the rest of the suite uses.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

/** Must match `companies/e2e_harness/company.toml`'s `[users] admins`. */
const ADMIN_EMAIL = "harness-e2e@tinyhumans.ai";

test("the sidebar owns the left edge, and Settings carries the host switcher", async ({
  page,
  baseURL,
}) => {
  const base = (baseURL ?? "http://127.0.0.1:8080").replace(/\/$/, "");

  // Signs in through the page's OWN context, so the session cookie lands in
  // the jar the page uses. Deliberately not relying on the shared
  // `storageState`: this spec seeds `oc.connections.v1` before boot, and a
  // spec that rewrites the connection list should not also depend on a
  // session minted for a different connection list.
  const api = page.context().request;
  const requested = await api.post(`${base}/api/v1/company/auth/request`, {
    data: { email: ADMIN_EMAIL },
  });
  const { dev_code: devCode } = (await requested.json()) as { dev_code?: string };
  expect(devCode, "the host should echo a dev_code when no mail is configured").toBeTruthy();
  const verified = await api.post(`${base}/api/v1/company/auth/verify`, {
    data: { email: ADMIN_EMAIL, code: devCode },
  });
  expect(verified.ok(), "sign-in should mint a session").toBe(true);
  // Two hosts, so the switcher opens a menu. The second address is never
  // contacted — the menu draws it as unreachable, which is all this spec needs.
  await page.addInitScript((primary: string) => {
    localStorage.setItem(
      "oc.connections.v1",
      JSON.stringify([
        {
          id: "switcher-spec-primary",
          baseUrl: primary,
          label: "Primary",
          defaultCompany: null,
          credential: { kind: "cookie" },
        },
        {
          id: "switcher-spec-second",
          baseUrl: "http://127.0.0.1:9",
          label: "Second host",
          defaultCompany: null,
          credential: { kind: "cookie" },
        },
      ]),
    );
  }, base);

  await page.goto("/#/settings/general");

  // Nothing stands to the left of the sidebar any more.
  await expect(page.getByTestId("connection-rail")).toHaveCount(0);

  const sidebar = page.locator("[data-slot=sidebar-container]");
  await expect(sidebar).toBeVisible();
  const sidebarBox = await sidebar.boundingBox();
  expect(sidebarBox, "sidebar should have a box").not.toBeNull();
  expect(sidebarBox!.x, "the sidebar starts at the left edge of the window").toBe(0);

  // And the labels are actually on screen rather than clipped — the symptom a
  // reader would report, asserted separately from the cause so a future change
  // that moves the sidebar for some other reason still fails here rather than
  // passing on a technicality.
  // The section tabs are buttons, not links — the shell routes on the hash
  // rather than navigating.
  const label = page.getByRole("button", { name: "Company", exact: true });
  const labelBox = await label.boundingBox();
  expect(labelBox, "the Company tab should have a box").not.toBeNull();
  expect(labelBox!.x).toBeGreaterThanOrEqual(0);

  // The switcher heads the Settings rail, right of the sidebar rather than a
  // column of its own to the left of it, and it holds both hosts.
  const switcher = page.getByTestId("host-switcher");
  await expect(switcher).toBeVisible();
  const switcherBox = await switcher.boundingBox();
  expect(switcherBox, "the switcher should have a box").not.toBeNull();
  expect(
    switcherBox!.x,
    "the switcher is on the Settings rail, past the sidebar's right edge",
  ).toBeGreaterThanOrEqual(sidebarBox!.x + sidebarBox!.width);

  // And it is a real control, not a nameplate: `HOSTS_HIDDEN`
  // (`product-scope.ts`) is `false`, and two hosts are seeded above, so this
  // is `hostSwitcherMenu`'s own `count >= 1` case — a genuine menu, carrying
  // both hosts' rows plus "Add a host" and "Manage hosts".
  //
  // Asserted after a click AND a keyboard Enter, because either has to reach
  // it — a trigger that opens under a pointer but not the keyboard is broken
  // in a way a click-only assertion would miss.
  await switcher.click();
  await expect(page.locator('[role="menu"]')).toBeVisible();
  await expect(page.getByTestId("host-row-switcher-spec-primary")).toBeVisible();
  await expect(page.getByTestId("host-row-switcher-spec-second")).toBeVisible();
  await expect(page.getByTestId("host-switcher-add")).toBeVisible();
  await expect(page.getByTestId("host-switcher-manage")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator('[role="menu"]')).toHaveCount(0);

  await switcher.focus();
  await page.keyboard.press("Enter");
  await expect(page.locator('[role="menu"]')).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator('[role="menu"]')).toHaveCount(0);

  // It still names the company, and still reports the roster's health,
  // whether or not the menu is open — the two things the trigger carried all
  // along, and still true now that it also opens something.
  // The count includes the browser's own same-origin bootstrap alongside the
  // two seeded here, so this asserts the signal survives rather than an
  // exact total.
  await expect(switcher).toHaveAttribute("data-host-count", /\d+/);
  await expect(switcher).toHaveAttribute("data-worst-status", /\w+/);
});
