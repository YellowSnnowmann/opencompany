import { expect, test } from "@playwright/test";

test("an SSO fragment redeems once and confirms the signed-in owner", async ({ browser }) => {
  const token = "header.payload.signature";
  const context = await browser.newContext({ storageState: undefined });
  const page = await context.newPage();
  let redemptionCount = 0;
  let setupSession: string | undefined;

  await page.route("**/auth/config", (route) =>
    route.fulfill({ json: { mode: "email", passwords: true, magicLink: true, claimable: false } }),
  );
  await page.route("**/api/v1/sso/redeem", async (route) => {
    redemptionCount += 1;
    expect(route.request().postDataJSON()).toEqual({ token });
    await route.fulfill({
      json: {
        id: "ada@example.com",
        email: "ada@example.com",
        role: "admin",
        company: "acme",
        hasPassword: false,
        mustChangePassword: false,
        session: "acme.header.payload.signature",
      },
    });
  });
  await page.route("**/api/v1/spec", (route) =>
    route.fulfill({ json: { setup_complete: false } }),
  );
  await page.route("**/api/v1/setup", async (route) => {
    setupSession = route.request().headers()["x-opencompany-session"];
    await route.fulfill({ status: 503, json: { error: "test setup response" } });
  });

  try {
    await page.goto(`/#/sso?token=${token}`);
    const confirmation = page.getByTestId("login-sso-done");
    await expect(confirmation).toContainText("Signed in as");
    await expect(confirmation).toContainText("ada@example.com");
    expect(redemptionCount).toBe(1);
    expect(new URL(page.url()).hash).not.toContain(token);
    await expect
      .poll(() => setupSession)
      .toBe("acme.header.payload.signature");
  } finally {
    await context.close();
  }
});
