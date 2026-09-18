import { expect, test } from "@playwright/test";

/**
 * The TinyHumans key-grant ("hub link") flow, end to end, against a real
 * backend fronted by the backend's own local sign-in stand-in
 * (`backend/src/scripts/localKeyGrantHub.ts`).
 *
 * The sibling `tinyhumans-account-key.spec.ts` pastes a key. This one never
 * sees a key at all — which is the point of the grant: the host starts a PKCE
 * grant (`POST …/credential/link/start`), the browser is sent to the hub's
 * `GET /auth/key`, signs in, approves the real consent page, and comes back
 * to the console with a one-time `code`; the host redeems it at the hub's
 * `POST /auth/keys` and stores the minted key it — and only it — ever holds.
 * The stub replaces exactly one step, the Google sign-in: it answers
 * `GET /auth/key` as a fixed seeded user, mints the grant through the
 * backend's own `startKeyGrant`, and renders the backend's own consent page.
 * Everything else — the redeem, the proxy catalog probe, the fan-out, the
 * chat turn — is the real backend.
 *
 * What it proves, beyond the paste lane:
 *  1. a grant landing on the console is redeemed and the key stored, with
 *     the console never having handled the key;
 *  2. a grant answers `needsModel` (a grant carries no model), and the
 *     console opens the model step off the stored key
 *     (`PUT …/credential/model`) instead of dead-ending on "choose a model";
 *  3. the finished company is rebuilt live and a turn reaches the backend.
 *
 * Skipped unless `PW_TINYHUMANS_MODEL` is set and `PW_TINYHUMANS_LINK=1`:
 * the umbrella's `scripts/e2e-tinyhumans-key.sh` sets both once the stub hub
 * is up and the host's `TINYHUMANS_API_URL` points at it.
 */

const MODEL = process.env.PW_TINYHUMANS_MODEL ?? "";
const LINK = process.env.PW_TINYHUMANS_LINK === "1";

test.skip(
  !LINK || !MODEL,
  "PW_TINYHUMANS_LINK=1 and PW_TINYHUMANS_MODEL: the host must be pointed at the local " +
    "key-grant hub stub; the umbrella's scripts/e2e-tinyhumans-key.sh sets both.",
);

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

type Page = import("@playwright/test").Page;

const toasts = (page: Page) => page.locator("[data-sonner-toast]");

async function open(page: Page, hash: string): Promise<void> {
  await page.goto(hash);
  const skip = page.getByRole("button", { name: "Skip for now" });
  await skip
    .first()
    .waitFor({ state: "visible", timeout: 3_000 })
    .catch(() => {});
  let dismissed = false;
  for (let attempt = 0; attempt < 5; attempt += 1) {
    if (!(await skip.isVisible().catch(() => false))) break;
    dismissed = true;
    await skip.click({ force: true }).catch(() => {});
    await page.waitForTimeout(300);
  }
  if (dismissed) await page.goto(hash);
}

test("a key grant from the hub sets up TinyHumans without the console ever seeing the key", async ({
  page,
}) => {
  const before = await page.request.get("/api/v1/company/inference");
  expect(before.ok()).toBeTruthy();
  expect(((await before.json()) as { cognition: string }).cognition).toBe("echo");

  const status = await page.request.get("/api/v1/company/credential");
  const credential = (await status.json()) as { hubLink: boolean; configured: boolean };
  expect(credential.hubLink, "the host must be wired to a hub (tinyhumans feature)").toBe(true);
  expect(credential.configured).toBe(false);

  // 1. Start the grant the way the hub site does, with this browser's session,
  //    and follow the authorize URL to the hub.
  await open(page, "/#/connections/api-key");
  const started = await page.request.post("/api/v1/company/credential/link/start", {
    headers: { origin: new URL(page.url()).origin },
  });
  expect(started.ok(), await started.text()).toBeTruthy();
  const { authorizeUrl } = (await started.json()) as { authorizeUrl: string };
  expect(authorizeUrl).toContain("/auth/key?");
  expect(authorizeUrl).toContain("code_challenge_method=S256");

  const consoleOrigin = new URL(page.url()).origin;
  await page.goto(authorizeUrl);
  // 2. The backend's own consent page: names the requesting origin and the
  //    scopes; "Connect" is the approve link back to the console.
  await expect(
    page.getByRole("heading", { name: "Give this company a TinyHumans key?" }),
  ).toBeVisible();
  await expect(page.getByText(consoleOrigin).first()).toBeVisible();
  await page.getByRole("link", { name: "Connect" }).click();

  // 3. Back on the console. The landing strips `?key=link&state=&code=` and
  //    stashes the grant for the Account page, which redeems it. A hash
  //    change reaches that page without a reload (a reload would empty the
  //    stash — the code is single-use and must not survive a refresh).
  await page.waitForURL((url) => url.origin === consoleOrigin);
  await page.evaluate(() => {
    window.location.hash = "#/connections/api-key";
  });
  await expect(toasts(page).filter({ hasText: /Connected to TinyHumans/ }).first()).toBeVisible({
    timeout: 30_000,
  });

  // 4. A grant has no model step of its own, so the host answered
  //    `needsModel` and the console opened step two — off the STORED key.
  const modelStep = page.getByTestId("account-key-model-step");
  await expect(modelStep).toBeVisible({ timeout: 30_000 });
  await modelStep.locator("#account-key-model").click();
  const option = page.getByRole("option", { name: new RegExp(`^${escapeRegExp(MODEL)}`) });
  await expect(option).toBeVisible({ timeout: 10_000 });
  await option.click();
  await page.getByTestId("account-key-model-save").click();

  // Not `/Key saved/`: the redeem's own toast note already says "Key saved.
  // Composio now uses this key…", and matching it would read the status
  // before step two has landed. The model-step note is the one that names the
  // model.
  const saved = toasts(page)
    .filter({ hasText: `TinyHumans is set up for LLM with ${MODEL}` })
    .first();
  await expect(saved).toBeVisible({ timeout: 30_000 });
  await expect(saved).not.toContainText("restart required");

  const after = await page.request.get("/api/v1/company/inference");
  const afterStatus = (await after.json()) as {
    cognition: string;
    restartRequired: boolean;
    defaultChoice: { provider: string; model: string; broken: boolean } | null;
    providers: { slug: string; baseUrl: string; enabled: boolean; keyConfigured: boolean }[];
  };
  expect(afterStatus.cognition).toBe("harness");
  expect(afterStatus.restartRequired).toBe(false);
  expect(afterStatus.defaultChoice).toEqual({ provider: "tinyhumans", model: MODEL, broken: false });
  const row = afterStatus.providers.find((p) => p.slug === "tinyhumans");
  expect(row?.keyConfigured).toBe(true);
  expect(row?.baseUrl).toMatch(/\/agent-integrations\/openrouter$/);

  const finalCredential = (await (await page.request.get("/api/v1/company/credential")).json()) as {
    configured: boolean;
    source: string;
  };
  expect(finalCredential).toMatchObject({ configured: true, source: "company" });

  // 5. A turn goes through the backend on the granted key.
  await open(page, "/#/chat");
  const backendReplies = page.locator("article[data-message-id]").filter({ hasText: /MOCK_LLM/ });
  const replyCountBefore = await backendReplies.count();
  await page.getByPlaceholder(/^Message /).fill(`tinyhumans key grant e2e ${Date.now()}`);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  // Scoped to rendered message bubbles and counted before/after — the same
  // pattern as `tinyhumans-account-key.spec.ts` — so this proves the *new*
  // turn reached the backend rather than matching an older visible MOCK_LLM
  // reply already in the transcript (CodeRabbit review).
  await expect(backendReplies).toHaveCount(replyCountBefore + 1, { timeout: 90_000 });
  await expect(page.getByText(/^You said:/)).toHaveCount(0);
});

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
