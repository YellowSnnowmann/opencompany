import { expect, test } from "@playwright/test";

const API = "/api/v1/company";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const seen = JSON.stringify({ skipped: true, seenAt: Date.now() });
    for (const key of [
      "oc-tour:single",
      "oc-tour:e2e-harness-co",
      "oc-tour:null",
    ]) {
      window.localStorage.setItem(key, seen);
    }
  });
});

/** A list shape, with `evidence` required or not. */
function declaration(slug: string, evidenceRequired: boolean) {
  return {
    slug,
    title: `E2E required ${slug}`,
    purpose: "A list used to check its required-field contract.",
    derived: `derived/${slug}.md`,
    fields: [
      { name: "id", role: "id" },
      { name: "finding", role: "title", required: true },
      { name: "status", role: "status", required: true },
      {
        name: "evidence",
        role: "prose",
        required: evidenceRequired,
        description: "What actually happened, concretely.",
      },
    ],
    statuses: [{ name: "noted", label: "Noted" }],
    checks: ["required-field", "known-status"],
  };
}

/**
 * The write and the read agree about what a row must carry.
 *
 * The list declares `evidence` required. Before this, the write took a row
 * without one and answered 200, and every read then reported that same row as
 * one that could not be read — so the rendered file listed it twice, once as a
 * row and once as a fault about itself. The refusal below is that contradiction
 * closed at the only place that can close it: the write.
 */
test("a row missing a required field is refused, and a complete one reads back clean", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);

  const marker = Date.now();
  const slug = `e2e-required-${marker}`;
  const declared = await request.post(`${API}/ledgers`, {
    data: declaration(slug, true),
  });
  expect(declared.ok()).toBeTruthy();

  try {
    const refused = await request.post(`${API}/ledgers/${slug}/entries`, {
      data: {
        id: "missing-evidence",
        status: "noted",
        fields: { finding: "a row with no evidence field" },
      },
    });
    expect(refused.status()).toBe(400);
    // The refusal names the field and carries its description, so whoever
    // filled the row in wrong learns what belongs there without a second call.
    const body = await refused.text();
    expect(body).toContain("evidence");
    expect(body).toContain("What actually happened");

    // Refused means nothing landed — not stored-and-complained-about.
    const afterRefusal = await (
      await request.get(`${API}/ledgers/${slug}/entries`)
    ).json();
    expect(afterRefusal.entries).toHaveLength(0);
    expect(afterRefusal.faults ?? []).toHaveLength(0);

    // An amendment carries only what changes: the row already holds the
    // evidence, so moving its status must not be refused for not repeating it.
    const opened = await request.post(`${API}/ledgers/${slug}/entries`, {
      data: {
        id: "complete",
        status: "noted",
        fields: { finding: "a complete row", evidence: "it happened twice" },
      },
    });
    expect(opened.ok()).toBeTruthy();
    const amended = await request.post(`${API}/ledgers/${slug}/entries`, {
      data: { id: "complete", status: "noted" },
    });
    expect(amended.ok()).toBeTruthy();

    await page.goto(`/#/company/work/${slug}`);
    await expect(page.getByTestId("ledger-entry-complete")).toBeVisible({
      timeout: 15_000,
    });
    // The contract, on screen: a list holding only rows the write accepted
    // reports nothing unreadable.
    await expect(page.getByTestId("ledger-faults")).toHaveCount(0);
  } finally {
    await request.delete(`${API}/ledgers/${slug}?purge=true`);
  }
});

/**
 * A row that predates the requirement is named, not merely counted.
 *
 * The write refusing new ones does not empty the fault surface: a list amended
 * to require a field it did not before still holds rows written under the older
 * shape, and those are reported rather than hidden. Editing a list is a retire
 * (which keeps the rows) and a re-declare, which is what this reproduces.
 *
 * The console used to answer that with "1 row could not be read" and nothing
 * else — the row and the reason were behind a closed disclosure, while the
 * workspace's rendered copy printed both. A fault nobody can act on is a fault
 * nobody acts on.
 */
test("the board names the row that could not be read and why", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);

  const marker = Date.now();
  const slug = `e2e-legacy-${marker}`;
  const declared = await request.post(`${API}/ledgers`, {
    data: declaration(slug, false),
  });
  expect(declared.ok()).toBeTruthy();

  try {
    const legacy = await request.post(`${API}/ledgers/${slug}/entries`, {
      data: {
        id: "predates-the-rule",
        status: "noted",
        fields: { finding: "written before evidence was required" },
      },
    });
    expect(legacy.ok()).toBeTruthy();

    // Retire without `purge`, so the rows survive the shape change.
    expect((await request.delete(`${API}/ledgers/${slug}`)).ok()).toBeTruthy();
    const redeclared = await request.post(`${API}/ledgers`, {
      data: declaration(slug, true),
    });
    expect(redeclared.ok()).toBeTruthy();

    await page.goto(`/#/company/work/${slug}`);
    const faults = page.getByTestId("ledger-faults");
    await expect(faults).toBeVisible({ timeout: 15_000 });
    await expect(faults).toContainText("1 row could not be read");

    // Asserted on the *rendered* text, not on `textContent`: the reason this
    // was reported at all is that the row and the fault sat inside a closed
    // `<details>`, and a collapsed disclosure still carries its text content.
    // `toContainText` would therefore have passed against the very bug this
    // pins. What has to be true is that a reader sees it without opening
    // anything.
    await expect(
      faults.getByText("predates-the-rule", { exact: false }),
    ).toBeVisible();
    const stated = await faults.innerText();
    expect(stated).toContain("predates-the-rule");
    expect(stated).toContain("evidence");
  } finally {
    await request.delete(`${API}/ledgers/${slug}?purge=true`);
  }
});

/**
 * The count beside the list's name is counted over the rows on screen.
 *
 * It used to come from the sidebar's own read, which happens once per company
 * and again only when a list is declared or retired — never when a row is
 * recorded. So a list opened while empty went on saying zero with a live row
 * rendered underneath it, until a full page reload.
 */
test("the open count follows a row recorded after the screen was opened", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);

  const marker = Date.now();
  const slug = `e2e-count-${marker}`;
  const declared = await request.post(`${API}/ledgers`, {
    data: declaration(slug, true),
  });
  expect(declared.ok()).toBeTruthy();

  try {
    await page.goto(`/#/company/work/${slug}`);
    const count = page.getByTestId("ledger-open-count");
    await expect(count).toHaveText("0", { timeout: 15_000 });

    const recorded = await request.post(`${API}/ledgers/${slug}/entries`, {
      data: {
        id: "live-row",
        status: "noted",
        fields: { finding: "a live row", evidence: "it happened" },
      },
    });
    expect(recorded.ok()).toBeTruthy();

    await page.getByRole("button", { name: /refresh/i }).first().click();
    await expect(page.getByTestId("ledger-entry-live-row")).toBeVisible({
      timeout: 15_000,
    });
    // The row is on screen, so the count beside the title cannot still say
    // there is nothing here.
    await expect(count).toHaveText("1");
  } finally {
    await request.delete(`${API}/ledgers/${slug}?purge=true`);
  }
});

/**
 * The count beside the list's name describes the rows rendered under it.
 *
 * `read_ledger` used to answer with a count from a second, independent fold
 * of the ledger — taken separately from the one that produced the rows in
 * the same response. A write landing between the two could flip a row's
 * status after the rows were read but before the count was, so the badge
 * disagreed with what the screen actually showed beside it. This checks the
 * agreement against the DOM itself — counting the rows Playwright can see,
 * not a second value pulled from another request — after a write and after
 * a close, which is where a stale or independently-folded count would show.
 */
test("the open count agrees with the rows the screen actually shows", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);

  const marker = Date.now();
  const slug = `e2e-count-agree-${marker}`;
  const declared = await request.post(`${API}/ledgers`, {
    data: {
      slug,
      title: `E2E count agree ${marker}`,
      purpose: "A list used to check the badge against the rows it counts.",
      derived: `derived/${slug}.md`,
      fields: [
        { name: "id", role: "id" },
        { name: "risk", role: "title" },
        { name: "status", role: "status" },
        { name: "reason", role: "prose" },
      ],
      statuses: [
        { name: "open" },
        { name: "closed", closed: true, needs_reason: true },
      ],
    },
  });
  expect(declared.ok()).toBeTruthy();

  try {
    const recordRow = (id: string, risk: string) =>
      request.post(`${API}/ledgers/${slug}/entries`, {
        data: { id, status: "open", fields: { risk } },
      });
    expect((await recordRow("r1", "first")).ok()).toBeTruthy();
    expect((await recordRow("r2", "second")).ok()).toBeTruthy();

    await page.goto(`/#/company/work/${slug}`);
    const count = page.getByTestId("ledger-open-count");
    await expect(page.getByTestId("ledger-entry-r2")).toBeVisible({
      timeout: 15_000,
    });

    // The invariant: the badge equals however many rows the DOM itself
    // renders as not-closed — never a fixed number asserted independently
    // of what is actually on screen.
    const assertBadgeMatchesRows = async () => {
      const statuses = await page
        .getByTestId("ledger-entry-status")
        .allTextContents();
      const openRows = statuses.filter(
        (text) => !text.trim().startsWith("Closed"),
      ).length;
      await expect(count).toHaveText(String(openRows));
    };
    await assertBadgeMatchesRows();
    await expect(count).toHaveText("2");

    // Record a third row live: the badge must move with the rows.
    expect((await recordRow("r3", "third")).ok()).toBeTruthy();
    await page.getByRole("button", { name: /refresh/i }).first().click();
    await expect(page.getByTestId("ledger-entry-r3")).toBeVisible({
      timeout: 15_000,
    });
    await assertBadgeMatchesRows();
    await expect(count).toHaveText("3");

    // Close one: the badge must drop with the row, not lag behind it.
    const closed = await request.post(`${API}/ledgers/${slug}/entries`, {
      data: { id: "r1", status: "closed", reason: "resolved" },
    });
    expect(closed.ok()).toBeTruthy();
    await page.getByRole("button", { name: /refresh/i }).first().click();
    await expect(
      page.getByTestId("ledger-entry-r1").getByTestId("ledger-entry-status"),
    ).toHaveText("Closed", { timeout: 15_000 });
    await assertBadgeMatchesRows();
    await expect(count).toHaveText("2");
  } finally {
    await request.delete(`${API}/ledgers/${slug}?purge=true`);
  }
});
