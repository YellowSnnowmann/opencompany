import { expect, test } from "@playwright/test";

import { disconnectSharedProviders } from "./shared-inference";

/**
 * The LLM page, against a real browser and a real host.
 *
 * The `Console E2E` job runs this (issue #428) and it is a merge gate, so treat
 * a red run here as a real regression rather than a stale reproduction.
 *
 * ## What these cover, and why they are the ones that need a browser
 *
 * The rules this surface is built on are pure functions with unit tests —
 * what each category offers, what a probe class means, what a removal costs,
 * whether a model id is valid. None of those needs a browser.
 *
 * What does need one is the part that only breaks in integration: a credential
 * travelling from a dialog through a write route into a store and back as a
 * boolean, a probe classification reaching the row it belongs to, and a delete
 * that has to clear a credential. Those are below.
 *
 * ## The invariant that predates the list (issue #265)
 *
 * The page must never report a successful save for a save that threw the
 * operator's key away. It used to be possible because there was **one**
 * credential slot: switching provider left the previous vendor's key in it, and
 * a managed save was a revert that carried none. The list removes the shape of
 * the bug — each provider holds its own credential — and the test for it is now
 * "two providers, two independent keys", below.
 *
 * ## The model-required flow (keys rework, issue #2306)
 *
 * Every add now goes: fill the key or endpoint, submit (opens the model step,
 * always — D-model), choose or type a model, submit again. `pickModel` below is
 * that second step, added to every existing add flow.
 *
 * ## What is deliberately not here
 *
 * The single-provider form — `#inference-provider`, `inference-save`,
 * `inference-current-provider`, the "Managed (TinyHumans)" option and its
 * `connect-tinyhumans` button — was retired with the Routing tab (commit
 * 7aa1d0b22), and "TinyHumans is offered as an ordinary catalogue row" below
 * asserts there is no `Managed` entry any more. A spec for that form,
 * "switching to the managed brain and saving stays on managed", outlived it
 * through a merge and sat red on every lane; it is gone, not skipped. Do not
 * resurrect it from an older branch.
 */

type Page = import("@playwright/test").Page;

/**
 * A fresh browser context has no tour state, so the first-run welcome dialog
 * opens over the console and swallows clicks. Skip it when it shows up.
 */
async function openInference(page: Page) {
  await page.goto("/#/connections/inference");
  const skip = page.getByRole("button", { name: "Skip for now" });
  await skip
    .waitFor({ state: "visible", timeout: 10_000 })
    .then(() => skip.click())
    .catch(() => {
      /* already seen in this context — nothing to dismiss */
    });
  // The page renders directly — no tab bar any more (routing removed, phase
  // 5b) — so waiting on either the list or its empty state is the whole wait.
  await expect(
    page.getByTestId("inference-providers").or(page.getByTestId("inference-providers-empty")),
  ).toBeVisible({ timeout: 30_000 });
}

/**
 * Wait for the connect dialog to have finished seeding its own fields.
 *
 * It resets Name, URL and Key in an effect keyed on the option it opened for,
 * so a `fill()` that lands before that effect commits is wiped by it —
 * silently, leaving a disabled Add button and a sixty-second wait on a click
 * that can never happen. That is how `a second provider holds a credential of
 * its own` failed on the live-brain lane: Name and Key were set,
 * `#inference-connect-url` was blank, and nothing on the page said so.
 *
 * Waiting on the dialog being visible is enough: React has committed the effect
 * by the time the element it mounted is in the DOM.
 */
async function connectDialogReady(page: Page) {
  await expect(page.getByTestId("inference-connect-provider")).toBeVisible();
}

/** Open the add dialog and choose one option out of a category. */
async function choose(page: Page, category: "cloud" | "local" | "cli", label: string) {
  await page.getByTestId("inference-add-open").click();
  await expect(page.getByTestId("inference-add-provider")).toBeVisible();
  await page.locator(`#inference-add-${category}`).click();
  await page.getByRole("option", { name: new RegExp(label) }).click();
  await connectDialogReady(page);
}

/**
 * Open the add dialog, then take the custom-provider route out of it.
 *
 * `inference-add-custom` is rendered inside the dialog's content, so reaching
 * for it straight off the page waits out the timeout on an element that has
 * not been mounted yet.
 */
async function addCustom(page: Page) {
  await page.getByTestId("inference-add-open").click();
  await expect(page.getByTestId("inference-add-provider")).toBeVisible();
  await page.getByTestId("inference-add-custom").click();
  await connectDialogReady(page);
}

/**
 * Step 2 of the connect dialog: the model step, which opens unconditionally
 * once step 1 succeeds (D-model, keys rework issue #2306) — there is no more
 * "this endpoint resolves tiers itself, skip the model" case. Against an
 * unreachable endpoint the draft probe fails, so the field is free text.
 */
async function pickModel(page: Page, model: string) {
  await expect(page.getByTestId("inference-connect-model-step")).toBeVisible({ timeout: 30_000 });
  // Every spec in this file points at `UNREACHABLE`, so the draft probe always
  // fails and the field is always free text (`#inference-connect-model`), never
  // the catalogue combobox — this helper does not need to handle that case.
  await page.locator("#inference-connect-model").fill(model);
  await page.getByTestId("inference-connect-submit").click();
}

/** The discard port: refused immediately, no DNS, no wait. */
const UNREACHABLE = "http://127.0.0.1:9/v1";

/**
 * Every provider slug a spec in this file can leave behind. Each is removed
 * after every test, whatever that test did — see the hook below for why.
 */
const PROVIDERS_THIS_FILE_CREATES = [
  "e2e-gateway",
  "e2e-one",
  "e2e-two",
  "ollama",
  "e2e-parked",
  "e2e-doomed",
];

/**
 * Disconnects what each test connected, after it — even one that timed out.
 *
 * Every spec here that adds a provider points it at `UNREACHABLE`, on the one
 * company the whole run shares — and the first
 * provider a company connects becomes its default (X1), so a row left behind
 * is where every agent turn in every *later* spec file goes. That turned a
 * workspace note attaching and a workflow running into `inference request
 * failed … 127.0.0.1:9/v1/chat/completions` across thirty-odd specs.
 *
 * An `afterEach` hook and not a `try { … } finally { … }` inside each test,
 * on purpose: when a test hits its timeout Playwright abandons the test
 * function outright and an in-body `finally` never runs — which is exactly
 * how the leak happened, since the leaking specs were the ones timing out.
 *
 * One flat list rather than per-test bookkeeping that could itself be
 * skipped: a slug the test already removed, or never got as far as creating,
 * answers 404 and is ignored. What the deletes can and cannot put back — the
 * rows go, the default the first of them became does not — is in
 * `disconnectSharedProviders`.
 */
test.afterEach(async ({ request }) => {
  await disconnectSharedProviders(request, PROVIDERS_THIS_FILE_CREATES);
});

/**
 * Removes a provider this file connected, so it cannot outlive its test.
 *
 * Every spec in this file runs against the one company the whole `npm run
 * e2e:live` run shares (`playwright.config.ts`'s `managesHost` path — one
 * host process, one company, for every spec file that is not first-run/Euler/
 * live-LLM/visual). `resolve_effective`'s unset-workload fallback is the
 * *primary* provider — the first enabled one (`company::inference::resolve`)
 * — so a provider this file leaves connected and enabled does not just sit
 * there: it becomes the route every agent turn in every *other* spec takes,
 * for the rest of the run. An unreachable one (every provider this file
 * creates points at `UNREACHABLE`) turns every later turn into the exact 500
 * this cleanup exists to prevent. `.catch()` swallows a delete on a slug a
 * test already removed itself.
 */
async function deleteProvider(page: Page, slug: string) {
  await page.request.delete(`/api/v1/company/inference/providers/${slug}`).catch(() => {});
}

test("TinyHumans is offered as an ordinary catalogue row, once (keys rework, slice 2a)", async ({ page }) => {
  await openInference(page);

  await page.getByTestId("inference-add-open").click();
  await expect(page.getByTestId("inference-add-provider")).toBeVisible();
  await page.locator("#inference-add-cloud").click();
  const option = page.getByRole("option", { name: /^TinyHumans/ });
  await expect(option).toBeVisible();
  await expect(option).toContainText("api.tinyhumans.ai");
  // Never a "Managed (TinyHumans)" special entry any more — it is this same
  // ordinary row.
  await expect(page.getByRole("option", { name: /^Managed/ })).toHaveCount(0);
  await page.keyboard.press("Escape");
});

test("the legacy Managed row is a connected row only when its chain actually resolves, and never renders beside a tinyhumans row", async ({
  page,
}) => {
  // Managed is not a record, so the row is keyed on whether the chain answers
  // rather than on anything having been stored. Three states are asserted
  // here on purpose: the default lane's host serves a company with no managed
  // credential anywhere in the chain, the live-brain lane's has a key but no
  // chosen model (needsModel — D-model, keys rework issue #2306), and a fully
  // resolved chain is the third. A test that assumed only one of these would
  // be red on the others — and one that simply returned early on whichever
  // state it did not expect would be quietly vacuous.
  //
  // Decision Q3 (keys rework, issue #2306, slice 2a): once a `tinyhumans` row
  // exists this legacy row must never render beside it — that is the
  // `legacyRow` field's whole job.
  await openInference(page);

  const managed = page.getByTestId("inference-provider-managed");
  const tinyhumans = page.getByTestId("inference-provider-tinyhumans");
  if ((await tinyhumans.count()) > 0) {
    await expect(managed).toHaveCount(0);
    return;
  }
  if ((await managed.count()) === 0) {
    // Nothing in the chain answers. The honest rendering is not a dead row: it
    // is no row, plus the sentence saying managed is not a fallback.
    await expect(page.getByTestId("inference-managed-fallback")).toContainText("not set up");
    return;
  }

  const needsModel = page.getByTestId("inference-provider-managed-needs-model");
  if ((await needsModel.count()) > 0) {
    // The chain has a credential but no chosen model yet (keys rework,
    // decision X5 / P1-3): the row is honest that it is not serving turns —
    // "Key added — choose a model", the affordance in place of a live switch,
    // and never the connected-sounding "Billed to" text a resolved chain gets.
    await expect(managed).toContainText("Key added — choose a model");
    await expect(needsModel).toContainText("Needs a model");
    await expect(managed.locator("[role='switch']")).toHaveCount(0);
    return;
  }

  // It resolves and a model is chosen, so the row says which step answers and
  // who it bills.
  await expect(managed).toContainText("Billed to");
  await expect(managed.locator("[role='switch']")).toHaveAttribute("aria-checked", "true");
  await expect(managed.locator("[role='switch']")).toBeEnabled();
});

test("a provider behind an unreachable endpoint is saved, amber, and keeps its key", async ({
  page,
}) => {
  // The non-destructive path, and the one the naive implementation gets wrong:
  // a proxy, a WAF, a rate limit and a mistyped model id all fail a probe while
  // the key is perfectly good.
  await openInference(page);

  try {
    await addCustom(page);
    await page.locator("#inference-connect-name").fill("E2E Gateway");
    await expect(page.getByTestId("inference-slug-preview")).toHaveText("Slug: e2e-gateway");
    await page.locator("#inference-connect-url").fill(UNREACHABLE);
    await page.locator("#inference-connect-key").fill(`pw-e2e-${Date.now()}`);
    await page.getByTestId("inference-connect-submit").click();
    await pickModel(page, "e2e-model");

    const row = page.getByTestId("inference-provider-e2e-gateway");
    await expect(row).toBeVisible({ timeout: 30_000 });
    // The row was created and the credential was kept: the save succeeded, and
    // only reachability is in question.
    await expect(row).toContainText("•••• configured");
    await expect(page.getByTestId("inference-provider-e2e-gateway-health")).toContainText(
      "unreachable",
    );

    // And it survives a reload, which is the half a component test cannot see.
    await page.reload();
    await openInference(page);
    await expect(page.getByTestId("inference-provider-e2e-gateway")).toContainText(
      "•••• configured",
    );
  } finally {
    // An enabled, unreachable provider is exactly the shape that becomes the
    // shared company's *primary* route (see `deleteProvider`) — remove it
    // regardless of where the assertions above landed.
    await deleteProvider(page, "e2e-gateway");
  }
});

test("a second provider holds a credential of its own", async ({ page }) => {
  // The first moment two keys exist at once. One slot per company is why
  // switching provider used to strand a credential for the wrong vendor in the
  // only slot there was.
  await openInference(page);

  for (const name of ["E2E One", "E2E Two"]) {
    await addCustom(page);
    await page.locator("#inference-connect-name").fill(name);
    await page.locator("#inference-connect-url").fill(UNREACHABLE);
    await page.locator("#inference-connect-key").fill(`pw-e2e-${name}-${Date.now()}`);
    await page.getByTestId("inference-connect-submit").click();
    await pickModel(page, "e2e-model");
    await expect(page.getByTestId("inference-connect-provider")).toHaveCount(0, {
      timeout: 30_000,
    });
  }

  await expect(page.getByTestId("inference-provider-e2e-one")).toContainText("•••• configured");
  await expect(page.getByTestId("inference-provider-e2e-two")).toContainText("•••• configured");
});

test("the add dialog stops offering a provider once it is connected", async ({ page }) => {
  // Offering to add something twice is how you get two rows for one provider.
  //
  // A local runtime rather than a cloud vendor, on purpose. This used to
  // connect Groq with a made-up key and take "Add anyway" past the refusal —
  // but a key the vendor rejects now stops on the key field with no "Add
  // anyway" at all (round-2 review, P1-8: the escape hatch is for an endpoint
  // that cannot be reached, never for a credential that is wrong), so there
  // is no honest way to a connected cloud row without a real key, and no
  // reason for a merge gate to depend on a vendor's network being reachable.
  await openInference(page);

  // Through the dialog first: a local runtime that is not running is rolled
  // back rather than stored looking connected (`probe::rolls_back` —
  // category-aware, the opposite of what the same class means for a cloud
  // provider), and the dialog says so and keeps the operator on the model
  // step. No row appears.
  await choose(page, "local", "Ollama");
  await page.locator("#inference-connect-url").fill(UNREACHABLE);
  await page.getByTestId("inference-connect-submit").click();
  await pickModel(page, "e2e-model");
  await expect(page.getByTestId("inference-connect-error")).toContainText(
    "so it was not connected",
    { timeout: 30_000 },
  );
  await expect(page.getByTestId("inference-provider-ollama")).toHaveCount(0);
  await page.keyboard.press("Escape");

  // Then the escape hatch, at the route the dialog itself posts to: `addAnyway`
  // is the documented way to store a row whose endpoint the probe could not
  // reach, and it is the only way to a connected catalogue row on a host with
  // nothing real behind it. The page is reloaded so what is asserted below is
  // what the list route reports, not what a dialog left in component state.
  const added = await page.request.post("/api/v1/company/inference/providers", {
    data: { kind: "ollama", baseUrl: UNREACHABLE, model: "e2e-model", addAnyway: true },
  });
  expect(added.ok()).toBeTruthy();
  await page.reload();
  await openInference(page);
  await expect(page.getByTestId("inference-provider-ollama")).toBeVisible({ timeout: 30_000 });

  await page.getByTestId("inference-add-open").click();
  await page.locator("#inference-add-local").click();
  await expect(page.getByRole("option", { name: /^Ollama/ })).toHaveCount(0);
  await page.keyboard.press("Escape");
});

test("a custom provider may not take a name the catalogue ships", async ({ page }) => {
  // A stored slug saying `cerebras` would otherwise mean two things — and the
  // refusal happens before anything is written.
  //
  // **Deliberately a catalogue row nothing else connects.** `checkSlug` reports
  // `taken` before `reserved`, and every test in this file shares one company:
  // once "the add dialog stops offering a provider once it is connected" has
  // added Ollama, typing "Ollama" here answers "This company already has a
  // provider with that name" — a true sentence about the wrong rule, and the
  // assertion below would be pinning test order rather than the reservation.
  // Cerebras is in the catalogue and is connected by no test.
  await openInference(page);

  await addCustom(page);
  await page.locator("#inference-connect-name").fill("Cerebras");
  await page.locator("#inference-connect-url").fill(UNREACHABLE);
  await expect(page.getByTestId("inference-slug-error")).toContainText("built-in");
  await expect(page.getByTestId("inference-connect-submit")).toBeDisabled();
});

test("turning a provider off and back on both confirm (decision X3)", async ({ page }) => {
  // Distinct from deleting it: "stop billing this account this week" has to be
  // expressible, and neither direction of the toggle scrubs anything.
  await openInference(page);

  await addCustom(page);
  await page.locator("#inference-connect-name").fill("E2E Parked");
  await page.locator("#inference-connect-url").fill(UNREACHABLE);
  await page.locator("#inference-connect-key").fill(`pw-e2e-${Date.now()}`);
  await page.getByTestId("inference-connect-submit").click();
  await pickModel(page, "e2e-model");
  await expect(page.getByTestId("inference-provider-e2e-parked")).toBeVisible({ timeout: 30_000 });

  // Off, reversible language.
  await page.getByTestId("inference-provider-e2e-parked-toggle").click();
  await expect(page.getByTestId("inference-remove-dialog")).toContainText("Turn off");
  await page.getByTestId("inference-remove-confirm").click();
  await expect(page.getByTestId("inference-remove-dialog")).toHaveCount(0);
  await page.reload();
  await openInference(page);

  const row = page.getByTestId("inference-provider-e2e-parked");
  await expect(row.locator("[role='switch']")).toHaveAttribute("aria-checked", "false");
  await expect(row).toContainText("•••• configured");

  // On, also confirmed (decision X3: every toggle does).
  await row.getByTestId("inference-provider-e2e-parked-toggle").click();
  await expect(page.getByTestId("inference-remove-dialog")).toContainText("Turn on");
  await page.getByTestId("inference-remove-confirm").click();
  await expect(page.getByTestId("inference-remove-dialog")).toHaveCount(0);
  await expect(row.locator("[role='switch']")).toHaveAttribute("aria-checked", "true");
});

test("deleting a provider removes its row", async ({ page }) => {
  await openInference(page);

  await addCustom(page);
  await page.locator("#inference-connect-name").fill("E2E Doomed");
  await page.locator("#inference-connect-url").fill(UNREACHABLE);
  await page.locator("#inference-connect-key").fill(`pw-e2e-${Date.now()}`);
  await page.getByTestId("inference-connect-submit").click();
  await pickModel(page, "e2e-model");
  await expect(page.getByTestId("inference-provider-e2e-doomed")).toBeVisible({ timeout: 30_000 });

  await page.getByTestId("inference-provider-e2e-doomed-menu").click();
  // Named exactly. The menu carries "Remove key" beside "Remove provider" —
  // two different acts, and the distinction between them is the whole reason
  // both are there — so a substring match resolves to both and takes neither.
  await page.getByTestId("inference-provider-e2e-doomed-remove").click();
  await expect(page.getByTestId("inference-remove-dialog")).toBeVisible();
  await page.getByTestId("inference-remove-confirm").click();
  await expect(page.getByTestId("inference-provider-e2e-doomed")).toHaveCount(0, {
    timeout: 30_000,
  });
});

test("the inference page has no routing tab (keys rework, phase 5b)", async ({ page }) => {
  // An old `?tab=routing` link still lands on the providers — there is one
  // page now.
  await openInference(page);
  await expect(page.getByRole("tab", { name: "Routing" })).toHaveCount(0);
  await expect(page.getByRole("tab", { name: "LLM Providers" })).toHaveCount(0);
  await expect(page.getByTestId("inference-add-open")).toBeVisible();
});
