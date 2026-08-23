import { expect, test, type Page } from "@playwright/test";

/**
 * Proof for issue #1476: Settings → Devices exists and reaches the host.
 *
 * The gap this closes was not a broken route. `GET/POST …/devices` has been
 * served since device pairing landed, with backend tests over it — what was
 * missing was any caller. The desktop app pointed people at "Settings →
 * devices", a repo-wide grep of `frontend/src` found no call to either route,
 * and the sub-page did not exist. So the interesting assertion is not that the
 * page renders: it is that pressing the button on it produces a code minted by
 * the real host, which only running against one can show.
 *
 * That is also why this spec is here rather than in the unit suite. The unit
 * tests stub the client and prove the console asks for the right paths; nothing
 * in them would notice if those paths were wrong.
 */

/**
 * A fresh host greets the first visit with a welcome tour rendered over the
 * console, which swallows clicks on the view beneath it.
 */
async function dismissOnboarding(page: Page) {
  const skip = page.getByRole("button", { name: "Skip for now" });
  await skip.waitFor({ state: "visible", timeout: 15_000 }).catch(() => {});
  if (await skip.isVisible()) {
    await skip.click();
    await expect(skip).toBeHidden();
  }
}

test("the devices sub-page mints a pairing code from the live host", async ({ page }) => {
  await page.goto("/#/settings/devices");
  await dismissOnboarding(page);

  // The page the desktop's prompt names. Before this it resolved to nothing and
  // the hash silently fell back to General, so a person following the
  // instruction could not tell "wrong place" from "not built".
  await expect(page.getByRole("heading", { level: 1, name: "Devices" })).toBeVisible({
    timeout: 30_000,
  });
  await expect(page.getByTestId("devices-empty")).toBeVisible();

  await page.getByTestId("devices-pair").click();

  // Minted server-side and returned exactly once — the console cannot re-read
  // it, so what is on screen is the only copy.
  const code = page.getByTestId("pairing-code");
  await expect(code).toBeVisible();
  expect(((await code.textContent()) ?? "").trim().length).toBeGreaterThan(16);

  // Five minutes, counted down rather than implied. A code that looks durable
  // gets saved and pasted later into a form that answers "invalid or expired".
  await expect(page.getByTestId("pairing-code-expiry")).toContainText(/Expires in \d+:\d\d/);

  // Minting a code enrols nothing. The device appears when the machine holding
  // the code redeems it, which happens on that machine and not in this browser.
  await expect(page.getByTestId("devices-empty")).toBeVisible();
});
