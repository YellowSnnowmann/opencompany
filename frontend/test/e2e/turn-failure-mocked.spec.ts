import { expect, test, type Page } from "@playwright/test";

import { bubbles, openChannel } from "./chat-helpers";

/**
 * The fail-closed turn notice (keys rework, issue #2306, round-2 review
 * KR-L2-03) — the host's exact X9 sentence, rendered verbatim, plus the
 * action that fixes it, in place of the generic "This turn couldn't be
 * finished… try again" line every fail-closed case rendered before this.
 *
 * Fully mocked `chat/history`, following `chat-history-loading.spec.ts`'s
 * pattern: a running host (`playwright.config.ts` brings one up), this
 * file's own response in place of a real one. No real turn is run and no
 * credential is involved — this is purely "does the console draw what the
 * host said", coded against the orchestrator's stated contract
 * (`userFacing`, `code`, `message`, plus agent id and provider slug) ahead
 * of `docs/key-reworks/in-use-guards.md` §5 documenting it for real. See
 * `src/lib/turn-failure.ts`'s own module doc for the alignment note.
 */

/** The harness roster's agent id this DM addresses; its DM channel id is `dm:<id>` (issue #364). */
const TEAMMATE = "engineer";
const DM_CHANNEL = `dm:${TEAMMATE}`;

/** One journaled line, exactly `ChatHistoryMessageDto`-shaped. */
function historyLine(seq: number, over: Record<string, unknown> = {}) {
  return {
    id: String(seq),
    channel: TEAMMATE,
    author: TEAMMATE,
    text: "This turn couldn't be finished — something went wrong or a step took too long. Nothing was left half-done. Send the message again to retry.",
    atMillis: Date.now() + seq,
    mine: false,
    ...over,
  };
}

async function mockHistory(page: Page, messages: unknown[]) {
  await page.route(
    (url) => url.pathname.endsWith("/chat/history") && url.searchParams.get("desk") === TEAMMATE,
    async (route) => {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(messages) });
    },
  );
}

test.beforeEach(async ({ page }) => {
  // The first-run tour swallows every click beneath it — see
  // `chat-history-loading.spec.ts`'s own copy of this workaround.
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

test("a pair case: the agent's own sentence, and a button to that agent's Model tab", async ({ page }) => {
  await mockHistory(page, [
    historyLine(1, {
      text: "This turn couldn't be finished — something went wrong or a step took too long. Nothing was left half-done. Send the message again to retry.",
      userFacing: true,
      code: "pair_provider_off",
      message:
        "Engineer uses E2E Rec A, which is switched off. Switch it on in Connections → API Keys → LLM, or choose another in Team → engineer → Model.",
      pairAgentId: TEAMMATE,
      providerSlug: "e2e-rec-a",
    }),
  ]);

  await openChannel(page, DM_CHANNEL);

  const bubble = bubbles(page).filter({ has: page.getByTestId("turn-failure-notice") });
  await expect(bubble).toBeVisible({ timeout: 30_000 });
  await expect(bubble).toContainText("Engineer uses E2E Rec A, which is switched off.");
  // Never the generic fallback text this same reply's own `text` still carries.
  await expect(bubble).not.toContainText("something went wrong or a step took too long");

  const action = bubble.getByTestId("turn-failure-action");
  await expect(action).toHaveText("Open Model settings");
  await expect(action).toHaveAttribute("href", `#/company/agent/${TEAMMATE}?tab=model`);
});

test("a default case: the company-default sentence, and a button to LLM settings", async ({ page }) => {
  await mockHistory(page, [
    historyLine(1, {
      userFacing: true,
      code: "default_provider_removed",
      message: "The company default uses E2E Rec C, which is removed. Choose a new default in Connections → API Keys → LLM.",
    }),
  ]);

  await openChannel(page, DM_CHANNEL);

  const bubble = bubbles(page).filter({ has: page.getByTestId("turn-failure-notice") });
  await expect(bubble).toBeVisible({ timeout: 30_000 });
  await expect(bubble).toContainText("The company default uses E2E Rec C, which is removed.");
  await expect(bubble).not.toContainText("something went wrong or a step took too long");

  const action = bubble.getByTestId("turn-failure-action");
  await expect(action).toHaveText("Open LLM settings");
  await expect(action).toHaveAttribute("href", "#/connections/inference");
});

test("every other failure keeps the generic text and no action button", async ({ page }) => {
  await mockHistory(page, [
    historyLine(1), // no userFacing/code/message — a generic provider/network failure
  ]);

  await openChannel(page, DM_CHANNEL);

  const bubble = bubbles(page).first();
  await expect(bubble).toBeVisible({ timeout: 30_000 });
  await expect(bubble).toContainText("This turn couldn't be finished");
  await expect(bubble.getByTestId("turn-failure-notice")).toHaveCount(0);
  await expect(bubble.getByTestId("turn-failure-action")).toHaveCount(0);
});
