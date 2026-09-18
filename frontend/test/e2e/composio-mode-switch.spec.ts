import { expect, test } from "@playwright/test";

/**
 * Keys rework (issue #2306) — every destructive action and every on/off
 * toggle on the Composio page gets a confirm dialog, and the host refuses an
 * in-use clear/switch unless the request confirms it
 * (`docs/key-reworks/in-use-guards.md`).
 *
 * # Two gaps this closes
 *
 * `ComposioSection` already asked before the FIRST move onto BYOK (an inline
 * warning inside the credential dialog, `confirmSwitch`). It asked nothing at
 * all before clearing the managed token, or before giving the managed route
 * back — both used to write immediately on click. Those two are what this
 * file drives.
 *
 * # Fully mocked, no live Composio
 *
 * Every request this spec cares about is stubbed at the wire with
 * `page.route`, following `connections-native-not-offered.spec.ts`'s pattern:
 * a running host (`playwright.config.ts` brings one up), with the handful of
 * routes under test answered by this file instead of a real backend. No
 * `COMPOSIO` fixture, no network to Composio, and no real credential — the
 * pasted values below are obviously fake
 * (`th-not-a-real-key`/`ak-not-a-real-key`) and the host never dials out for
 * them because the PUT handlers themselves are intercepted.
 */

type Page = import("@playwright/test").Page;
type Route = import("@playwright/test").Route;

/** The status route, and only it — not `.../token`, `.../api-key`, `.../connections`. */
const isComposioStatus = (url: URL) => /\/composio$/.test(url.pathname);
const isComposioToken = (url: URL) => /\/composio\/token$/.test(url.pathname);
const isComposioApiKey = (url: URL) => /\/composio\/api-key$/.test(url.pathname);

const IN_USE_ERROR = "Composio's key is used by Composio.";
const IN_USE_BODY = {
  error: IN_USE_ERROR,
  code: "in_use",
  usedBy: { surfaces: ["composio"] as const },
};

/** A status where the managed route is active and holds a stored token. */
function managedStatus() {
  return {
    inBuild: true,
    granted: true,
    credentialSource: "static",
    managedCredentialSource: "static",
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
 * A status where BYOK is active — the managed chain still resolves
 * (`managedCredentialSource: "attested"`), so the managed row offers "Use
 * this" rather than hiding it as a switch into an outage.
 */
function byokStatus() {
  return {
    inBuild: true,
    granted: true,
    credentialSource: "static",
    managedCredentialSource: "attested",
    mode: "byok",
    backendUrl: "https://backend.composio.dev",
    toolkits: ["gmail"],
    openMode: false,
    effectiveToolkits: ["gmail"],
    effectiveCatalog: [],
    catalogSource: "manifest",
    catalogNotice: null,
  };
}

/** Stub `GET .../composio` to answer with a fixed status, every time it is asked. */
async function stubStatus(page: Page, status: Record<string, unknown>): Promise<void> {
  await page.route(
    (url) => isComposioStatus(url),
    async (route: Route) => {
      if (route.request().method() !== "GET") return route.fallback();
      await route.fulfill({ json: status });
    },
  );
}

/**
 * Stub a guarded PUT route (`.../token` or `.../api-key`) to answer 409
 * `in_use` on a request that does not confirm, and 200 (echoing `usedBy`) on
 * one that does — exactly the host's own guard behaviour
 * (`src/server/ops/composio.rs`), without a real backend behind it.
 */
async function stubGuardedRoute(
  page: Page,
  matches: (url: URL) => boolean,
  okStatus: Record<string, unknown>,
  okNote: string,
): Promise<void> {
  await page.route(
    (url) => matches(url),
    async (route: Route) => {
      if (route.request().method() !== "PUT") return route.fallback();
      const body = route.request().postDataJSON() as { confirmInUse?: boolean };
      if (body.confirmInUse !== true) {
        await route.fulfill({ status: 409, json: IN_USE_BODY });
        return;
      }
      await route.fulfill({
        json: {
          status: okStatus,
          note: okNote,
          usedBy: { surfaces: ["composio"] },
        },
      });
    },
  );
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

test.describe("clearing the managed-route token", () => {
  test.beforeEach(async ({ page }) => {
    await stubStatus(page, managedStatus());
    await stubGuardedRoute(page, isComposioToken, managedStatus(), "Composio token cleared.");
  });

  test("shows the token is in use up front (mode: managed), and the first click already confirms (round-3 review, P1-2)", async ({
    page,
  }) => {
    await openComposio(page);

    await page.getByTestId("composio-row-managed-remove").click();

    const dialog = page.getByTestId("composio-clear-token-dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("heading", { name: "Disconnect Composio?" })).toBeVisible();
    // Named on the very first open — `status.mode` already reads "managed",
    // and that alone is the host's own guard rule, so there is nothing to
    // wait for a refusal to learn.
    await expect(dialog).toContainText("Composio uses this key.");

    const confirm = page.getByTestId("composio-clear-token-confirm");
    await expect(confirm).toHaveText(/Disconnect Composio/);

    const confirmed = page.waitForRequest(
      (request) =>
        isComposioToken(new URL(request.url())) &&
        request.method() === "PUT" &&
        (request.postDataJSON() as { confirmInUse?: boolean }).confirmInUse === true,
    );
    await confirm.click();
    await confirmed;
    await expect(dialog).toBeHidden({ timeout: 10_000 });
  });

  test("a stale read: mode said nothing at open, still gets a 409, and the confirmed retry succeeds", async ({
    page,
  }) => {
    // `Remove` only ever renders while `mode` reads "managed" — the row's own
    // `removeKey: onManaged && managedTokenStored` (`composioRows`, pinned by
    // `test/unit/composio-rows.test.ts`'s "never rotates or removes a
    // credential on the row a company is not on") — so the mount's own read
    // has to stay "managed" for this test to reach the row at all. The
    // staleness this test is actually about is the read `requestClearManagedToken`
    // takes the moment `Remove` is clicked (P1-2's "re-read on open"): swapped
    // in here, after mount, to "byok" — a switch that landed between this
    // page's mount and this click — so the console's own optimism
    // (`composioUsesThisKey`) has nothing to go on, and only the real
    // backend's own check (stubbed 409 below) still knows the token is used.
    //
    // `openComposio` runs first, against `beforeEach`'s "managed" stub, so
    // mount sees the row and renders `Remove` — only THEN does the stub swap
    // to "byok", ahead of the click that triggers the re-read.
    await openComposio(page);
    await stubStatus(page, { ...managedStatus(), mode: "byok" });

    await page.getByTestId("composio-row-managed-remove").click();

    const dialog = page.getByTestId("composio-clear-token-dialog");
    await expect(dialog).toBeVisible();
    // Nothing shown as used yet — the re-read this dialog opened with says
    // mode is byok, not managed.
    await expect(dialog).not.toContainText("Composio uses this key.");
    await expect(dialog).not.toContainText(IN_USE_ERROR);

    const confirm = page.getByTestId("composio-clear-token-confirm");

    // First click: unconfirmed, since nothing here said the token was in use.
    // The stub still answers 409 (the real backend's own check is
    // authoritative regardless of what any GET read said), and the dialog
    // reopens naming it.
    const firstAttempt = page.waitForRequest(
      (request) => isComposioToken(new URL(request.url())) && request.method() === "PUT",
    );
    await confirm.click();
    const first = await firstAttempt;
    expect((first.postDataJSON() as { confirmInUse?: boolean }).confirmInUse).not.toBe(true);
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText(IN_USE_ERROR);

    // Second click, now informed: sends `confirmInUse: true` and succeeds.
    const confirmed = page.waitForRequest(
      (request) =>
        isComposioToken(new URL(request.url())) &&
        request.method() === "PUT" &&
        (request.postDataJSON() as { confirmInUse?: boolean }).confirmInUse === true,
    );
    await confirm.click();
    await confirmed;
    await expect(dialog).toBeHidden({ timeout: 10_000 });
  });

  test("Cancel leaves the dialog with nothing sent", async ({ page }) => {
    await openComposio(page);

    let calls = 0;
    await page.route(isComposioToken, async (route) => {
      calls++;
      await route.fulfill({ status: 409, json: IN_USE_BODY });
    });

    await page.getByTestId("composio-row-managed-remove").click();
    await expect(page.getByTestId("composio-clear-token-dialog")).toBeVisible();

    await page.getByRole("button", { name: "Keep the token" }).click();
    await expect(page.getByTestId("composio-clear-token-dialog")).toBeHidden();
    expect(calls, "Cancel must send no request at all").toBe(0);
  });
});

test.describe("switching Composio back to the managed route", () => {
  test.beforeEach(async ({ page }) => {
    await stubStatus(page, byokStatus());
    await stubGuardedRoute(
      page,
      isComposioApiKey,
      managedStatus(),
      "Composio API key cleared.",
    );
  });

  test("shows the key is in use up front (mode: byok), and the first click already confirms (round-3 review, P1-2)", async ({
    page,
  }) => {
    await openComposio(page);

    // The managed row's radio is this route's own "Use this" — see
    // `composioRows`: it is offered because `managedCredentialSource` here
    // resolves ("attested"), so choosing it is not a switch into an outage.
    await page.getByTestId("composio-row-managed-select").click();

    const dialog = page.getByTestId("composio-use-managed-dialog");
    await expect(dialog).toBeVisible();
    await expect(
      dialog.getByRole("heading", {
        name: "Switch Composio to the TinyHumans-managed route?",
      }),
    ).toBeVisible();
    // Named on the very first open — `status.mode` already reads "byok".
    await expect(dialog).toContainText("Composio uses this key.");

    const confirm = page.getByTestId("composio-use-managed-confirm");

    // The first click already sends `confirmInUse: true` and an empty
    // `apiKey` — the clear-and-switch-back this route always performs
    // together.
    const confirmed = page.waitForRequest((request) => {
      if (!isComposioApiKey(new URL(request.url())) || request.method() !== "PUT") {
        return false;
      }
      const body = request.postDataJSON() as { apiKey?: string; confirmInUse?: boolean };
      return body.confirmInUse === true && body.apiKey === "";
    });
    await confirm.click();
    await confirmed;
    await expect(dialog).toBeHidden({ timeout: 10_000 });
  });
});
