import { randomUUID } from "node:crypto";

import { expect, test } from "@playwright/test";

import { LIVE_BRAIN, LIVE_BRAIN_REASON, MCP_SERVER } from "./capabilities";

/**
 * A server's tool permissions, narrowed for one teammate.
 *
 * Needs a host with the `openhuman` and `mcp` features and a real MCP server to
 * probe: without a tool inventory there are no rows, and a spec asserting that
 * a forbidden control is absent would pass against a panel rendering nothing.
 * That is why this file is gated and the rendering specs are not.
 *
 * The server is registered here against the live lane's own `mcp-server.mjs`
 * rather than driving the manifest's `deepwiki`, whose inventory comes off the
 * public network — the rows are the inventory, so that would put this spec's
 * verdict outside the repository.
 */

type Page = import("@playwright/test").Page;

test.skip(
  !MCP_SERVER,
  "needs PW_MCP_SERVER pointing at an HTTP MCP server. The `Console E2E " +
    "(live brain)` CI lane starts one.",
);
test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);

// A test here may open the panel twice, and each open waits up to 30s for the
// probed inventory — which does not fit the suite's 60s default.
test.describe.configure({ timeout: 120_000 });

/** The tools `mcp-server.mjs` advertises. */
const TOOL = "echo";

/** The teammate the lens is pointed at, by the id the address carries. */
const TEAMMATE = { id: "engineer", name: "Engineer" };

const SERVER = `pw-lens-${randomUUID().slice(0, 8)}`;

test.beforeAll(async ({ request }) => {
  if (!MCP_SERVER || !LIVE_BRAIN) return;
  const added = await request.post("/api/v1/company/mcp/servers", {
    data: { name: SERVER, endpoint: MCP_SERVER, description: "e2e fixture" },
  });
  expect(
    added.ok(),
    `registering ${SERVER} failed: ${added.status()} ${await added.text()}`,
  ).toBeTruthy();
});

test.afterAll(async ({ request }) => {
  if (!MCP_SERVER || !LIVE_BRAIN) return;
  await request
    .delete(`/api/v1/company/mcp/servers/${encodeURIComponent(SERVER)}`)
    .catch(() => undefined);
});

async function openPermissions(page: Page, showing?: string) {
  const lens = showing === undefined ? "" : `&showing=${showing}`;
  // A reload, because moving between lenses changes only the hash: the SPA
  // would not remount and the panel under test would be the previous one.
  await page.goto(`/#/connections/mcp?server=${SERVER}${lens}`);
  await page.reload();
  const skip = page.getByRole("button", { name: "Skip for now" });
  await skip
    .waitFor({ state: "visible", timeout: 10_000 })
    .then(() => skip.click())
    .catch(() => {
      /* already seen in this context */
    });
  await expect(page.getByTestId("mcp-tool-permissions")).toBeVisible({
    timeout: 30_000,
  });
}

function toolRow(page: Page, tool: string) {
  return page.locator(
    `[data-testid="mcp-permission-row"][data-tool="${tool}"]`,
  );
}

test("the company lens is the default, and names its own exceptions", async ({
  page,
}) => {
  await openPermissions(page);

  const lens = page.getByTestId("mcp-permissions-lens");
  await expect(lens).toBeVisible();

  // The company document is what an operator sees first: it is the truth most
  // of the roster runs on, and a page that opened on one teammate would make
  // the common case the exception.
  await expect(lens).toContainText("Everyone");

  // No source chip and no scope notice in this lens — the Everyone view must
  // read exactly as it did before per-teammate rules existed.
  await expect(toolRow(page, TOOL)).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId("mcp-permission-source")).toHaveCount(0);
  await expect(page.getByTestId("mcp-permissions-scope-notice")).toHaveCount(0);
});

test("the picker scopes the read, and puts the lens in the address", async ({
  page,
}) => {
  await openPermissions(page);
  await expect(toolRow(page, TOOL)).toBeVisible({ timeout: 30_000 });

  const scoped: string[] = [];
  await page.route("**/tools/policy*", (route) => {
    scoped.push(route.request().url());
    return route.continue();
  });

  await page.getByTestId("mcp-permissions-lens").click();
  await page.getByRole("option", { name: TEAMMATE.name, exact: true }).click();

  await expect(page.getByTestId("mcp-permission-source").first()).toBeVisible({
    timeout: 30_000,
  });

  // The lens is in the address, so the view is linkable and a reload keeps it.
  expect(page.url()).toContain(`showing=${TEAMMATE.id}`);
  expect(
    scoped.some((url) => url.includes("agent=")),
    `no scoped read was made: ${scoped.join(" | ")}`,
  ).toBe(true);
});

test("a teammate lens says whose permissions a click would change", async ({
  page,
}) => {
  await openPermissions(page, TEAMMATE.id);

  // The one place a mis-click is expensive: every control on the panel has
  // silently moved to another document, and the lens value is the only thing
  // that said so.
  const notice = page.getByTestId("mcp-permissions-scope-notice");
  await expect(notice).toBeVisible({ timeout: 30_000 });
  await expect(notice).toContainText(TEAMMATE.name);
  await expect(notice).toContainText("leaves the company default as it is");
});

test("an option the host would refuse is disabled, and says why", async ({
  page,
}) => {
  // The rule under test is relative, so the company's own mode is asserted
  // rather than assumed: if this tool ever resolves to always_allow, the
  // refusal below is measuring an empty set and this line fails first.
  await openPermissions(page);
  const company = toolRow(page, TOOL);
  await expect(company).toBeVisible({ timeout: 30_000 });
  await expect(company.getByTestId("mcp-mode-needs_approval")).toHaveAttribute(
    "aria-checked",
    "true",
  );

  await openPermissions(page, TEAMMATE.id);
  const scoped = toolRow(page, TOOL);
  await expect(scoped).toBeVisible({ timeout: 30_000 });

  // A per-teammate rule may only restrict. Hiding the looser option would make
  // the page silently different per row; disabling it teaches the rule where it
  // applies, and a control the host would discard is never rendered as live.
  const looser = scoped.getByTestId("mcp-mode-always_allow");
  await expect(looser).toBeDisabled();
  await expect(looser).toHaveAttribute("title", /can only be stricter/);
  // The stricter one is still live, or the lens would be read-only by accident.
  await expect(scoped.getByTestId("mcp-mode-blocked")).toBeEnabled();
});

test("the teammate lens says what the narrowing costs", async ({ page }) => {
  await openPermissions(page, TEAMMATE.id);

  await expect(page.getByTestId("mcp-permissions-summary")).toBeVisible({
    timeout: 30_000,
  });

  // The tier default belongs to the tool, never to the teammate — the host
  // refuses a scoped tier write — so this lens shows its VALUE and no control.
  // A disabled select here read as a live one at a glance, and three of them
  // buried the per-tool controls that are the point of the lens.
  await expect(
    page.getByTestId("mcp-permissions-tier-default-read_only"),
  ).toHaveCount(0);
  const companyOnly = page.getByTestId("mcp-tier-company-only-read_only");
  await expect(companyOnly).toBeVisible();
  await expect(companyOnly).toContainText("for everyone");
});

test("the inert-approval notice is a function of the host, not a constant", async ({
  page,
}) => {
  await openPermissions(page);

  // Approvals do not park on this build, so "needs approval" behaves as allow
  // and the page must say so where permissions are set, not only on the
  // teammate page. When approvals return, the host reports it and this notice
  // goes on its own.
  await expect(page.getByTestId("mcp-approvals-inert")).toBeVisible({
    timeout: 30_000,
  });
});

test.afterEach(async ({ request }) => {
  if (!MCP_SERVER || !LIVE_BRAIN) return;
  // Each test above states a property of one document, so none of them may
  // inherit another's writes.
  await request
    .delete(
      `/api/v1/company/mcp/servers/${encodeURIComponent(SERVER)}/tools/policy`,
    )
    .catch(() => undefined);
});
