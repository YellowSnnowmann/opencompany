import { randomUUID } from "node:crypto";

import { expect, test, type Page } from "@playwright/test";

import { LIVE_BRAIN, LIVE_BRAIN_REASON, MCP_SERVER } from "./capabilities";

let registeredServer: string | undefined;

test.afterEach(async ({ request }) => {
  // External hosts outlive the run; managed E2E keeps the registration for its
  // isolated host lifetime because durable agent snapshots can retain tools.
  if (!registeredServer || !process.env.PW_BASE_URL) return;
  const server = registeredServer;
  let failure: string | undefined;
  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      const removed = await request.delete(
        `/api/v1/company/mcp/servers/${encodeURIComponent(server)}`,
      );
      if (removed.ok()) {
        registeredServer = undefined;
        return;
      }
      failure = `HTTP ${removed.status()}: ${await removed.text()}`;
    } catch (error) {
      failure = `request failed: ${String(error)}`;
    }
  }
  expect(failure, `removing registered MCP server ${server} failed`).toBeUndefined();
});

/**
 * The half of the MCP bridge a console cannot reach: an **agent** calling a
 * tool on a real server, over the real transport (issues #50, #467).
 *
 * `mcp.spec.ts` covers the operator's side — Settings, MCP Servers lists what
 * the host serves, and an admin adds and removes a runtime server — against a
 * default-feature host, because none of that needs an agent. Everything past
 * the registry does: `mcp_call_tool` is on an agent's belt only under the `mcp`
 * feature, and something has to decide to call it. So this spec lives in the
 * live-brain lane and is the only place the whole path is exercised:
 *
 *   agent turn → tool belt → registry → HTTP → MCP server → tool result →
 *   the agent's own reply
 *
 * # What is a fixture, and what is not
 *
 * Two fixtures, both local and neither faking a boundary that matters. The MCP
 * server is `mcp-server.mjs` — two tools, no network, speaking the JSON-RPC the
 * vendored transport drives. And the inference backend's *choice* of tool is
 * scripted: `__MOCK_TOOL_CALL__` makes the model call one named tool, because a
 * spec cannot assert on a model that is free to decline. The registry, the
 * transport, the belt, the approval policy and the journal are all real.
 *
 * # The tool is `mcp_call_tool`
 *
 * Not upstream OpenHuman's `mcp_registry_tool_call`, which is what this file's
 * predecessor asked for and never ran to find out. The belt this host builds
 * carries OpenCompany's own decorator (`src/harness/build.rs`), whose schema is
 * `{server, tool, arguments}` and which addresses a server by NAME.
 */

// The fixture is the whole subject: without a server there is nothing to call.
test.skip(
  !MCP_SERVER,
  "needs PW_MCP_SERVER pointing at an HTTP MCP server. The `Console E2E " +
    "(live brain)` CI lane starts one (issue #467).",
);
test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);

/**
 * Opens the engineer's direct conversation.
 *
 * The channel comes from the address, not a rail click: a composer is present
 * either way, so a `fill` succeeds even when nothing is selected — and the
 * reply then lands in a transcript this page is not showing.
 */
async function openThread(page: Page) {
  // A desk line is an episode run by the hive coordinator. This test is about
  // one agent's MCP call and its reply, so use the agent's direct conversation.
  await page.goto("/#/chat/dm:engineer");
  const skip = page.getByRole("button", { name: "Skip for now" });
  await skip
    .waitFor({ state: "visible", timeout: 5_000 })
    .then(() => skip.click())
    .catch(() => {
      /* already dismissed in this context */
    });
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
}

/** A row of the open transcript carrying `text`. */
function transcriptRow(page: Page, text: string) {
  return page.locator("article[data-message-id]").filter({ hasText: text }).last();
}

test("an agent calls a tool on a registered MCP server and shows the result", async ({
  page,
}) => {
  // Unique per run, and a 409 is a FAILURE rather than a shrug. The host keeps
  // runtime servers in its secret store, so a fixed name plus a tolerated
  // "already exists" would let this spec adopt a leftover registration from an
  // earlier run — pointing anywhere at all — pass against it, and then delete a
  // server it never created.
  const server = `pw-agent-mcp-${randomUUID()}`;

  // Registered through the API rather than the console: the console's add form
  // is `mcp.spec.ts`'s subject, and repeating it here would make this spec fail
  // for that page's reasons rather than its own.
  const added = await page.request.post("/api/v1/company/mcp/servers", {
    data: { name: server, endpoint: MCP_SERVER!, description: "e2e fixture" },
  });
  expect(
    added.ok(),
    `registering ${server} failed: ${added.status()} ${await added.text()}`,
  ).toBeTruthy();
  registeredServer = server;

  // Keep the registration for the life of this E2E host. OpenHuman retains
  // tool declarations in its durable agent session; deleting the server here
  // would leave a later turn with a declared but non-executable tool. The
  // managed live-brain host uses an isolated test data root and shuts down
  // after the suite, so this fixture cannot escape the run.
  await openThread(page);

  const marker = `agent-mcp-${randomUUID()}`;
  const directive = `__MOCK_TOOL_CALL__ ${JSON.stringify({
    name: "mcp_call_tool",
    arguments: { server, tool: "echo", arguments: { text: marker } },
  })}`;

    // The POST is awaited EXPLICITLY, and the reload below is why. A turn runs
    // inside the request that started it and the host drops the work when the
    // client goes away, so reloading while the send is in flight cancels the
    // turn before it reaches the model. Observed, not feared: on the run that
    // first reloaded here, the mock backend logged no call at all for this
    // message where the run before it had logged the whole round trip.
  const posted = page.waitForResponse(
    (response) => response.url().endsWith("/chat") && response.request().method() === "POST",
    { timeout: 90_000 },
  );
  await page.getByPlaceholder(/^Message /).fill(directive);
    // `exact`, because the composer's button is labelled exactly "Send" while
    // the sidebar's thread previews take their accessible names from message
    // text — so a loose match resolves to two elements the moment any message
    // in the transcript mentions sending, and dies on a strict-mode violation
    // in a spec that has nothing to do with whatever wrote that message.
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByText(/^Couldn't send/)).toHaveCount(0);
  expect((await posted).ok(), "the chat POST did not succeed").toBeTruthy();

    // Read the answer from a RELOADED transcript: it is then rehydrated from
    // `chat/history`, so what is asserted is the durable record of the turn
    // rather than whatever the open view chose to draw — the same move
    // `chat-to-card.spec.ts` makes, and the stronger claim.
  await page.reload();
  await openThread(page);

    // Both halves of the round trip on one row: the remote tool's own output,
    // which can only have come from the fixture over HTTP, and the marker that
    // says the mocked backend produced the reply carrying it.
    //
    // `MOCK_LLM`, not `__MOCK_LLM__`: a bubble renders its text as markdown and
    // the marker's own underscores are emphasis syntax, so what reaches the DOM
    // is `MOCK_LLM` inside a `<strong>`. Only the plain-text surfaces — the
    // rail's thread preview, an API response — carry it verbatim, which is a
    // tidy way to assert against the sidebar by accident.
  const reply = transcriptRow(page, `echo: ${marker}`);
  await expect(reply).toBeVisible({ timeout: 30_000 });
  await expect(reply).toContainText("MOCK_LLM");
});
