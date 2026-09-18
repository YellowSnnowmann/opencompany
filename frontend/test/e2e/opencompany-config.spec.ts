import { expect, test } from "@playwright/test";

test("serves the runtime console configuration before OpenPanel loads", async ({
  page,
  request,
}) => {
  test.skip(Boolean(process.env.PW_BASE_URL), "requires the Playwright-managed host configuration");
  const [indexResponse, configResponse] = await Promise.all([
    request.get("/"),
    request.get("/opencompany-config.js"),
  ]);

  expect(indexResponse.ok()).toBe(true);
  expect(await indexResponse.text()).toContain(
    '<script src="/opencompany-config.js" defer></script>',
  );

  expect(configResponse.ok()).toBe(true);
  expect(configResponse.headers()["content-type"]).toContain(
    "application/javascript",
  );
  expect(configResponse.headers()["cache-control"]).toBe("no-store");
  const expectedConfig =
    process.env.PW_ANALYTICS === "1"
      ? 'window.OPENCOMPANY_CONFIG=Object.assign(window.OPENCOMPANY_CONFIG||{},' +
        '{analytics:true,analyticsEndpoint:"https://collector.example/"});\n'
      : "window.OPENCOMPANY_CONFIG=window.OPENCOMPANY_CONFIG||{};\n";
  expect(await configResponse.text()).toBe(expectedConfig);

  await page.goto("/");
  await expect(page).toHaveTitle("OpenCompany Console");
});
