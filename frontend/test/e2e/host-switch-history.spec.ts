import { expect, test, type Page } from "@playwright/test";

import { openHostMenu } from "./host-switcher";

/**
 * Switching hosts is a navigation, and the address says whose page it is
 * (issue #1358).
 *
 * ## The report
 *
 * Two hosts, the second one down. On the Tasks board of the working host, pick
 * the dead one from the switcher: a full-screen "Can't connect" comes up and
 * the address bar goes on reading `#/ledgers/tasks` — a page of the host just
 * left. Press Back to undo the switch and **nothing appears to happen**: the
 * failed host's console does not read the route, so no pixel changes. Press it
 * again, same. Then switch back to the working host and land on Overview,
 * because those two inert-looking presses were popping *its* history.
 *
 * Two defects with one symptom, and the sequence below is the only way to see
 * either of them:
 *
 *   1. selection was plain React state, so Back could not undo a switch;
 *   2. the hash named a page without naming a host, so Back on the failed
 *      host's screen spent the working host's route stack instead.
 *
 * Together they made the natural recovery gesture the thing that destroyed
 * your place, silently.
 *
 * ## Why this is an e2e spec
 *
 * `test/unit/host-route.test.ts` pins the scope helpers and the hooks against a
 * synthetic hash. It cannot reach the thing that was actually broken: a real
 * `history` stack, a real Back, and a console whose shell is not mounted for
 * the host on screen. The bug lives exactly in that gap — every piece worked
 * on its own.
 *
 * The dead host is an address nothing listens on, for the reason
 * `connection-degrade.spec.ts` gives: a spec that needs two live servers is a
 * spec nobody runs.
 */

/** A port nothing is listening on, so the second connection is always down. */
const DEAD_HOST = "http://127.0.0.1:8451";

/** The tour modal covers the board and swallows clicks. */
async function silenceTour(page: Page) {
  await page.addInitScript(() => {
    for (const key of ["oc-tour:single", "oc-tour:e2e-harness-co", "oc-tour:null"]) {
      window.localStorage.setItem(key, JSON.stringify({ skipped: true, seenAt: Date.now() }));
    }
  });
}

/** Seeds a working host and an unreachable one, the way the app persists them. */
async function seedTwoHosts(page: Page) {
  await silenceTour(page);
  await page.addInitScript((dead) => {
    window.localStorage.setItem(
      "oc.connections.v1",
      JSON.stringify([
        {
          id: "conn-primary",
          baseUrl: "",
          label: "Primary",
          defaultCompany: null,
          credential: { kind: "cookie" },
        },
        {
          id: "conn-dead",
          baseUrl: dead,
          label: "Offline host",
          defaultCompany: null,
          credential: { kind: "cookie" },
        },
      ]),
    );
  }, DEAD_HOST);
}

/** The hash, which is where the whole of this issue is visible. */
function hash(page: Page): string {
  return new URL(page.url()).hash;
}

test("Back undoes a host switch instead of silently spending the other host's route", async ({
  page,
}) => {
  await seedTwoHosts(page);
  await page.goto("/#/ledgers/tasks");

  // The board of the working host, and an address that says so. Before this
  // fix the `?host=` half simply did not exist.
  await expect(page.getByRole("button", { name: "Add task" })).toHaveCount(1, {
    timeout: 30_000,
  });
  await expect(page).toHaveURL(/#\/ledgers\/tasks\?host=conn-primary$/);

  // Step 2 of the report: pick the host that is down.
  await openHostMenu(page);
  await page.getByTestId("host-row-conn-dead").click();
  await expect(page.getByTestId("connection-error")).toBeVisible({ timeout: 30_000 });

  // Defect 2. The bar named `conn-primary`'s Tasks board for the whole time
  // this failure was on screen; now it names the host the failure belongs to,
  // so the URL can be copied and reloaded onto what is actually being looked
  // at. The path is *frozen*, not rewritten — going back to a working host
  // returns to the page that was open, which was already correct and must stay
  // so.
  expect(hash(page)).toBe("#/ledgers/tasks?host=conn-dead");

  // Steps 3–4, and defect 1. One Back, and the working host's Tasks board is
  // back — the switch is undone. Before this, Back consumed
  // `#/ledgers/tasks` → `#/ledgers` while the screen stayed on "Can't connect",
  // and it took a second press and a manual switch to discover the damage.
  await page.goBack();
  await expect(page.getByRole("button", { name: "Add task" })).toHaveCount(1, {
    timeout: 30_000,
  });
  await expect(page.getByTestId("connection-error")).toHaveCount(0);
  expect(hash(page)).toBe("#/ledgers/tasks?host=conn-primary");
});

test("the working host's own route stack is intact after a trip through the failure", async ({
  page,
}) => {
  // The damage the operator could not see. Two views deep on the working host,
  // then out to the dead one and back: the entries behind them must still be
  // the ones they walked in on, not two shallower.
  await seedTwoHosts(page);
  await page.goto("/#/overview");
  await expect(page.getByTestId("host-switcher")).toBeVisible({ timeout: 30_000 });

  await page.evaluate(() => {
    window.location.hash = "/ledgers/tasks";
  });
  await expect(page.getByRole("button", { name: "Add task" })).toHaveCount(1, {
    timeout: 30_000,
  });
  await expect(page).toHaveURL(/#\/ledgers\/tasks\?host=conn-primary$/);

  await openHostMenu(page);
  await page.getByTestId("host-row-conn-dead").click();
  await expect(page.getByTestId("connection-error")).toBeVisible({ timeout: 30_000 });

  // Back out of the failure, then back through the page that was open before
  // it. Each press moves one step, and the steps are the working host's —
  // which is the whole of "nothing was spent while the error screen was up".
  // Before this, the first press was silently the second: it popped
  // `#/ledgers/tasks` while the error screen stayed put, so this Overview was
  // one press closer than the operator believed.
  await page.goBack();
  expect(hash(page)).toBe("#/ledgers/tasks?host=conn-primary");
  await page.goBack();
  await expect(page).toHaveURL(/#\/overview\?host=conn-primary$/);
});

test("a copied address reopens the host it named, failure and all", async ({ page }) => {
  // The other half of defect 2. Reloading the failure screen used to drop back
  // to the bootstrap connection, so "Retry" on a non-bootstrap host quietly
  // moved you to a different one — the address carried no host to return to.
  await seedTwoHosts(page);
  await page.goto("/#/ledgers/tasks?host=conn-dead");
  await expect(page.getByTestId("connection-error")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole("button", { name: "Add task" })).toHaveCount(0);
});

test("a host that cannot be reached can be forgotten from the failure itself", async ({
  page,
}) => {
  // The adjacent half of the issue. This screen used to offer Retry and the
  // single-host boot hint — telling somebody who picked a row out of a switcher
  // to "set the host with ?api=" — and no way to dispose of a host that is
  // simply gone.
  await seedTwoHosts(page);
  await page.goto("/#/ledgers/tasks?host=conn-dead");
  await expect(page.getByTestId("connection-error")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId("connection-error")).not.toContainText("?api=");

  await page.getByTestId("connection-error-forget").click();

  // Forgetting selects what is left, and the console it lands on is a working
  // one. The switcher drops to a single host, so the address stops carrying a
  // scope there is no longer anything to disambiguate.
  await expect(page.getByRole("button", { name: "Add task" })).toHaveCount(1, {
    timeout: 30_000,
  });
  await expect(page.getByTestId("host-switcher")).toHaveAttribute("data-host-count", "1");
});
