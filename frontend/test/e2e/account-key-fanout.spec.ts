import { expect, test } from "@playwright/test";

/**
 * The Account page's key dialog, fully mocked (keys rework, issue #2306,
 * slice 4b) — `docs/key-reworks/phase-4b-account-dialog.md` §6, plus the
 * decision "X10" (2026-09-15) that a successful *live* account-key save
 * cannot be exercised here at all: this repo has no real TinyHumans key to
 * give it, so a live save would either hit the network or fail before it
 * proves anything. Every case below is instead driven through `page.route`
 * stubs shaped exactly like `company_key::fan_out`'s real response
 * (`MutationResponse`, `src/server/ops/company_key.rs`), following
 * `composio-mode-switch.spec.ts`'s pattern: a running host
 * (`playwright.config.ts` brings one up), with the one route under test
 * (`GET`/`PUT …/credential`) answered by this file instead of a real
 * TinyHumans backend. No real credential anywhere — every key below is
 * obviously fake (`th-not-a-real-key`, `th-not-a-real-key-2`,
 * `th-not-a-real-key-custom`, the exact matrix the phase-4a plan itself
 * uses).
 *
 * Coverage, beyond the phase-4b doc's own single `needsModel` case (decision
 * "X10" asks for the success path in full since nothing here can reach a
 * live one):
 *  - one fill-line variant per derived slot, the all-three variant, and the
 *    no-line case
 *  - a rotation (an existing account key replaced by a new one)
 *  - a clear that only touches copies still equal to the old key
 *  - a save that overwrites no derived slot when each holds a custom key
 *  - an auth rejection that rolls the LLM copy back while Composio keeps its
 */

type Page = import("@playwright/test").Page;
type Route = import("@playwright/test").Route;

const isCredential = (url: URL) => /\/credential$/.test(url.pathname);
const isBilling = (url: URL) => /\/credential\/billing$/.test(url.pathname);

const KEY_A = "th-not-a-real-key";
const KEY_B = "th-not-a-real-key-2";
const KEY_CUSTOM = "th-not-a-real-key-custom";

/** A `GET …/credential` status, shaped like the real `CredentialStatusDto`. */
function status(over: Record<string, unknown> = {}) {
  return {
    configured: false,
    source: "none",
    notice: "This is the company's TinyHumans account key.",
    hubLink: false,
    inferenceHasOwnKey: false,
    composioHasOwnKey: false,
    // This suite covers the pre-existing LLM and Composio fan-out matrix. Keep
    // the newer managed Search slot occupied unless a case explicitly tests it,
    // so `accountFills` has a complete status shape without changing each
    // sentence's intended two-slot assertion.
    searchHasOwnKey: true,
    defaultSet: false,
    ...over,
  };
}

/** One `SlotReportDto`. */
function slotReport(slot: string, outcome: string, detail?: string) {
  return detail === undefined ? { slot, outcome } : { slot, outcome, detail };
}

/** A `MutationResponse`, shaped like the real host's `PUT …/credential` answer. */
function mutation(over: Record<string, unknown> = {}) {
  return {
    status: status({ configured: true, source: "company" }),
    note: "Key saved.",
    slots: [
      slotReport("composio", "filled"),
      slotReport("inference", "filled"),
      slotReport("provider", "skipped", "needsModel"),
      slotReport("default", "skipped", "needsModel"),
      slotReport("health", "ok"),
    ],
    needsModel: false,
    setsDefault: false,
    ...over,
  };
}

/** Answers `GET …/credential` with a fixed body, every time it is asked. */
async function stubStatus(page: Page, body: Record<string, unknown>): Promise<void> {
  await page.route(isCredential, async (route: Route) => {
    if (route.request().method() !== "GET") return route.fallback();
    await route.fulfill({ json: body });
  });
}

/** Answers `PUT …/credential` from a fixed queue, one response per call. */
async function stubSaves(page: Page, responses: Record<string, unknown>[]): Promise<void> {
  let call = 0;
  await page.route(isCredential, async (route: Route) => {
    if (route.request().method() !== "PUT") return route.fallback();
    const response = responses[Math.min(call, responses.length - 1)];
    call += 1;
    await route.fulfill({ json: response });
  });
}

/**
 * The same queue as {@link stubSaves}, but each entry names its own HTTP
 * status — for KR-L3-01's stale-then-`409` case, where the first `PUT` must
 * answer the host's real `in_use` refusal rather than a 200.
 */
async function stubSavesWithStatus(
  page: Page,
  responses: { status?: number; json: Record<string, unknown> }[],
): Promise<void> {
  let call = 0;
  await page.route(isCredential, async (route: Route) => {
    if (route.request().method() !== "PUT") return route.fallback();
    const response = responses[Math.min(call, responses.length - 1)];
    call += 1;
    await route.fulfill({ status: response.status ?? 200, json: response.json });
  });
}

/**
 * Answers `GET …/credential/billing` so the card's state is decided by this
 * file and not by whatever the running host's own hub read returns.
 *
 * The page reads billing alongside the credential and the account row's state
 * is drawn from both — a stored key the hub refuses is a different row from a
 * stored key, and `isCredential` does not match this path. Left to the live
 * host, every assertion below would depend on a hub answer no stub controls.
 */
async function stubBilling(page: Page, body: Record<string, unknown>): Promise<void> {
  await page.route(isBilling, async (route: Route) => {
    if (route.request().method() !== "GET") return route.fallback();
    await route.fulfill({ json: body });
  });
}

/** Open the Account page with the first-run tour out of the way. */
async function openAccount(page: Page): Promise<void> {
  await stubBilling(page, { configured: false });
  await page.goto("/#/connections/api-key");
  const skip = page.getByRole("button", { name: "Skip for now" });
  await skip
    .waitFor({ state: "visible", timeout: 10_000 })
    .then(() => skip.click())
    .catch(() => {
      /* already dismissed in this context */
    });
  await expect(
    page.getByTestId("account-rows").or(page.getByTestId("account-empty")),
  ).toBeVisible({ timeout: 30_000 });
}

const toasts = (page: Page) => page.locator("[data-sonner-toast]");

test.describe("the fill line names only the slots saving would fill", () => {
  test("every slot empty: names LLM, Composio and Search, without claiming LLM is connected", async ({
    page,
  }) => {
    await stubStatus(
      page,
      status({
        inferenceHasOwnKey: false,
        composioHasOwnKey: false,
        searchHasOwnKey: false,
      }),
    );
    await openAccount(page);
    await page.getByTestId("account-add-key").click();

    const line = page.getByTestId("account-key-fill-line");
    await expect(line).toContainText(
      "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next — connects it for Composio, and uses it as this company's managed Search credential.",
    );
    await expect(line).not.toContainText("connects TinyHumans for LLM");
    await expect(page.getByTestId("account-key-llm-link")).toBeVisible();
    await expect(page.getByTestId("account-key-composio-link")).toBeVisible();
    await expect(page.getByTestId("account-key-search-link")).toBeVisible();
  });

  test("only the LLM slot is empty: names LLM alone, and still does not say connected", async ({
    page,
  }) => {
    await stubStatus(
      page,
      status({
        inferenceHasOwnKey: false,
        composioHasOwnKey: true,
        searchHasOwnKey: true,
      }),
    );
    await openAccount(page);
    await page.getByTestId("account-add-key").click();

    const line = page.getByTestId("account-key-fill-line");
    await expect(line).toContainText(
      "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next.",
    );
    await expect(page.getByTestId("account-key-llm-link")).toBeVisible();
    await expect(page.getByTestId("account-key-composio-link")).toHaveCount(0);
    await expect(page.getByTestId("account-key-search-link")).toHaveCount(0);
  });

  test("only the Composio slot is empty: names Composio alone", async ({ page }) => {
    await stubStatus(
      page,
      status({
        inferenceHasOwnKey: true,
        composioHasOwnKey: false,
        searchHasOwnKey: true,
      }),
    );
    await openAccount(page);
    await page.getByTestId("account-add-key").click();

    const line = page.getByTestId("account-key-fill-line");
    await expect(line).toContainText("Saving also connects TinyHumans for Composio.");
    await expect(page.getByTestId("account-key-llm-link")).toHaveCount(0);
    await expect(page.getByTestId("account-key-composio-link")).toBeVisible();
    await expect(page.getByTestId("account-key-search-link")).toHaveCount(0);
  });

  test("only the Search slot is empty: names Search alone", async ({ page }) => {
    await stubStatus(
      page,
      status({
        inferenceHasOwnKey: true,
        composioHasOwnKey: true,
        searchHasOwnKey: false,
      }),
    );
    await openAccount(page);
    await page.getByTestId("account-add-key").click();

    const line = page.getByTestId("account-key-fill-line");
    await expect(line).toContainText(
      "Saving also uses this key as the company's managed Search credential.",
    );
    await expect(page.getByTestId("account-key-llm-link")).toHaveCount(0);
    await expect(page.getByTestId("account-key-composio-link")).toHaveCount(0);
    await expect(page.getByTestId("account-key-search-link")).toBeVisible();
  });

  test("every slot already holds its own key: no line at all", async ({ page }) => {
    await stubStatus(
      page,
      status({
        inferenceHasOwnKey: true,
        composioHasOwnKey: true,
        searchHasOwnKey: true,
      }),
    );
    await openAccount(page);
    await page.getByTestId("account-add-key").click();

    await expect(page.getByTestId("account-key-fill-line")).toHaveCount(0);
  });
});

test("saving asks for a model when the host needs one, then reposts the key with it", async ({
  page,
}) => {
  await stubStatus(
    page,
    status({
      inferenceHasOwnKey: false,
      composioHasOwnKey: false,
      searchHasOwnKey: true,
    }),
  );
  await stubSaves(page, [
    mutation({
      status: status({ configured: true, source: "company" }),
      note: "Key saved. Choose a model to finish setting up TinyHumans for LLM.",
      slots: [
        slotReport("composio", "filled"),
        slotReport("inference", "filled"),
        slotReport("provider", "skipped", "needsModel"),
        slotReport("default", "skipped", "needsModel"),
        slotReport("health", "ok"),
      ],
      needsModel: true,
      setsDefault: true,
      models: ["acme/test-model", "acme/other-model"],
    }),
    mutation({
      status: status({ configured: true, source: "company", defaultSet: true }),
      note:
        "Key saved. TinyHumans is set up for LLM with acme/test-model. It is now the default for new work.",
      slots: [
        slotReport("composio", "kept", "alreadyCurrent"),
        slotReport("inference", "kept", "alreadyCurrent"),
        slotReport("provider", "filled"),
        slotReport("default", "filled"),
        slotReport("health", "ok"),
      ],
      needsModel: false,
      setsDefault: false,
    }),
  ]);

  await openAccount(page);
  await page.getByTestId("account-add-key").click();
  await expect(page.getByTestId("account-key-fill-line")).toContainText(
    "Saving also adds this key to TinyHumans on the LLM page, with the model you choose next — and connects it for Composio.",
  );

  await page.getByTestId("account-key-input").fill(KEY_A);
  const firstSave = page.waitForRequest(
    (request) => isCredential(new URL(request.url())) && request.method() === "PUT",
  );
  await page.getByTestId("account-key-save").click();
  const first = await firstSave;
  expect(first.postDataJSON()).toEqual({ key: KEY_A });

  await expect(page.getByTestId("account-key-model-step")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Choose the model new work uses" })).toBeVisible();
  await expect(page.getByTestId("account-key-note")).toHaveText(
    "Key saved. Choose a model to finish setting up TinyHumans for LLM.",
  );

  // The real combobox, in a real browser — `inference-mocked.spec.ts`'s own
  // pattern for the catalogue select `ModelCombobox` renders.
  const trigger = page.locator("#account-key-model");
  await expect(trigger).toHaveText("Choose a model");
  await trigger.click();
  await page.getByRole("listbox", { name: "Models" }).getByRole("option", { name: "acme/test-model" }).click();
  await expect(trigger).toHaveText("acme/test-model");

  const secondSave = page.waitForRequest(
    (request) => isCredential(new URL(request.url())) && request.method() === "PUT",
  );
  await page.getByTestId("account-key-model-save").click();
  const second = await secondSave;
  expect(second.postDataJSON()).toEqual({ key: KEY_A, model: "acme/test-model" });

  await expect(page.getByTestId("account-key-model-step")).toHaveCount(0);
  const message = toasts(page).first();
  await expect(message).toBeVisible({ timeout: 10_000 });
  await expect(message).toContainText("TinyHumans is set up for LLM");
});

test("a rejected model save keeps the model step open for retry", async ({ page }) => {
  await stubStatus(page, status({ inferenceHasOwnKey: false, composioHasOwnKey: false }));
  await stubSaves(page, [
    mutation({
      needsModel: true,
      models: ["acme/test-model"],
    }),
    mutation({
      slots: [
        slotReport("composio", "kept", "alreadyCurrent"),
        slotReport("inference", "kept", "alreadyCurrent"),
        slotReport("provider", "skipped", "inferenceRejected"),
        slotReport("default", "skipped", "inferenceRejected"),
        slotReport("health", "failed", "auth"),
      ],
    }),
  ]);

  await openAccount(page);
  await page.getByTestId("account-add-key").click();
  await page.getByTestId("account-key-input").fill(KEY_A);
  await page.getByTestId("account-key-save").click();
  await expect(page.getByTestId("account-key-model-step")).toBeVisible();

  const trigger = page.locator("#account-key-model");
  await trigger.click();
  await page.getByRole("listbox", { name: "Models" }).getByRole("option", { name: "acme/test-model" }).click();
  await page.getByTestId("account-key-model-save").click();

  await expect(page.getByTestId("account-key-model-step")).toBeVisible();
  await expect(page.getByText("The key was saved, but its model could not be applied. Please try again.")).toBeVisible();
  await expect(toasts(page)).toHaveCount(0);
});

test("rotation: an existing account key is replaced by a new one", async ({ page }) => {
  // `replacing` (`canRemoveKey`) is keyed on `source === "company"`, so the
  // header's Connect button is gone and the row's own menu carries Replace.
  await stubStatus(
    page,
    status({
      configured: true,
      source: "company",
      inferenceHasOwnKey: false,
      composioHasOwnKey: false,
      defaultSet: true,
    }),
  );
  await stubSaves(page, [
    mutation({
      note: "Key saved.",
      slots: [
        slotReport("composio", "rotated"),
        slotReport("inference", "rotated"),
        slotReport("provider", "kept", "rowExists"),
        slotReport("default", "kept", "defaultAlreadySet"),
        slotReport("health", "ok"),
      ],
      needsModel: false,
      setsDefault: false,
    }),
  ]);

  await openAccount(page);
  await expect(page.getByTestId("account-add-key")).toHaveCount(0);
  await page.getByTestId("account-row-menu").click();
  await page.getByRole("menuitem", { name: "Replace key" }).click();

  await expect(page.getByRole("heading", { name: "Replace your API key" })).toBeVisible();
  await page.getByTestId("account-key-input").fill(KEY_B);
  const saved = page.waitForRequest(
    (request) => isCredential(new URL(request.url())) && request.method() === "PUT",
  );
  await page.getByTestId("account-key-save").click();
  const request = await saved;
  expect(request.postDataJSON()).toEqual({ key: KEY_B });

  // No model step on a rotation into an existing row/default — the dialog
  // closes at once.
  await expect(page.getByTestId("account-key-input")).toHaveCount(0);
  const message = toasts(page).first();
  await expect(message).toBeVisible({ timeout: 10_000 });
  await expect(message).toContainText("Key saved.");
});

test("clearing removes only the copies still equal to the old key — a custom key elsewhere survives", async ({
  page,
}) => {
  await stubStatus(
    page,
    status({
      configured: true,
      source: "company",
      // The Composio copy is still the fanned-out account key; the LLM copy
      // was pasted by hand on the LLM page and is not the account key.
      inferenceHasOwnKey: true,
      composioHasOwnKey: false,
      defaultSet: true,
    }),
  );
  await stubSaves(page, [
    mutation({
      status: status({ configured: false, source: "none" }),
      note:
        "Key removed. Composio's copy was removed too. LLM keeps the TinyHumans key set on its own page.",
      slots: [
        slotReport("composio", "cleared"),
        slotReport("inference", "kept", "customKey"),
        slotReport("provider", "skipped", "keyCleared"),
        slotReport("default", "skipped", "keyCleared"),
        slotReport("health", "skipped", "keyCleared"),
      ],
      needsModel: false,
      setsDefault: false,
    }),
  ]);

  await openAccount(page);
  await page.getByTestId("account-row-menu").click();
  await page.getByTestId("account-remove-key").click();
  await expect(page.getByRole("heading", { name: "Remove this company's account key?" })).toBeVisible();

  const cleared = page.waitForRequest(
    (request) => isCredential(new URL(request.url())) && request.method() === "PUT",
  );
  await page.getByTestId("account-remove-key-confirm").click();
  const request = await cleared;
  expect(request.postDataJSON()).toEqual({ key: "" });

  const message = toasts(page).first();
  await expect(message).toBeVisible({ timeout: 10_000 });
  await expect(message).toContainText("Composio's copy was removed too.");
  await expect(message).toContainText("LLM keeps the TinyHumans key set on its own page.");
});

test.describe("KR-L3-01: the Remove-key dialog names dependents and honors confirmInUse", () => {
  // Bug KR-L3-01: "Remove key" could not finish once anything depended on the
  // account key — the frontend never sent `confirmInUse`, there was no
  // reopen-with-reason flow, and the dialog's text never named what depends
  // on the key. This covers the two paths the fix adds: known up front (from
  // the status the page already read, no round trip), and the stale case
  // (nothing known at open, but the host answers `409 in_use` on submit).

  test("in-use at open: the dialog names dependents with no round trip, and confirms with confirmInUse", async ({
    page,
  }) => {
    await stubStatus(
      page,
      status({
        configured: true,
        source: "company",
        inferenceHasOwnKey: false,
        composioHasOwnKey: false,
        defaultSet: true,
        usedBy: { surfaces: ["llm", "composio"] },
      }),
    );
    await stubSaves(page, [
      mutation({
        status: status({ configured: false, source: "none" }),
        note: "Key removed. Composio's copy was removed too.",
        slots: [
          slotReport("composio", "cleared"),
          slotReport("inference", "cleared"),
          slotReport("provider", "skipped", "keyCleared"),
          slotReport("default", "skipped", "keyCleared"),
          slotReport("health", "skipped", "keyCleared"),
        ],
        needsModel: false,
        setsDefault: false,
        usedBy: { surfaces: ["llm", "composio"] },
      }),
    ]);

    await openAccount(page);
    await page.getByTestId("account-row-menu").click();
    await page.getByTestId("account-remove-key").click();

    // Named the moment the dialog opens — no `PUT` has happened yet.
    await expect(page.getByTestId("account-remove-key-reason")).toHaveText(
      "Used by TinyHumans on the LLM page and by Composio.",
    );

    const cleared = page.waitForRequest(
      (request) => isCredential(new URL(request.url())) && request.method() === "PUT",
    );
    await page.getByTestId("account-remove-key-confirm").click();
    const request = await cleared;
    expect(request.postDataJSON()).toEqual({ key: "", confirmInUse: true });

    await expect(
      page.getByRole("heading", { name: "Remove this company's account key?" }),
    ).toHaveCount(0);
    const message = toasts(page).first();
    await expect(message).toBeVisible({ timeout: 10_000 });
    await expect(message).toContainText("Composio's copy was removed too.");
  });

  test("stale at open: a 409 on submit reopens the dialog with the server's reason, then clears on confirm", async ({
    page,
  }) => {
    // Nothing depends on the key when the dialog opens — no `usedBy` on the
    // status the page read — but something else started depending on it
    // (a Composio connection made from another tab, say) before the confirm
    // reaches the host.
    await stubStatus(
      page,
      status({ configured: true, source: "company", defaultSet: true }),
    );
    await stubSavesWithStatus(page, [
      {
        status: 409,
        json: {
          error: "Used by Composio.",
          code: "in_use",
          usedBy: { surfaces: ["composio"] },
        },
      },
      {
        json: mutation({
          status: status({ configured: false, source: "none" }),
          note: "Key removed.",
          usedBy: { surfaces: ["composio"] },
        }),
      },
    ]);

    await openAccount(page);
    await page.getByTestId("account-row-menu").click();
    await page.getByTestId("account-remove-key").click();
    await expect(page.getByTestId("account-remove-key-reason")).toHaveCount(0);

    const firstAttempt = page.waitForRequest(
      (request) => isCredential(new URL(request.url())) && request.method() === "PUT",
    );
    await page.getByTestId("account-remove-key-confirm").click();
    const first = await firstAttempt;
    expect(first.postDataJSON()).toEqual({ key: "" });

    // Refused, uninformed — the dialog reopens with the host's own reason
    // rather than closing on a bare toast (the bug this dispatch fixes).
    await expect(
      page.getByRole("heading", { name: "Remove this company's account key?" }),
    ).toBeVisible();
    await expect(page.getByTestId("account-remove-key-reason")).toHaveText(
      "Used by Composio.",
    );

    const secondAttempt = page.waitForRequest(
      (request) => isCredential(new URL(request.url())) && request.method() === "PUT",
    );
    await page.getByTestId("account-remove-key-confirm").click();
    const second = await secondAttempt;
    expect(second.postDataJSON()).toEqual({ key: "", confirmInUse: true });

    await expect(
      page.getByRole("heading", { name: "Remove this company's account key?" }),
    ).toHaveCount(0);
  });
});

test("never overwrites a key set on another page — every derived slot is kept", async ({
  page,
}) => {
  await stubStatus(
    page,
    status({
      inferenceHasOwnKey: true,
      composioHasOwnKey: true,
      searchHasOwnKey: true,
    }),
  );
  await stubSaves(page, [
    mutation({
      note:
        "Key saved. Composio keeps the key set on its own page. LLM keeps the TinyHumans key set on its own page.",
      slots: [
        slotReport("composio", "kept", "customKey"),
        slotReport("inference", "kept", "customKey"),
        slotReport("provider", "skipped", "customKey"),
        slotReport("default", "skipped", "customKey"),
        slotReport("health", "skipped", "customKey"),
      ],
      needsModel: false,
      setsDefault: false,
    }),
  ]);

  await openAccount(page);
  await page.getByTestId("account-add-key").click();
  // No fill line at all — saving would touch no derived slot.
  await expect(page.getByTestId("account-key-fill-line")).toHaveCount(0);

  await page.getByTestId("account-key-input").fill(KEY_CUSTOM);
  const saved = page.waitForRequest(
    (request) => isCredential(new URL(request.url())) && request.method() === "PUT",
  );
  await page.getByTestId("account-key-save").click();
  const request = await saved;
  // The account key itself is still written — only the derived copies are
  // left alone.
  expect(request.postDataJSON()).toEqual({ key: KEY_CUSTOM });

  const message = toasts(page).first();
  await expect(message).toBeVisible({ timeout: 10_000 });
  await expect(message).toContainText("Composio keeps the key set on its own page.");
  await expect(message).toContainText("LLM keeps the TinyHumans key set on its own page.");
});

test("an auth rejection rolls the LLM copy back while the Composio copy stays, surfaced as the save's own toast", async ({
  page,
}) => {
  await stubStatus(page, status({ inferenceHasOwnKey: false, composioHasOwnKey: false }));
  await stubSaves(page, [
    mutation({
      status: status({ configured: true, source: "company" }),
      note:
        "Key saved. Composio now uses this key. A key you created by hand may lack the connections permission Composio needs. TinyHumans rejected this key for LLM, so the LLM copy was not kept.",
      slots: [
        slotReport("composio", "filled"),
        slotReport("inference", "rolledBack"),
        slotReport("provider", "skipped", "inferenceRejected"),
        slotReport("default", "skipped", "inferenceRejected"),
        slotReport("health", "failed", "auth"),
      ],
      // Q6/the dialog's own gotcha: an auth rejection never asks for a model —
      // there is nothing new on the LLM side to name one for.
      needsModel: false,
      setsDefault: false,
    }),
  ]);

  await openAccount(page);
  await page.getByTestId("account-add-key").click();
  await page.getByTestId("account-key-input").fill(KEY_A);
  const saved = page.waitForRequest(
    (request) => isCredential(new URL(request.url())) && request.method() === "PUT",
  );
  await page.getByTestId("account-key-save").click();
  await saved;

  // Not a save error — the account key genuinely saved. The dialog closes
  // with the host's note as the toast rather than staying open with an
  // error in it.
  await expect(page.getByTestId("account-key-input")).toHaveCount(0);
  await expect(page.getByTestId("account-key-model-step")).toHaveCount(0);
  const message = toasts(page).first();
  await expect(message).toBeVisible({ timeout: 10_000 });
  await expect(message).toContainText("Composio now uses this key.");
  await expect(message).toContainText("TinyHumans rejected this key for LLM");
});

test("a response from a host predating slice 4a degrades to a single step, with no fill line", async ({
  page,
}) => {
  // No `inferenceHasOwnKey`/`composioHasOwnKey`/`searchHasOwnKey`/`defaultSet`, and a `PUT`
  // answer with no `slots`/`needsModel` at all — exactly what an older host
  // sends.
  await stubStatus(page, {
    configured: false,
    source: "none",
    notice: "n",
    hubLink: false,
  });
  await stubSaves(page, [{ status: { configured: true, source: "company" }, note: "Key saved." }]);

  await openAccount(page);
  await page.getByTestId("account-add-key").click();
  await expect(page.getByTestId("account-key-fill-line")).toHaveCount(0);

  await page.getByTestId("account-key-input").fill(KEY_A);
  await page.getByTestId("account-key-save").click();

  await expect(page.getByTestId("account-key-input")).toHaveCount(0);
  await expect(page.getByTestId("account-key-model-step")).toHaveCount(0);
});
