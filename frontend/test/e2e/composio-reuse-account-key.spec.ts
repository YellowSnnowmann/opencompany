import { expect, test } from "@playwright/test";

/**
 * The Composio half of the account-key reuse banner, fully mocked (keys
 * rework, issue #2306, slice 4c — `docs/key-reworks/phase-4c-reuse-banner.md`).
 * Follows `composio-mode-switch.spec.ts`'s pattern: a running host
 * (`playwright.config.ts` brings one up), with the handful of routes under
 * test answered by this file instead of a real TinyHumans/Composio backend.
 * No real credential anywhere — the fake key never reaches the network, since
 * the route it would be checked against is intercepted.
 *
 * A separate spec (a different, concurrent dispatch's file, under
 * `frontend/src/inference/**`'s scope) covers the LLM page's own banner —
 * out of scope here.
 */

type Page = import("@playwright/test").Page;
type Route = import("@playwright/test").Route;

const isComposioStatus = (url: URL) => /\/composio$/.test(url.pathname);
const isCredential = (url: URL) => /\/credential$/.test(url.pathname);
const isFromAccount = (url: URL) =>
  /\/composio\/tinyhumans\/key\/from-account$/.test(url.pathname);

/** A managed-route status with nothing of its own in the managed slot. */
function managedStatusNeedingAKey() {
  return {
    inBuild: true,
    granted: true,
    credentialSource: "none",
    managedCredentialSource: "none",
    mode: "managed",
    backendUrl: "https://api.tinyhumans.ai",
    toolkits: ["gmail"],
    openMode: false,
    effectiveToolkits: ["gmail"],
    effectiveCatalog: [],
    catalogSource: "manifest",
    catalogNotice: null,
  };
}

/**
 * The same, once the account key has filled the managed slot.
 *
 * `static`, not `company`: the copy writes the account key's literal value
 * into `composio/tinyhumans/key`, and `resolve_credential` cannot tell that
 * value apart from a token pasted directly — both read back as
 * `Credential::Value`, which reports as `static`
 * (`src/company/credentials.rs`). This is exactly why the banner's visibility
 * rule excludes `static`: the fill is what makes the condition go false.
 */
function managedStatusFilled() {
  return {
    ...managedStatusNeedingAKey(),
    credentialSource: "static",
    managedCredentialSource: "static",
  };
}

/** A `GET …/credential` status, shaped like the real `CredentialStatusDto`. */
function credentialStatus(over: Record<string, unknown> = {}) {
  return {
    configured: true,
    source: "company",
    notice: "This is the company's TinyHumans account key.",
    hubLink: false,
    inferenceHasOwnKey: false,
    composioHasOwnKey: false,
    searchHasOwnKey: false,
    defaultSet: false,
    ...over,
  };
}

async function stubCredential(
  page: Page,
  status: Record<string, unknown>,
): Promise<void> {
  await page.route(isCredential, async (route: Route) => {
    if (route.request().method() !== "GET") return route.fallback();
    await route.fulfill({ json: status });
  });
}

/** Open the Composio page with the first-run tour out of the way. */
async function openComposio(page: Page): Promise<void> {
  await page.goto("/#/connections/composio");
  const skip = page.getByRole("button", { name: "Skip for now" });
  await skip
    .waitFor({ state: "visible", timeout: 10_000 })
    .then(() => skip.click())
    .catch(() => {
      /* already dismissed in this context */
    });
  await expect(skip).toBeHidden({ timeout: 10_000 });
  await expect(page.getByRole("heading", { name: "Connected" })).toBeVisible({
    timeout: 30_000,
  });
}

const toasts = (page: Page) => page.locator("[data-sonner-toast]");
const banner = (page: Page) =>
  page.getByTestId("composio-reuse-account-key-banner");

test.beforeEach(async ({ page }) => {
  await stubCredential(page, credentialStatus());
});

test("Yes copies the account key, toasts the host's note, and the banner is gone", async ({
  page,
}) => {
  // Stateful: `GET .../composio` answers "needs a key" until the copy lands,
  // then "filled" — because a successful write calls `onChanged()`, and
  // `ComposioSection`'s own doc comment on `settle()` notes this remounts the
  // section (a fresh `refresh()`), which would otherwise re-fetch the stale
  // "needs a key" status a single fixed stub would keep answering with.
  let filled = false;
  await page.route(isComposioStatus, async (route: Route) => {
    if (route.request().method() !== "GET") return route.fallback();
    await route.fulfill({
      json: filled ? managedStatusFilled() : managedStatusNeedingAKey(),
    });
  });
  await page.route(isFromAccount, async (route: Route) => {
    if (route.request().method() !== "POST") return route.fallback();
    filled = true;
    await route.fulfill({
      json: {
        status: managedStatusFilled(),
        note:
          "Composio now uses your account key. A key you created by hand may lack the connections permission Composio needs.",
        slots: [{ slot: "composio", outcome: "filled" }],
      },
    });
  });

  await openComposio(page);

  const banner1 = banner(page);
  await expect(banner1).toBeVisible();
  await expect(banner1).toContainText(
    "Your TinyHumans account is connected. Use the same key for Composio?",
  );

  const request = page.waitForRequest(
    (r) => isFromAccount(new URL(r.url())) && r.method() === "POST",
  );
  await page.getByTestId("composio-reuse-account-key-banner-yes").click();
  await request;

  const message = toasts(page).first();
  await expect(message).toBeVisible({ timeout: 10_000 });
  await expect(message).toContainText("Composio now uses your account key.");

  await expect(banner(page)).toHaveCount(0);
});

test("Not now hides the banner, and it stays hidden after a reload", async ({
  page,
}) => {
  await page.route(isComposioStatus, async (route: Route) => {
    if (route.request().method() !== "GET") return route.fallback();
    await route.fulfill({ json: managedStatusNeedingAKey() });
  });

  await openComposio(page);

  await expect(banner(page)).toBeVisible();
  await page.getByTestId("composio-reuse-account-key-banner-not-now").click();
  await expect(banner(page)).toHaveCount(0);

  await page.reload();
  await expect(page.getByRole("heading", { name: "Connected" })).toBeVisible({
    timeout: 30_000,
  });
  await expect(banner(page)).toHaveCount(0);
});
