import { expect, test } from "@playwright/test";

/**
 * Issue #1401 — Settings must not offer a lifecycle control it cannot reach.
 *
 * `Settings → General → Lifecycle` used to render four buttons. Two of them,
 * `Suspend` and `Archive`, are `PlatformScope` routes: that extractor resolves
 * through `resolve_claims`, which cannot return a human, so a session cookie
 * can never reach them whatever it contains. The console holds platform scope
 * only when it was handed a platform bearer through `?token=`, which is not how
 * an operator signing in with a magic link arrives.
 *
 * So `Archive` — styled `destructive`, behind a dialog calling itself permanent
 * — took the operator's confirmation and answered `Couldn't archive —
 * unauthorized`. That is the failure this console is otherwise careful about:
 * Billing and Hosting both explain, in the page, when a control cannot work on
 * this deployment.
 *
 * This spec is deliberately two halves that have to agree. Asserting only that
 * the buttons are gone would pass just as well if somebody deleted them for the
 * wrong reason, and would keep passing if the routes were later opened up to
 * company admins — at which point hiding them becomes the bug. So the second
 * half asks the host directly, with this browser's own session, and pins the
 * `401` the missing buttons are standing in for.
 */

/** The first-run tour opens a modal over a fresh console and intercepts clicks. */
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

test("the lifecycle card offers pause, withholds suspend and archive, and says why", async ({
  page,
}) => {
  await page.goto("/#/settings");

  // The card is there and operable — the fix must not have emptied it. `Pause`
  // is a `CompanyAuth` route and is the operator's real, reversible stop.
  await expect(page.getByRole("button", { name: "Pause", exact: true })).toBeVisible({
    timeout: 30_000,
  });

  // Absent, not disabled. A disabled `Archive` would still assert that
  // archiving is something this console does, and an operator would go looking
  // for the permission that enables it. There is none.
  await expect(page.getByRole("button", { name: "Suspend", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Archive", exact: true })).toHaveCount(0);

  // And the omission explains itself, in the page, the way Billing and Hosting
  // do — not as a toast after a click that failed.
  const banner = page.getByTestId("lifecycle-platform-only");
  await expect(banner).toBeVisible();
  await expect(banner).toContainText("platform");
});

test("the host really does refuse suspend and archive to this session", async ({
  request,
}) => {
  // The company this session can see, rather than the harness id hard-coded.
  // `GET /api/v1/companies` is `CompanyAuth` and filtered by `visible_companies`,
  // so it answers a signed-in person with exactly the companies they operate —
  // unlike `/api/v1/company`, which has no status alias and 404s with an empty
  // body that reads as a JSON parse error rather than as a wrong URL.
  const list = await request.get("/api/v1/companies");
  expect(list.ok(), `listing companies failed: ${list.status()}`).toBeTruthy();
  const companies = (await list.json()) as { id: string; lifecycle: string }[];
  const company = companies[0];
  expect(company?.id, "the harness host must serve a company").toBeTruthy();

  // The reason the buttons are gone. `request` carries the same signed-in
  // storage state the page does, so these are the exact calls the Archive
  // button used to make *after* taking an operator's confirmation.
  for (const action of ["suspend", "archive"]) {
    const res = await request.post(`/api/v1/companies/${company.id}/${action}`);
    expect(res.status(), `${action} must refuse a human session`).toBe(401);
  }

  // And the refusal really was a refusal: nothing above may have archived the
  // company the specs after this one drive.
  const after = (await (await request.get("/api/v1/companies")).json()) as {
    id: string;
    lifecycle: string;
  }[];
  expect(after.find((c) => c.id === company.id)?.lifecycle).toBe(company.lifecycle);
});
