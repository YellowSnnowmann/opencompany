import { randomUUID } from "node:crypto";

import { expect, test } from "@playwright/test";

import { LIVE_BRAIN, LIVE_BRAIN_REASON, MCP_SERVER } from "./capabilities";

/**
 * A per-teammate block reaches the agent, on its next turn, with no restart.
 *
 * This is the spec the whole staleness fix exists for. Before it, a tool set to
 * Block kept being callable: the deny list is baked into the roster attachment
 * at build time, and the staleness fingerprint did not hash the policy
 * document, so writing a permission invalidated nothing. It took a host restart
 * to apply — which the docs already claimed was unnecessary.
 *
 * Unit tests pin the fingerprint moving and the attachment denying. Only this
 * one pins the thing an operator actually cares about: the agent stops being
 * able to call it, without anybody bouncing the host.
 *
 * Needs the `openhuman` and `mcp` features plus an inference backend, so it runs
 * in the live-brain lane. The server it drives is registered here against that
 * lane's local `mcp-server.mjs`, not the manifest's `deepwiki`: the rows are the
 * probed inventory, and reading them off the public network would make this
 * spec's verdict depend on it.
 *
 * Serial: each test leaves a permission written that the next
 * one reads or clears, which is the same order an operator moves through.
 */

type Page = import("@playwright/test").Page;

test.skip(
  !MCP_SERVER,
  "needs PW_MCP_SERVER pointing at an HTTP MCP server. The `Console E2E " +
    "(live brain)` CI lane starts one.",
);
test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);

// Serial, and each test drives at least one model turn whose own waits are
// 120s — more than the suite's 60s default allows a test to reach.
test.describe.configure({ mode: "serial", timeout: 300_000 });

/**
 * A refusal, in either of the two wordings a blocked call can carry.
 *
 * A block on a declared server is folded into the attachment's `deny_tools`, so
 * the vendored transport refuses it before the call reaches OpenCompany's own
 * wrapper — `tool `x` is not permitted on server `y``. `blocked_refusal` is what
 * the wrapper writes, and it is the one a registry-installed server produces.
 * Both are the same verdict, and pinning only one made this spec pass or fail on
 * which server the fixture happened to be.
 */
const REFUSAL =
  /is blocked by this company's tool permissions|is not permitted on server/;

/** The tool `mcp-server.mjs` answers by quoting its `text` argument back. */
const TOOL = "echo";

/**
 * The teammate the rule is written for, and the desk whose only member it is.
 *
 * A teammate's direct conversation addresses that teammate's own turn. Group
 * chat lines are conducted episodes, whose completion does not render a tool
 * refusal as a direct reply in the room transcript.
 */
const BLOCKED = { id: "engineer", name: "Engineer", thread: "dm:engineer" };
const UNTOUCHED = { thread: "dm:writer" };

/** Registered per run: the host keeps runtime servers in its secret store. */
const SERVER = `pw-perm-${randomUUID().slice(0, 8)}`;

test.beforeAll(async ({ request }) => {
  if (!MCP_SERVER || !LIVE_BRAIN) return;
  // The add probes, so the tool inventory the rows are drawn from is stored
  // before the first test looks for a row.
  const added = await request.post("/api/v1/company/mcp/servers", {
    data: { name: SERVER, endpoint: MCP_SERVER, description: "e2e fixture" },
  });
  expect(
    added.ok(),
    `registering ${SERVER} failed: ${added.status()} ${await added.text()}`,
  ).toBeTruthy();
});

test.afterAll(async ({ request }) => {
  // The managed host is isolated and exits after the suite; its durable agent
  // snapshots retain this server's tool declarations, so removing it here
  // makes later tests fail with non-executable tools. External hosts persist
  // beyond the run and do need the randomized registration removed.
  if (!MCP_SERVER || !LIVE_BRAIN || !process.env.PW_BASE_URL) return;
  const removed = await request.delete(
    `/api/v1/company/mcp/servers/${encodeURIComponent(SERVER)}`,
  );
  expect(
    removed.ok(),
    `removing ${SERVER} failed: ${removed.status()} ${await removed.text()}`,
  ).toBeTruthy();
});

/**
 * Opens this server's permissions panel, with `showing` as the lens.
 *
 * The lens comes from the address rather than from the picker: which teammate is
 * scoped is this spec's premise, not its subject, and a click through a select
 * would make it fail for the picker's reasons.
 */
/** Clears the first-visit tour, which a reload puts back over the page. */
async function dismissTour(page: Page) {
  const skip = page.getByRole("button", { name: "Skip for now" });
  await skip
    .waitFor({ state: "visible", timeout: 5_000 })
    .then(() => skip.click())
    .catch(() => {
      /* not shown in this context */
    });
}

async function openPermissions(page: Page, showing?: string) {
  const lens = showing === undefined ? "" : `&showing=${showing}`;
  // Hash-only navigation does not remount the SPA, so each entry reloads.
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

/** The row for one tool, addressed by the name it carries. */
function toolRow(page: Page, tool: string) {
  return page.locator(
    `[data-testid="mcp-permission-row"][data-tool="${tool}"]`,
  );
}

/**
 * Makes the agent on `desk` call `TOOL` once, and returns the marker its result
 * would carry.
 *
 * The POST is awaited explicitly before the reload: a turn runs inside the
 * request that started it and the host drops the work when the client goes away,
 * so reloading while the send is in flight cancels the turn before it reaches
 * the model. The marker is unique per call because the mock backend serves one
 * `__MOCK_TOOL_CALL__` per distinct payload — two turns asking for the same
 * arguments would leave the second one with a plain text reply.
 */
async function callTool(page: Page, thread: string): Promise<string> {
  const marker = `perm-${randomUUID()}`;
  await page.goto(`/#/chat/${thread}`);
  await page.reload();
  await dismissTour(page);
  const composer = page.getByPlaceholder(/^Message /);
  await expect(composer).toBeVisible({ timeout: 30_000 });

  const send = page.getByRole("button", { name: "Send", exact: true });

  const posted = page.waitForResponse(
    (response) =>
      response.url().endsWith("/chat") && response.request().method() === "POST",
    { timeout: 120_000 },
  );
  await composer.fill(
    `__MOCK_TOOL_CALL__ ${JSON.stringify({
      name: "mcp_call_tool",
      arguments: {
        server: SERVER,
        tool: TOOL,
        arguments: { text: marker },
      },
    })}`,
  );
  // Send enables only once the composer holds a draft.
  await expect(send).toBeEnabled({ timeout: 60_000 });
  await send.click();
  await expect(page.getByText(/^Couldn't send/)).toHaveCount(0);
  expect((await posted).ok(), "the chat POST did not succeed").toBeTruthy();

  // Read from a reloaded transcript, so what is asserted is the durable record
  // of the turn rather than whatever the open view chose to draw.
  await page.reload();
  await dismissTour(page);
  await page.goto(`/#/chat/${thread}`);
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({
    timeout: 30_000,
  });
  return marker;
}

/** Drives one listing of the server's tools, so the belt's inventory is readable. */
async function listTools(page: Page, thread: string): Promise<void> {
  await page.goto(`/#/chat/${thread}`);
  await page.reload();
  await dismissTour(page);
  const composer = page.getByPlaceholder(/^Message /);
  await expect(composer).toBeVisible({ timeout: 30_000 });

  const send = page.getByRole("button", { name: "Send", exact: true });

  const posted = page.waitForResponse(
    (response) =>
      response.url().endsWith("/chat") && response.request().method() === "POST",
    { timeout: 120_000 },
  );
  await composer.fill(
    `__MOCK_TOOL_CALL__ ${JSON.stringify({
      name: "mcp_list_tools",
      arguments: { server: SERVER },
    })}`,
  );
  // Send enables only once the composer holds a draft.
  await expect(send).toBeEnabled({ timeout: 60_000 });
  await send.click();
  await expect(page.getByText(/^Couldn't send/)).toHaveCount(0);
  expect((await posted).ok(), "the chat POST did not succeed").toBeTruthy();

  await page.reload();
  await dismissTour(page);
  await page.goto(`/#/chat/${thread}`);
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({
    timeout: 30_000,
  });
}

/** Every bubble in the open transcript. */
function bubbles(page: Page) {
  return page.locator("article[data-message-id]");
}

test("a block set for one teammate refuses that teammate's next turn", async ({
  page,
}) => {
  await openPermissions(page, BLOCKED.id);

  const row = toolRow(page, TOOL);
  await expect(row).toBeVisible({ timeout: 30_000 });
  await row.getByTestId("mcp-mode-blocked").click();
  await expect(row.getByTestId("mcp-permission-source")).toContainText(
    new RegExp(`et for ${BLOCKED.name}`),
  );

  // No restart and no rebuild: the next turn is the whole contract.
  const marker = await callTool(page, BLOCKED.thread);

  await expect(bubbles(page).filter({ hasText: REFUSAL }).last()).toBeVisible({
    timeout: 120_000,
  });
  // And the call did not also go through — a refusal beside a result would mean
  // the deny list was consulted somewhere that does not gate the dispatch.
  await expect(bubbles(page).filter({ hasText: `echo: ${marker}` })).toHaveCount(
    0,
  );

  // The blocked tool is gone from what this teammate can even see: a deny that
  // only refuses the dispatch still offers the model a tool it will be refused
  // for choosing. The listing is asked for rather than waited on, because the
  // mock brain chooses no tool of its own.
  await listTools(page, BLOCKED.thread);
  const listing = bubbles(page).filter({ hasText: /"name":\s*"describe"/ });
  await expect(listing.last()).toBeVisible({ timeout: 120_000 });
  await expect(
    listing.filter({ hasText: new RegExp(`"name":\\s*"${TOOL}"`) }),
  ).toHaveCount(0);
});

test("the same tool stays callable for a teammate the rule does not name", async ({
  page,
}) => {
  // The other half of the promise, and the reason a per-teammate layer exists at
  // all: narrowing one teammate must not narrow the company.
  const marker = await callTool(page, UNTOUCHED.thread);

  await expect(
    bubbles(page).filter({ hasText: `echo: ${marker}` }).last(),
  ).toBeVisible({ timeout: 120_000 });

  // Scoped to this turn by its marker: the transcript still holds the
  // refusal the earlier block produced, and that one is meant to stay.
  await expect(
    bubbles(page).filter({ hasText: marker }).filter({ hasText: REFUSAL }),
  ).toHaveCount(0);
});

test("clearing the teammate's rule restores the call, again with no restart", async ({
  page,
}) => {
  await openPermissions(page, BLOCKED.id);

  const row = toolRow(page, TOOL);
  await expect(row).toBeVisible({ timeout: 30_000 });
  await row.getByTestId("mcp-permission-clear-row").click();
  await expect(row.getByTestId("mcp-permission-source")).not.toContainText(
    new RegExp(`et for ${BLOCKED.name}`),
  );

  const marker = await callTool(page, BLOCKED.thread);

  await expect(
    bubbles(page).filter({ hasText: `echo: ${marker}` }).last(),
  ).toBeVisible({ timeout: 120_000 });

  // Scoped to this turn by its marker: the transcript still holds the
  // refusal the earlier block produced, and that one is meant to stay.
  await expect(
    bubbles(page).filter({ hasText: marker }).filter({ hasText: REFUSAL }),
  ).toHaveCount(0);
});

test("a teammate refused every tool is told so, not left guessing", async ({
  page,
}) => {
  await openPermissions(page, BLOCKED.id);
  await expect(toolRow(page, TOOL)).toBeVisible({ timeout: 30_000 });

  const rows = page.getByTestId("mcp-permission-row");
  const count = await rows.count();
  expect(count, "the panel listed no rows to refuse").toBeGreaterThan(0);
  for (let i = 0; i < count; i += 1) {
    await rows.nth(i).getByTestId("mcp-mode-blocked").click();
  }

  // The state that cannot arise company-wide without a deliberate all-tools
  // block, and is the expected shape of a per-teammate narrowing. An agent in it
  // sees an empty tool list and cannot tell it from a server being down.
  await expect(page.getByTestId("mcp-permissions-fully-refused")).toBeVisible();

  // The way out is offered where the state is announced. The other reset sits
  // below every tier group, which on a server with tools is off-screen — so the
  // page named a dead end and put the exit somewhere the operator had to hunt.
  await page.getByTestId("mcp-permissions-fully-refused-clear").click();
  await expect(page.getByTestId("mcp-permissions-fully-refused")).toHaveCount(0);
});
