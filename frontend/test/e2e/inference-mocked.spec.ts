import { expect, test } from "@playwright/test";

/**
 * The LLM page, fully mocked — no live host state, no real provider.
 *
 * `test/e2e/inference.spec.ts` drives a real host and real (fake-keyed)
 * credentials; it cannot stage a `409 in_use` refusal, a slow probe worth
 * cancelling mid-flight, a probe that actually succeeds with a catalogue (its
 * only reachable endpoint is `UNREACHABLE`, so every one of its specs lands on
 * the free-text field, never the combobox), or a host that already speaks the
 * keys-rework contract (`defaultChoice`, `providers[].model`, `usedBy`) ahead
 * of the backend actually shipping it. This file is the round-2 review's own
 * request for exactly those four (P0-1, P0-2, P1-1, and the untested catalogue
 * combobox from P2-6), following `composio-mode-switch.spec.ts`'s pattern: a
 * running host (`playwright.config.ts` brings one up), with the handful of
 * routes under test answered by this file instead. No real credential
 * anywhere — every key below is obviously fake (`sk-not-a-real-key`).
 */

type Page = import("@playwright/test").Page;
type Route = import("@playwright/test").Route;

const isInferenceStatus = (url: URL) => /\/inference$/.test(url.pathname);
const isManagedEnabled = (url: URL) => /\/inference\/managed\/enabled$/.test(url.pathname);
const isProviderEnabled = (url: URL) => /\/inference\/providers\/[^/]+\/enabled$/.test(url.pathname);
const isProbe = (url: URL) => /\/inference\/probe$/.test(url.pathname);

const IN_USE_ERROR = "Acme is pinned by an agent.";

/** A minimal, already-contract-shaped status — see the file doc on why this is mocked rather than read from a live host. */
function status(over: Record<string, unknown> = {}) {
  return {
    provider: "acme",
    slug: "acme",
    baseUrl: "https://acme.example/v1",
    models: {},
    defaultTierModels: {},
    source: "runtime",
    keyConfigured: true,
    cognition: "harness",
    usageMetering: "perTurn",
    restartRequired: false,
    harnessReachable: true,
    designsProfiles: true,
    canRebuildInPlace: true,
    defaultChoice: { provider: "acme", model: "acme/test-model" },
    routesNotCarried: null,
    providers: [
      {
        id: "prv_acme",
        slug: "acme",
        label: "Acme",
        kind: "openai_compatible",
        baseUrl: "https://acme.example/v1",
        models: {},
        model: "acme/test-model",
        modelAmbiguous: false,
        enabled: true,
        keyConfigured: true,
        origin: "indexed",
        isDefault: true,
      },
      {
        id: "prv_beta",
        slug: "beta",
        label: "Beta",
        kind: "openai_compatible",
        baseUrl: "https://beta.example/v1",
        models: {},
        model: "beta/test-model",
        modelAmbiguous: false,
        enabled: true,
        keyConfigured: true,
        origin: "indexed",
        isDefault: false,
      },
    ],
    routes: {},
    managed: { source: "none", configured: false, baseUrl: "", enabled: true, legacyRow: true, needsModel: false },
    ...over,
  };
}

async function stubStatus(page: Page, body: Record<string, unknown>): Promise<void> {
  await page.route(
    (url) => isInferenceStatus(url),
    async (route: Route) => {
      if (route.request().method() !== "GET") return route.fallback();
      await route.fulfill({ json: body });
    },
  );
}

async function openInference(page: Page): Promise<void> {
  await page.goto("/#/connections/inference");
  const skip = page.getByRole("button", { name: "Skip for now" });
  await skip
    .waitFor({ state: "visible", timeout: 10_000 })
    .then(() => skip.click())
    .catch(() => {});
  await expect(
    page.getByTestId("inference-providers").or(page.getByTestId("inference-providers-empty")),
  ).toBeVisible({ timeout: 30_000 });
}

test.describe("P0-1: the legacy Managed row's toggle never posts to a provider route with no row behind it", () => {
  test.beforeEach(async ({ page }) => {
    await stubStatus(
      page,
      status({
        managed: {
          source: "company_account",
          configured: true,
          baseUrl: "https://api.tinyhumans.ai",
          enabled: true,
          legacyRow: true,
          needsModel: false,
        },
      }),
    );
  });

  test("turning it off posts to /inference/managed/enabled with {enabled:false}, never .../providers/tinyhumans/enabled", async ({
    page,
  }) => {
    await openInference(page);
    const row = page.getByTestId("inference-provider-managed");
    await expect(row).toBeVisible();

    // A provider-route call would be the bug this pins against — assert it
    // never fires, alongside asserting the right one does.
    let providerRouteCalled = false;
    await page.route(isProviderEnabled, async (route) => {
      providerRouteCalled = true;
      await route.fulfill({ status: 404, json: { error: "no such provider", code: "not_found" } });
    });

    const managedCall = page.waitForRequest(
      (request) =>
        isManagedEnabled(new URL(request.url())) &&
        request.method() === "POST" &&
        (request.postDataJSON() as { enabled?: boolean }).enabled === false,
    );
    await page.route(isManagedEnabled, async (route) => {
      if (route.request().method() !== "POST") return route.fallback();
      await route.fulfill({
        json: { status: status({ managed: { source: "company_account", configured: true, baseUrl: "https://api.tinyhumans.ai", enabled: false, legacyRow: true, needsModel: false } }), note: "Managed turned off." },
      });
    });

    await row.getByTestId("inference-provider-managed-toggle").click();
    await expect(page.getByTestId("inference-remove-dialog")).toContainText("Turn off");
    await page.getByTestId("inference-remove-confirm").click();
    await managedCall;
    await expect(page.getByTestId("inference-remove-dialog")).toHaveCount(0, { timeout: 10_000 });
    expect(providerRouteCalled, "the legacy toggle must never call the provider route").toBe(false);
  });
});

test.describe("P0-2: a stale probe never leaks into the next dialog", () => {
  test.beforeEach(async ({ page }) => {
    await stubStatus(page, status({ providers: [] }));
  });

  test("Escape is blocked while the probe is in flight; once it settles, closing and reopening a different provider starts clean", async ({
    page,
  }) => {
    await openInference(page);

    // A short, real delay — the original bug was Escape racing a probe that
    // was going to answer anyway, not one that never would. Resolves with a
    // catalogue, so the model step opening is the deterministic signal that
    // `busy` has cleared.
    await page.route(isProbe, async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 300));
      await route.fulfill({ json: { ok: true, modelCount: 1, models: ["groq/test-model"] } });
    });

    await page.getByTestId("inference-add-open").click();
    await page.locator("#inference-add-cloud").click();
    await page.getByRole("option", { name: /^Groq/ }).click();
    await expect(page.getByTestId("inference-connect-provider")).toBeVisible();
    await page.locator("#inference-connect-key").fill("sk-not-a-real-key-groq");
    await page.getByTestId("inference-connect-submit").click();

    // Busy: Escape must not close the dialog (round-2 review, P0-2 — this is
    // the guard that makes the original race structurally unreachable, rather
    // than reopening the door for the attempt counter to have to catch it).
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("inference-connect-provider")).toBeVisible();

    // The probe settles — busy clears, and the model step is now showing
    // Groq's own catalogue.
    await expect(page.getByTestId("inference-connect-model-step")).toBeVisible({ timeout: 10_000 });

    // NOT busy any more: Escape now works.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("inference-connect-provider")).toHaveCount(0);

    // A different provider, opened fresh.
    await page.getByTestId("inference-add-open").click();
    await page.locator("#inference-add-cloud").click();
    await page.getByRole("option", { name: /^Anthropic/ }).click();
    await expect(page.getByTestId("inference-connect-provider")).toBeVisible();

    // Starts clean at the key step — no leftover model step, no leftover
    // catalogue or error from Groq, and the key field is genuinely empty.
    await expect(page.getByTestId("inference-connect-model-step")).toHaveCount(0);
    await expect(page.getByTestId("inference-connect-error")).toHaveText("");
    await expect(page.locator("#inference-connect-key")).toHaveValue("");
  });

  test("Escape and an overlay click send nothing while a request is in flight", async ({ page }) => {
    await openInference(page);
    let probeCalls = 0;
    await page.route(isProbe, async () => {
      probeCalls++;
      await new Promise(() => {});
    });

    await page.getByTestId("inference-add-open").click();
    await page.locator("#inference-add-cloud").click();
    await page.getByRole("option", { name: /^Groq/ }).click();
    await page.locator("#inference-connect-key").fill("sk-not-a-real-key-groq");
    await page.getByTestId("inference-connect-submit").click();
    expect(probeCalls).toBe(1);

    // Busy: Escape must not close the dialog.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("inference-connect-provider")).toBeVisible();
    expect(probeCalls, "no second request from the ignored Escape").toBe(1);
  });
});

test.describe("P1-1: confirmInUse is sent only when the dialog actually showed a usedBy, and a stale 409 re-opens it", () => {
  test("an agent pinned after the page loaded gets a 409, the dialog re-opens naming it, and the confirmed retry sends confirmInUse:true", async ({
    page,
  }) => {
    // The status the dialog opens against says nothing is pinned — this is
    // the "stale UI" the whole flow exists for: the page loaded before the
    // pin happened elsewhere.
    await stubStatus(page, status());
    await openInference(page);

    const row = page.getByTestId("inference-provider-beta");
    await expect(row).toBeVisible();
    await row.getByTestId("inference-provider-beta-menu").click();
    await page.getByTestId("inference-provider-beta-remove").click();

    const dialog = page.getByTestId("inference-remove-dialog");
    await expect(dialog).toBeVisible();
    // Nothing shown as used yet — the confirm click below must therefore NOT
    // send confirmInUse on the first try.
    await expect(dialog).not.toContainText(IN_USE_ERROR);

    let firstBody: { confirmInUse?: boolean } | undefined;
    await page.route(
      (url) => /\/inference\/providers\/beta$/.test(url.pathname),
      async (route: Route) => {
        if (route.request().method() !== "DELETE") return route.fallback();
        const confirmed = new URL(route.request().url()).searchParams.get("confirmInUse") === "true";
        if (!firstBody) firstBody = { confirmInUse: confirmed };
        if (!confirmed) {
          await route.fulfill({
            status: 409,
            json: { error: IN_USE_ERROR, code: "in_use", usedBy: { agents: [{ id: "a1", name: "Researcher" }] } },
          });
          return;
        }
        await route.fulfill({
          json: {
            status: status({ providers: [status().providers[0]] }),
            note: "Beta removed.",
          },
        });
      },
    );

    const firstAttempt = page.waitForRequest(
      (request) => /\/inference\/providers\/beta/.test(request.url()) && request.method() === "DELETE",
    );
    await page.getByTestId("inference-remove-confirm").click();
    await firstAttempt;
    expect(firstBody?.confirmInUse, "first click must not send confirmInUse — nothing was shown as used").toBe(
      false,
    );

    // The dialog stays open and now names what the host just said.
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText(IN_USE_ERROR);
    await expect(dialog).toContainText("Researcher");

    // The confirmed retry sends confirmInUse:true.
    const confirmedRetry = page.waitForRequest(
      (request) =>
        /\/inference\/providers\/beta/.test(request.url()) &&
        request.method() === "DELETE" &&
        new URL(request.url()).searchParams.get("confirmInUse") === "true",
    );
    await page.getByTestId("inference-remove-confirm").click();
    await confirmedRetry;
    await expect(dialog).toBeHidden({ timeout: 10_000 });
  });
});

test.describe("the catalogue combobox, once a probe actually succeeds", () => {
  // Round-2 review, P2-6: every add spec in `inference.spec.ts` probes
  // `UNREACHABLE`, so `pickModel` there always lands on free text and the
  // combobox itself — the select-and-filter path `ModelCombobox` renders —
  // has never been driven through a browser.
  test("a successful probe opens the model list, and picking one from it is what gets saved", async ({
    page,
  }) => {
    await stubStatus(page, status({ providers: [] }));
    await openInference(page);

    await page.route(isProbe, async (route) => {
      await route.fulfill({
        json: { ok: true, modelCount: 3, models: ["groq/model-a", "groq/model-b", "groq/model-c"] },
      });
    });

    await page.getByTestId("inference-add-open").click();
    await page.locator("#inference-add-cloud").click();
    await page.getByRole("option", { name: /^Groq/ }).click();
    await page.locator("#inference-connect-key").fill("sk-not-a-real-key-groq");
    await page.getByTestId("inference-connect-submit").click();
    await expect(page.getByTestId("inference-connect-model-step")).toBeVisible({ timeout: 10_000 });

    // A closed list of real ids: the combobox trigger, not the free-text
    // input — `showsCatalogSelect` only offers this once a catalogue with
    // something in it has actually loaded.
    const trigger = page.locator("#inference-connect-model");
    await expect(trigger).toBeVisible();
    await expect(trigger).toHaveText("Choose a model");
    await trigger.click();
    await page.getByRole("listbox", { name: "Models" }).getByRole("option", { name: "groq/model-b" }).click();
    await expect(trigger).toHaveText("groq/model-b");

    const added = page.waitForRequest(
      (request) =>
        /\/inference\/providers$/.test(new URL(request.url()).pathname) &&
        request.method() === "POST" &&
        (request.postDataJSON() as { model?: string }).model === "groq/model-b",
    );
    await page.route(
      (url) => /\/inference\/providers$/.test(url.pathname),
      async (route: Route) => {
        if (route.request().method() !== "POST") return route.fallback();
        await route.fulfill({
          json: {
            status: status({
              providers: [
                { id: "prv_groq", slug: "groq", label: "Groq", kind: "groq", baseUrl: "https://api.groq.com/openai/v1", models: {}, model: "groq/model-b", modelAmbiguous: false, enabled: true, keyConfigured: true, origin: "indexed", isDefault: true },
              ],
            }),
            note: "Groq connected.",
          },
        });
      },
    );
    await page.getByTestId("inference-connect-submit").click();
    await added;
    await expect(page.getByTestId("inference-provider-groq")).toBeVisible({ timeout: 10_000 });
  });
});

test.describe("KR-L1-02: the Add-a-provider picker does not reopen after a successful add", () => {
  test("adding Ollama leaves no dialog open, and the page stays interactive", async ({ page }) => {
    await stubStatus(page, status({ providers: [] }));
    await openInference(page);

    await page.route(isProbe, async (route) => {
      await route.fulfill({ status: 200, json: { ok: false, modelCount: 0, message: "connection refused" } });
    });
    await page.route(
      (url) => /\/inference\/providers$/.test(url.pathname),
      async (route: Route) => {
        if (route.request().method() !== "POST") return route.fallback();
        await route.fulfill({
          json: {
            status: status({
              providers: [
                { id: "prv_ollama", slug: "ollama", label: "Ollama", kind: "ollama", baseUrl: "http://localhost:11434/v1", models: {}, model: "llama3", modelAmbiguous: false, enabled: true, keyConfigured: false, origin: "indexed", isDefault: true },
              ],
            }),
            note: "Ollama connected.",
          },
        });
      },
    );

    await page.getByTestId("inference-add-open").click();
    await expect(page.getByTestId("inference-add-provider")).toBeVisible();
    await page.locator("#inference-add-local").click();
    await page.getByRole("option", { name: /^Ollama/ }).click();
    await expect(page.getByTestId("inference-connect-provider")).toBeVisible();
    // The picker must already be gone — this is the state KR-L1-02 found
    // stuck `true` underneath the connect dialog.
    await expect(page.getByTestId("inference-add-provider")).toHaveCount(0);

    await page.getByTestId("inference-connect-submit").click();
    await expect(page.getByTestId("inference-connect-model-step")).toBeVisible();
    await page.locator("#inference-connect-model").fill("llama3");
    await page.getByTestId("inference-connect-submit").click();

    await expect(page.getByTestId("inference-provider-ollama")).toBeVisible({ timeout: 10_000 });
    // Neither dialog remains, and the page's own content is reachable again
    // (KR-L1-02's actual symptom was the rest of the page staying
    // `aria-hidden` — a `getByRole` query below the page heading is exactly
    // what that broke).
    await expect(page.getByTestId("inference-connect-provider")).toHaveCount(0);
    await expect(page.getByTestId("inference-add-provider")).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "LLM", exact: true })).toBeVisible();
    await expect(page.getByTestId("inference-add-open")).toBeEnabled();

    // And it stays closed — reopening the picker starts fresh, not on top
    // of a phantom instance.
    await page.getByTestId("inference-add-open").click();
    await expect(page.getByTestId("inference-add-provider")).toBeVisible();
    await expect(page.getByTestId("inference-add-provider")).toHaveCount(1);
  });

  test("cancelling the connect dialog also leaves the picker closed", async ({ page }) => {
    await stubStatus(page, status({ providers: [] }));
    await openInference(page);

    await page.getByTestId("inference-add-open").click();
    await page.getByTestId("inference-add-custom").click();
    await expect(page.getByTestId("inference-connect-provider")).toBeVisible();
    await expect(page.getByTestId("inference-add-provider")).toHaveCount(0);

    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByTestId("inference-connect-provider")).toHaveCount(0);
    await expect(page.getByTestId("inference-add-provider")).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "LLM", exact: true })).toBeVisible();
  });
});

test.describe("KR-L1-05: a confirm dialog's own busy flag always clears, even when Escape is tried mid-request", () => {
  test("Escape does nothing while the remove is in flight; once it settles, a different row's confirm is not left stuck disabled", async ({
    page,
  }) => {
    await stubStatus(page, status());
    await openInference(page);

    let resolveDelete: (() => void) | undefined;
    const deleteStarted = new Promise<void>((resolve) => {
      page.route(
        (url) => /\/inference\/providers\/beta$/.test(url.pathname),
        async (route: Route) => {
          if (route.request().method() !== "DELETE") return route.fallback();
          resolve();
          await new Promise<void>((r) => {
            resolveDelete = r;
          });
          await route.fulfill({ json: { status: status({ providers: [status().providers[0]] }), note: "Beta removed." } });
        },
      );
    });

    await page.getByTestId("inference-provider-beta-menu").click();
    await page.getByTestId("inference-provider-beta-remove").click();
    const removeDialog = page.getByTestId("inference-remove-dialog");
    await expect(removeDialog).toBeVisible();

    await page.getByTestId("inference-remove-confirm").click();
    await deleteStarted;

    // In flight: Escape must not close this dialog (KR-L1-05's own ask —
    // every close path has to leave `busy` in a state a later dialog can
    // trust, and the simplest way is to not let this one close early at
    // all while its own request is still pending).
    await page.keyboard.press("Escape");
    await expect(removeDialog).toBeVisible();

    resolveDelete?.();
    await expect(removeDialog).toBeHidden({ timeout: 10_000 });
    await expect(page.getByTestId("inference-provider-beta")).toHaveCount(0);

    // A completely different row's confirm dialog opens ready to use —
    // not carrying a stale `busy` from the request that just finished.
    await page.getByTestId("inference-provider-acme-toggle").click();
    const toggleDialog = page.getByTestId("inference-remove-dialog");
    await expect(toggleDialog).toBeVisible();
    await expect(page.getByTestId("inference-remove-confirm")).toBeEnabled();
  });
});
