import { expect, test, type Page } from "@playwright/test";

import { disconnectSharedProviders } from "./shared-inference";

/**
 * Proof for issue #264: an agent can be opened, read, and edited **from the
 * Console**, against the live host.
 *
 * The issue's complaint is a dead end, so the evidence has to be the walk that
 * used to end nowhere: start on the Team tab, open a card, and read the things
 * that had no console surface at all — the instructions the agent was defined
 * with, its tier, the tools it may actually use, and the desks it sits on. The
 * tool grants had no *read endpoint* either (`GET …/team` sent `tools: null`
 * for every member), so this is also the first time that is checkable from
 * outside the process.
 *
 * Runs against the same live host as `wiring.spec.ts` (`companies/e2e_harness`),
 * whose manifest is what the assertions below are pinned to:
 *
 *   * `[tools] allow = ["composio", "mcp:*", "workspace", "workspace.*"]`
 *   * `ceo` is `tier = "orchestrator"`, asks for `mcp:*`, `composio`,
 *     `workspace.read`, and sits on no desk
 *   * `engineer` sits on the Engineering desk
 *
 * Default features are enough: the routes this exercises ship in the default
 * build, so nothing here is behind `capabilities.ts`.
 *
 * The spec removes the teammate it creates, so it can run repeatedly against a
 * host whose data directory persists between runs.
 */

/** The card for the teammate whose role matches `role`. */
function card(page: Page, role: string) {
  return page.getByTestId("team-card").filter({ hasText: role }).first();
}

/**
 * A fresh host greets the first visit with a welcome tour rendered over the
 * console, which swallows clicks on the view beneath it.
 *
 * Two halves, and the first is what matters. The tour is suppressed **before
 * the app boots** by seeding its own localStorage markers through an init
 * script, so it never renders and there is nothing to wait for. This is the
 * pattern `board-columns.spec.ts` uses, and it is here for a measured reason:
 * the earlier version of this helper blocked on `waitFor({ timeout: 15_000 })`
 * and swallowed the timeout, which costs the FULL fifteen seconds every time
 * the tour is absent — the common case. This spec navigates three times (the
 * `beforeEach`, the storage-cleared reload, and the cleanup), so it paid that
 * toll three times and blew the 60s test budget without a single assertion
 * failing.
 *
 * The click half stays as a belt-and-braces fallback for a host whose marker
 * key this list does not name, but it polls briefly rather than blocking.
 */
async function dismissOnboarding(page: Page) {
  const skip = page.getByRole("button", { name: "Skip for now" });
  for (let attempt = 0; attempt < 5; attempt += 1) {
    if (!(await skip.isVisible().catch(() => false))) return;
    await skip.click({ force: true }).catch(() => {});
    await page.waitForTimeout(300);
  }
  await expect(skip).toHaveCount(0);
}

/**
 * Issue #1989's reduced-vs-full Add-teammate dialog split is retired.
 * `AddMemberDialog` (`views/room/AddMemberDialog.tsx`) is the only
 * Add-teammate surface now, on every host: name, icon, post, and it always
 * lands the operator on the new agent's own page with the edit form open —
 * there is no design pass and no second shape to detect.
 */

async function goToTeam(page: Page) {
  // The Company page, whose Cards half is the roster (issue #1141). Bare
  // `#/team` redirects here; this asks for the address that exists.
  await page.goto("/#/company");
  await dismissOnboarding(page);
  await expect(page.getByTestId("team-card").first()).toBeVisible({ timeout: 30_000 });
}

test.beforeEach(async ({ page }) => {
  // Registered before any navigation, so it also re-seeds after the
  // storage-clearing reload below — which wipes the tour markers along with
  // everything else and would otherwise bring the tour back mid-test.
  await page.addInitScript(() => {
    const seen = JSON.stringify({ skipped: true, seenAt: Date.now() });
    for (const key of ["oc-tour:single", "oc-tour:e2e-harness-co", "oc-tour:null"]) {
      window.localStorage.setItem(key, seen);
    }
  });
  await goToTeam(page);
});

test("a company agent opens from its card and shows what it is", async ({ page }) => {
  // The walk that used to end nowhere: the card's own name is the way in.
  await card(page, "Chief Executive").getByTestId("team-card-open").click();

  // A sub-page, not a modal: the agent is addressable, so it survives a
  // refresh and Back returns to the roster.
  await expect(page).toHaveURL(/#\/company\/agent\/ceo$/);

  await expect(page.getByTestId("agent-name")).toHaveText("Chief Executive");

  // The tier, resolved by the host rather than read off the manifest string.
  await expect(page.getByTestId("agent-tier")).toContainText("Orchestrator");
  await expect(page.getByTestId("agent-source")).toHaveText("Company blueprint");

  // The instructions it was defined with — the "AGENT.md for that agent" the
  // issue asks for, which the manifest always carried and the console never
  // showed after creation. Lives on the Instructions tab, not the Overview
  // tab this arrival opens on.
  await page.getByRole("tab", { name: "Instructions" }).click();
  await expect(page.getByTestId("agent-description")).toContainText("Sets direction");

  // The effective tool grants. Every one of these is an intersection of the
  // agent's own `tools` line with the company allow-list, and none of it was
  // readable anywhere before this issue — reported on its own Tools tab.
  await page.getByRole("tab", { name: "Tools" }).click();
  const tools = page.getByTestId("agent-tools");
  await expect(tools).toContainText("workspace.read");
  await expect(tools).toContainText("composio");
  await expect(tools).toContainText("mcp:*");

  // This agent sits on no desk, so the identity row carries no desk chip.
  await expect(page.locator('[data-testid^="agent-desk-"]')).toHaveCount(0);

  // `ceo` is a blueprint teammate, and this is the assertion that used to pin
  // the opposite. The Edit affordance was *present and disabled* (#1141) with a
  // note saying the edit belonged in `company.toml` — which is advice with no
  // action behind it for a hosted tenant that has no checkout to edit and no
  // redeploy to make. A manifest teammate is editable now, through an overlay
  // layered on the record rather than a rewrite of the blueprint, so the button
  // is live and the read-only note is gone. Back on Instructions, where Edit
  // lives.
  await page.getByRole("tab", { name: "Instructions" }).click();
  await expect(page.getByTestId("agent-edit")).toBeEnabled();
  await expect(page.getByTestId("agent-readonly-note")).toHaveCount(0);

  // And it is a real editor rather than a live-looking button: clicking opens
  // the same fields a console-created teammate is edited through, so a manifest
  // teammate follows one flow and not a second, weaker one. Persona instructions
  // remain a distinct field layered on top of main's description editor.
  await page.getByTestId("agent-edit").click();
  await expect(page.getByTestId("agent-field-description")).toBeVisible();
  await expect(page.getByTestId("agent-field-instructions")).toBeVisible();

  // What does *not* change: the source still names the blueprint. Editing does
  // not launder a manifest teammate into an overlay one — the operator can
  // still see where this teammate came from.
  await expect(page.getByTestId("agent-source")).toHaveText("Company blueprint");

  // The breadcrumb returns to the Company page (issue #1141, replacing "Back to
  // team" — this page is linked into from the org chart and the chat pane, and
  // Back named a page half its arrivals had never seen).
  await page.getByTestId("agent-breadcrumb-company").click();
  await expect(page.getByTestId("team-card").first()).toBeVisible();
});

test("desk membership is on the agent, and an agent is reachable by link", async ({ page }) => {
  // Deep link straight to an agent: the detail view resolves the id against the
  // host rather than falling back to the roster.
  await page.goto("/#/company/agent/engineer");
  await dismissOnboarding(page);

  await expect(page.getByTestId("agent-name")).toHaveText("Engineer", { timeout: 30_000 });
  await expect(page.getByTestId("agent-tier")).toContainText("Worker");
  const desk = page.getByTestId("agent-desk-engineering");
  await expect(desk).toContainText("Engineering desk");
  await expect(desk).toHaveAttribute("href", "#/company/engineering");
});

test("an agent defined in the console can be read back and edited", async ({ page }) => {
  const role = "Spec Runner";
  const seededDescription = "Original instructions.";

  // The `try` opens BEFORE the teammate is created, not after. The POST lands
  // as soon as the dialog is submitted, so a failure in the assertion that
  // follows it would otherwise skip the cleanup and leave the teammate on the
  // host — which breaks the next run of a spec that is meant to be repeatable,
  // and leaves a second card for `card(page, role)` to match.
  try {
    // Define one through the dialog the issue calls create-only: a name and a
    // post. The description is written on the page this redirects to.
    await page.getByRole("button", { name: "Add agent" }).first().click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Name").fill("Detail Spec");
    await dialog.getByLabel("Post").fill(role);
    await dialog.getByRole("button", { name: "Add agent" }).click();

    // The redirect lands on the new agent's own page with the edit form
    // already open, so the description goes there.
    await expect(page).toHaveURL(/#\/company\/agent\/[^?]+\?edit/, { timeout: 30_000 });
    await page.getByTestId("agent-field-description").fill(seededDescription);
    await page.getByTestId("agent-save").click();
    await expect(page.getByTestId("agent-description")).toContainText(seededDescription, {
      timeout: 30_000,
    });

    // Back to the roster, so the walk below starts from the same place the
    // rest of this file's tests do.
    await goToTeam(page);
    await expect(card(page, "Detail Spec")).toBeVisible({ timeout: 30_000 });

    // Open it. This is the half that was impossible: the roster was write-once
    // per member, so iterating on an agent meant deleting it and starting over.
    await card(page, "Detail Spec").getByTestId("team-card-open").click();
    await expect(page.getByTestId("agent-source")).toHaveText("Added here");

    // Description and persona live on the Instructions tab, not the Overview
    // tab this plain (non-`?edit`) arrival opens on.
    await page.getByRole("tab", { name: "Instructions" }).click();
    await expect(page.getByTestId("agent-description")).toContainText(seededDescription);

    // A console-defined agent holds the company's standard grant, so it reads
    // back with the whole allow-list rather than an empty tool list — reported
    // on its own Tools tab.
    await page.getByRole("tab", { name: "Tools" }).click();
    await expect(page.getByTestId("agent-tools")).toContainText("composio");

    // Edit it, back on Instructions.
    await page.getByRole("tab", { name: "Instructions" }).click();
    await page.getByTestId("agent-edit").click();
    await page.getByTestId("agent-field-description").fill("Rewritten instructions.");
    await page.getByTestId("agent-field-role").fill("Spec Runner II");
    await page.getByTestId("agent-save").click();
    await expect(page.getByTestId("agent-description")).toContainText("Rewritten instructions.", {
      timeout: 30_000,
    });

    // Host-backed, not local state: it survives a storage-cleared reload. This
    // is the assertion that separates "the console remembers" from "the company
    // was actually changed".
    //
    // `reload`, not `goto(page.url())`. Navigating to the URL the page is
    // already on is a same-document navigation — the app keeps every piece of
    // in-memory state, so the two assertions below would pass off the panel's
    // own state and prove nothing about the host. That is not hypothetical: it
    // is why this spec went green on the panel while the roster underneath it
    // was stale.
    await page.evaluate(() => {
      localStorage.clear();
      sessionStorage.clear();
    });
    await page.reload();
    await dismissOnboarding(page);
    await page.getByRole("tab", { name: "Instructions" }).click();
    await expect(page.getByTestId("agent-description")).toContainText("Rewritten instructions.", {
      timeout: 30_000,
    });
    await expect(page.getByTestId("agent-role")).toHaveText("Spec Runner II");

    // …and the roster the operator came from agrees, rather than only the panel
    // they edited in.
    await page.getByTestId("agent-breadcrumb-company").click();
    await expect(card(page, "Spec Runner II")).toBeVisible({ timeout: 30_000 });
  } finally {
    // Leave the company as we found it, whatever happened above.
    await goToTeam(page);
    const leftover = page.getByTestId("team-card").filter({ hasText: "Detail Spec" }).first();
    if (await leftover.count()) {
      await leftover.getByLabel("Agent actions").click();
      await page.getByRole("menuitem", { name: "Remove" }).click();
      await expect(leftover).toHaveCount(0, { timeout: 30_000 });
    }
  }
});

/**
 * Undoes the pin test below, after it — even when it timed out.
 *
 * The `e2e-pair` row points at an unreachable endpoint, and connecting it to
 * the one company the whole run shares makes it that company's default (X1)
 * — so, left behind, it is where every agent turn in every later spec file
 * goes. That is what happened when this test timed out and its in-body
 * `finally` never ran: Playwright abandons a timed-out test function,
 * `finally` included, and thirty-odd unrelated specs failed with
 * `inference request failed … 127.0.0.1:9`. A hook runs regardless.
 *
 * The pin is cleared first so the delete is not refused as in-use by it; a
 * clear on an unpinned agent is a no-op. What the delete can and cannot put
 * back — the row goes, the default it became does not — is in
 * `disconnectSharedProviders`.
 */
test.afterEach(async ({ request }) => {
  await request
    .patch("/api/v1/company/team/researcher", { data: { provider: null, model: null } })
    .catch(() => {});
  await disconnectSharedProviders(request, ["e2e-pair"]);
});

test("an admin pins an agent to a provider and model, then clears it (keys rework, issue #2306, slice 3b)", async ({
  page,
}) => {
  // A row this agent can pin to, created through the same API the LLM page
  // itself calls (`POST …/inference/providers`, the model-required shape —
  // 2c). Nothing calls the endpoint in this test beyond setup: `UNREACHABLE`
  // means the draft probe and the row's own Test both fail, which is fine —
  // pinning an agent to a provider does not require it to answer.
  //
  // Cleanup is the `afterEach` above, not a `finally` here — see it for why.
  await page.request.post("/api/v1/company/inference/providers", {
    data: {
      kind: "custom",
      label: "E2E Pair",
      baseUrl: "http://127.0.0.1:9/v1",
      key: "sk-not-a-real-key",
      model: "e2e-model",
    },
  });

  await page.goto("/#/company/agent/researcher");
  await dismissOnboarding(page);
  await expect(page.getByTestId("agent-name")).toHaveText("Researcher", { timeout: 30_000 });

  await page.getByRole("tab", { name: "Model" }).click();
  await page.getByTestId("agent-harness-edit").click();

  // Unpinned: the company default line, not the ACP model picker — this
  // agent is on the default (`built_in`) harness.
  await expect(page.getByTestId("agent-provider-select")).toBeVisible();
  await expect(page.getByTestId("agent-model-select")).toHaveCount(0);
  await expect(page.getByTestId("agent-model-input")).toHaveCount(0);

  // The option's accessible name carries the row's model too
  // (`{label} · {model}`, `AgentDetailView.tsx`'s `pinnable.map`), so an exact
  // match on the bare label alone never resolves — this hung the whole test
  // on its 60s timeout waiting for an option that was on screen the entire
  // time, just under a longer name.
  await page.getByTestId("agent-provider-select").click();
  await page.getByRole("option", { name: "E2E Pair · e2e-model", exact: true }).click();

  // Prefilled from the row's own model (`e2e-model`), so an operator who
  // only wants "this agent, this provider" need not retype it — still
  // required to be a real, non-tier id (D-model, 2d) before Save unlocks.
  const model = page.locator("#agent-model-field-input");
  await expect(model).toHaveValue("e2e-model");
  await model.fill("test-model-large");

  await page.getByTestId("agent-harness-save").click();
  await expect(page.getByTestId("agent-pair-badge")).toContainText("E2E Pair · test-model-large", {
    timeout: 30_000,
  });

  // Host-backed, not local state.
  await page.reload();
  await dismissOnboarding(page);
  await page.getByRole("tab", { name: "Model" }).click();
  await expect(page.getByTestId("agent-pair-badge")).toContainText("E2E Pair · test-model-large", {
    timeout: 30_000,
  });

  // Clear it: back to the company default, in one request (`provider` and
  // `model` both `null`). Clearing an existing pin gates the save behind its
  // own confirm dialog (round-2 review, P2-5, `saveHarnessAndModel`'s doc in
  // `AgentDetailView.tsx`) — Save opens `agent-pair-clear-confirm` rather
  // than persisting immediately, and only the dialog's own submit sends the
  // request.
  await page.getByTestId("agent-harness-edit").click();
  await page.getByTestId("agent-pair-clear").click();
  await page.getByTestId("agent-harness-save").click();
  const clearConfirm = page.getByTestId("agent-pair-clear-confirm");
  await expect(clearConfirm).toBeVisible();
  await page.getByTestId("agent-pair-clear-confirm-submit").click();
  await expect(page.getByTestId("agent-pair-default")).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId("agent-pair-badge")).toHaveCount(0);
});
