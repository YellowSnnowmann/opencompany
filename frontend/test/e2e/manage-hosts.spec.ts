import { expect, test, type Page } from "@playwright/test";

import { openHostMenu } from "./host-switcher";

/**
 * Modifying a host, driven the way a person does it.
 *
 * The unit tests drive `editConnection` directly — what a move does to the id,
 * what is dropped when an address changes, which connectors may be re-typed.
 * What they cannot cover is the wiring this page is: the menu item that opens
 * it, the roster it draws from context, and the fact that forgetting the host
 * on screen has to move the selection *and* leave the page standing.
 *
 * The property underneath all of it: a connection id is the namespace every
 * browser-local key hangs off (`scopedKey`), so "this host moved" must be
 * expressible without minting a new one.
 */

/** A dead address, as in `add-host-connectors.spec.ts`. Port 9 is `discard`. */
const ASLEEP = "http://127.0.0.1:9";

/**
 * A second dead address, for the move to land on.
 *
 * Deliberately *not* the console's own origin: the primary row holds that one
 * (as the empty string), so moving here would be moving onto a host this
 * console already has — which the page refuses, and which the test below
 * asserts it refuses. Port 7 is `echo`; nothing is listening on either.
 */
const MOVED_TO = "http://127.0.0.1:7";

/** Seeds a same-origin host and a second one at an address nothing answers. */
async function seedTwoHosts(page: Page): Promise<void> {
  await page.addInitScript(
    (dead: string) => {
      // The tour modal covers the console and swallows every click, including
      // the one that opens the switcher.
      for (const key of ["oc-tour:single", "oc-tour:e2e-harness-co", "oc-tour:null"]) {
        window.localStorage.setItem(key, JSON.stringify({ skipped: true, seenAt: Date.now() }));
      }
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
            id: "conn-second",
            baseUrl: dead,
            label: "Second",
            defaultCompany: null,
            credential: { kind: "cookie" },
            connector: { kind: "remote" },
          },
        ]),
      );
    },
    ASLEEP,
  );
}

const switcher = (page: Page) => page.getByTestId("host-switcher");

/** What the console has written down about the hosts it holds. */
async function storedProfiles(page: Page): Promise<{ id: string; label: string; baseUrl: string }[]> {
  return page.evaluate(() => {
    const raw = window.localStorage.getItem("oc.connections.v1");
    return raw ? (JSON.parse(raw) as { id: string; label: string; baseUrl: string }[]) : [];
  });
}

/** Opens the switcher's "Manage hosts" and waits for the page. */
async function openTheManagePage(page: Page): Promise<void> {
  await openHostMenu(page);
  await page.getByTestId("host-switcher-manage").click();
  await expect(page.getByTestId("manage-hosts")).toBeVisible();
}

test("every connected host is listed with what can be done to it", async ({ page }) => {
  await seedTwoHosts(page);
  await page.goto("/");
  await expect(switcher(page)).toHaveAttribute("data-host-count", "2", { timeout: 30_000 });

  await openTheManagePage(page);

  await expect(page.getByTestId("manage-host-conn-primary")).toBeVisible();
  const second = page.getByTestId("manage-host-conn-second");
  await expect(second).toBeVisible();
  // The address is on the row, because the reason someone opened this page is
  // usually that one of them is wrong — and the menu never shows it.
  await expect(second).toContainText(ASLEEP);
});

test("renaming a host keeps everything the console remembers about it", async ({ page }) => {
  // The whole reason this is an edit rather than "forget it and add it again":
  // a fresh connection id orphans the tour progress, the last-read channel and
  // the drafts under the old one, silently.
  await seedTwoHosts(page);
  await page.goto("/");
  await expect(switcher(page)).toHaveAttribute("data-host-count", "2", { timeout: 30_000 });

  await openTheManagePage(page);
  await page.getByTestId("manage-host-edit-conn-second").click();
  await page.getByLabel("Name").fill("Acme staging");
  await page.getByTestId("manage-host-save-conn-second").click();

  await expect(page.getByTestId("manage-host-conn-second")).toContainText("Acme staging");

  const profiles = await storedProfiles(page);
  expect(profiles.find((p) => p.id === "conn-second")?.label).toBe("Acme staging");
  // Two hosts still, under the ids they had.
  expect(profiles.map((p) => p.id).sort()).toEqual(["conn-primary", "conn-second"]);
});

test("a host that moved is re-addressed under the id it already had", async ({ page }) => {
  await seedTwoHosts(page);
  await page.goto("/");
  await expect(switcher(page)).toHaveAttribute("data-host-count", "2", { timeout: 30_000 });

  await openTheManagePage(page);
  await page.getByTestId("manage-host-edit-conn-second").click();
  await page.getByLabel("Address").fill(MOVED_TO);
  await page.getByTestId("manage-host-save-conn-second").click();

  await expect
    .poll(async () => (await storedProfiles(page)).find((p) => p.id === "conn-second")?.baseUrl, {
      timeout: 30_000,
    })
    .toBe(MOVED_TO);
  // Still two rows: a move is not an add.
  await expect(switcher(page)).toHaveAttribute("data-host-count", "2");
});

test("moving a host onto one this console already holds is refused", async ({ page, baseURL }) => {
  // The primary row *is* this origin, stored as the empty string — so its
  // explicit url is the same host under a different spelling. Saved, it would
  // be two connection ids for one host, with every browser-local key split
  // between them. `canonicalAddress` is what makes the two spellings compare
  // equal; this is the assertion that they do.
  await seedTwoHosts(page);
  await page.goto("/");
  await expect(switcher(page)).toHaveAttribute("data-host-count", "2", { timeout: 30_000 });

  await openTheManagePage(page);
  await page.getByTestId("manage-host-edit-conn-second").click();
  await page.getByLabel("Address").fill(baseURL ?? "");

  await expect(page.getByTestId("manage-host-taken-conn-second")).toBeVisible();
  await expect(page.getByTestId("manage-host-save-conn-second")).toBeDisabled();
});

test("an address with no scheme is refused rather than saved", async ({ page }) => {
  // Saved as typed it would resolve against the console's own origin, and the
  // row would go down with an error naming a url nobody entered.
  await seedTwoHosts(page);
  await page.goto("/");
  await expect(switcher(page)).toHaveAttribute("data-host-count", "2", { timeout: 30_000 });

  await openTheManagePage(page);
  await page.getByTestId("manage-host-edit-conn-second").click();
  await page.getByLabel("Address").fill("acme.example.com");

  await expect(page.getByTestId("manage-host-bad-url-conn-second")).toBeVisible();
  await expect(page.getByTestId("manage-host-save-conn-second")).toBeDisabled();
});

test("forgetting a host drops it from the roster and from storage", async ({ page }) => {
  await seedTwoHosts(page);
  await page.goto("/");
  await expect(switcher(page)).toHaveAttribute("data-host-count", "2", { timeout: 30_000 });

  await openTheManagePage(page);
  await page.getByTestId("manage-host-forget-conn-second").click();
  await page.getByTestId("manage-host-forget-confirm-conn-second").click();

  await expect(page.getByTestId("manage-host-conn-second")).toHaveCount(0);
  await expect(switcher(page)).toHaveAttribute("data-host-count", "1");
  expect((await storedProfiles(page)).map((p) => p.id)).toEqual(["conn-primary"]);
});
