import { expect, test } from "@playwright/test";

import { LIVE_BRAIN } from "./capabilities";
import { bubbles, openChannel, workingRow } from "./chat-helpers";

/**
 * Where the live pair sits, and what it says, while a turn is still running.
 *
 * Three claims no unit test can make, because all three are about **rendered
 * order and rendered state** rather than about a pure function:
 *
 * 1. the live row is the LAST thing in the transcript, beneath every message
 *    journaled while the turn ran;
 * 2. it names whoever is working *now*, following the floor as it changes
 *    hands;
 * 3. a call parked on a sign-off reads as parked *while it waits*, not once
 *    the reply lands.
 *
 * Like `chat-live-events.spec.ts`'s synthetic fixture — whose pattern this
 * borrows wholesale — these write their own SSE stream. The offline brain this
 * suite runs against calls no tools and never delegates, so there is no live
 * multi-agent turn to watch without inventing one, and what is under test is
 * the rendering rather than the plumbing. The frames below are the exact shape
 * `turn_stream.rs` puts on the wire and `use-events.ts` types.
 *
 * Like the rest of `test/e2e` this needs a running host and is not a CI gate —
 * the Playwright config declares no `webServer`.
 */

/** The harness manifest's engineering desk. Its id is its channel id. */
const ENGINEERING = { id: "engineering", channel: "engineering-desk" };

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

/**
 * Opens the channel with `frames` already queued on the intercepted stream.
 *
 * The release dance is load-bearing and is why this is a helper rather than
 * three copies: Playwright counts a routed stream as pending navigation work,
 * so awaiting the navigation before releasing deadlocks — while fulfilling
 * immediately can deliver frames before the channel map has mounted. The
 * visible composer is the readiness boundary that threads between the two.
 */
async function openWithFrames(page: import("@playwright/test").Page, frames: unknown[]) {
  let releaseFrames: (() => void) | undefined;
  const framesReleased = new Promise<void>((resolve) => {
    releaseFrames = resolve;
  });
  let streamRequested: (() => void) | undefined;
  const streamIsWaiting = new Promise<void>((resolve) => {
    streamRequested = resolve;
  });
  await page.route("**/events**", async (route) => {
    streamRequested?.();
    await framesReleased;
    await route.fulfill({
      status: 200,
      headers: { "content-type": "text/event-stream", "cache-control": "no-cache" },
      body: frames.map((f) => `data: ${JSON.stringify(f)}\n\n`).join(""),
    });
  });

  const channelOpened = openChannel(page, ENGINEERING.id);
  await streamIsWaiting;
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
  releaseFrames?.();
  await channelOpened;
}

test("the live row stays below a message journaled while the turn runs", async ({ page }) => {
  test.skip(LIVE_BRAIN, "the default Console E2E lane covers the synthetic SSE rendering fixture");
  // The ordering claim, and the reason the pair is pinned to the foot. A turn
  // that journals lines as it works — a deliberating desk posts one per seat —
  // used to leave the pulsing row several messages up, claiming work had
  // finished before every line beneath it.
  const atMillis = Date.now();
  await openWithFrames(page, [
    {
      type: "tool_call",
      seq: 1,
      atMillis,
      chatId: ENGINEERING.id,
      agentId: "a-ada",
      toolCallId: "t1",
      label: "workspace_read",
    },
    // A line lands in the transcript WHILE that call is still running, which is
    // what a seat speaking mid-episode looks like from the console's side.
    {
      type: "agent_reply",
      seq: 2,
      atMillis: atMillis + 1,
      chatId: ENGINEERING.id,
      agentId: "a-ada",
      text: "Checking the roster now.",
    },
  ]);

  const live = workingRow(page);
  await expect(live).toBeVisible({ timeout: 30_000 });

  const lastBubble = bubbles(page).last();
  await expect(lastBubble).toContainText("Checking the roster now.");

  // Document order is the assertion: the live row must follow the message, not
  // precede it. `compareDocumentPosition` returns DOCUMENT_POSITION_FOLLOWING
  // (4) when the argument comes after the node it is called on.
  const liveFollowsMessage = await lastBubble.evaluate(
    (node, liveEl) => Boolean(node.compareDocumentPosition(liveEl as Node) & 4),
    await live.elementHandle(),
  );
  expect(liveFollowsMessage).toBe(true);
});

test("the live row names the agent working now, not the one who started", async ({ page }) => {
  test.skip(LIVE_BRAIN, "the default Console E2E lane covers the synthetic SSE rendering fixture");
  // One query, two agents — the shape a desk hand-off and a hive episode both
  // produce. The row used to latch the first agent it saw and sit there while
  // somebody else was visibly working.
  const atMillis = Date.now();
  await openWithFrames(page, [
    {
      type: "tool_call",
      seq: 1,
      atMillis,
      chatId: ENGINEERING.id,
      agentId: "a-ada",
      toolCallId: "t1",
      label: "workspace_list",
    },
    {
      type: "tool_result",
      seq: 2,
      atMillis: atMillis + 1,
      chatId: ENGINEERING.id,
      agentId: "a-ada",
      toolCallId: "t1",
      status: "ok",
      elapsedMs: 40,
    },
    // The floor passes. Same chat, same query, different seat.
    {
      type: "tool_call",
      seq: 3,
      atMillis: atMillis + 2,
      chatId: ENGINEERING.id,
      agentId: "a-grace",
      toolCallId: "t2",
      label: "workspace_search",
    },
  ]);

  // The newest running step names the line, and it belongs to the second agent.
  await expect(workingRow(page)).toContainText("workspace_search", { timeout: 30_000 });
  await expect(workingRow(page)).not.toContainText("workspace_list");
});

test("a call parked on a sign-off reads as parked while it waits", async ({ page }) => {
  test.skip(LIVE_BRAIN, "the default Console E2E lane covers the synthetic SSE rendering fixture");
  // The state the fold used to flatten into `ok`. A gated call rendered as one
  // that had succeeded for the whole time it sat waiting, then flipped to
  // "awaiting approval · didn't run" when the reply landed — so the one state
  // the operator could act on was the one the live timeline could not show.
  const atMillis = Date.now();
  await openWithFrames(page, [
    {
      type: "tool_call",
      seq: 1,
      atMillis,
      chatId: ENGINEERING.id,
      agentId: "a-ada",
      toolCallId: "t1",
      label: "composio_execute",
    },
    {
      type: "tool_result",
      seq: 2,
      atMillis: atMillis + 1,
      chatId: ENGINEERING.id,
      agentId: "a-ada",
      toolCallId: "t1",
      label: "composio_execute",
      status: "awaiting_approval",
    },
  ]);

  // No running step is left, so the line falls back to its generic wording
  // rather than naming a settled call — but the turn is still open, so the row
  // is still there. What must NOT happen is the row reading as finished work.
  await expect(workingRow(page)).toBeVisible({ timeout: 30_000 });
  await expect(page.getByText("didn't run")).toHaveCount(0);
});
