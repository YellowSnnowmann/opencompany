import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

import { LIVE_BRAIN, LIVE_BRAIN_REASON } from "./capabilities";

/**
 * End-to-end proof for issue #377 — a completed dispatch says so in the channel
 * the card came from.
 *
 * The defect this closes is a reader being **misled**, which is why it needs a
 * browser. A card dispatched from a channel can park in `paused` or bounce back
 * to `todo`, and all the channel showed was the agent's relay prose — so
 * someone watching the conversation, or arriving fresh after a reload,
 * reasonably concluded the work had finished. No unit test can observe that,
 * because the wrong impression is made of two correct halves: real prose, and a
 * missing structural line.
 *
 * Two tests, deliberately in two lanes:
 *
 * 1. **The addressing**, against a default host with a written stream. What
 *    went wrong historically in this file's neighbourhood was never the event —
 *    it was which channel it reached (#368 filed a decision into whatever
 *    channel was open; #367 filed live replies into a store the visible surface
 *    did not read). A default-feature host has no harness and cannot settle a
 *    dispatch, so the frames are written here, exactly as
 *    `chat-live-events.spec.ts` writes its `tool_call` rows for the same
 *    reason. This runs on every push.
 * 2. **The whole round trip**, behind the live-brain lane: a card raised from a
 *    channel, really dispatched, really settled by the harness, and — the half
 *    that only a reload can prove — still exactly one marker afterwards. The
 *    live line and its rehydrated twin are two different writers putting the
 *    same message into one transcript, which is precisely the shape that
 *    produced issue #483's duplicate.
 */

/** The single-company alias the host answers on, for out-of-band writes. */
const SCOPE = "/api/v1/company";

/** The harness manifest's two desks. A desk's channel id is its thread id. */
const ENGINEERING = "engineering";
const CONTENT = "content";

test.beforeEach(async ({ page }) => {
  // The first-run tour opens a modal over the console and swallows every click
  // beneath it. Answer "already skipped" for whatever company id the host
  // resolves to rather than hard-coding the harness's.
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

/** Opens one channel by id and waits for its composer, i.e. for the view. */
async function openChannel(page: Page, channelId: string) {
  await page.goto(`/#/chat/${channelId}`);
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
}

/**
 * Every dispatch marker rendered in the open transcript.
 *
 * Matched as a **link**, which does two jobs at once: it is the card link the
 * marker exists to offer, and it excludes the rail's one-line channel preview —
 * a button carrying the same text — which would otherwise make every count
 * assertion here read one too many, intermittently, depending on which
 * rendered first (the trap `chat-live-events.spec.ts` documents).
 */
function markers(page: Page) {
  return page.getByRole("link", { name: /^finished → / });
}

/**
 * The marker count, once the channel's rehydration has stopped adding to it.
 *
 * A channel opens empty and fills from `chat/history` a moment later, so a
 * count taken on arrival is a count of nothing — and a "no new marker appeared"
 * assertion against it would be measuring the hydration instead. Waits for two
 * equal readings rather than a fixed sleep, the same shape
 * `chat-live-events.spec.ts` uses for its bubble counts and for the same
 * reason: this suite shares one host and one data root across tests, so an
 * earlier test's marker is legitimately in this channel's history.
 */
async function settledMarkerCount(page: Page): Promise<number> {
  let last = -1;
  await expect
    .poll(
      async () => {
        const current = await markers(page).count();
        const settled = current === last;
        last = current;
        return settled;
      },
      { intervals: [400, 400, 400, 400, 400, 400, 400, 400], timeout: 20_000 },
    )
    .toBe(true);
  return last;
}

test("a settled dispatch marks the channel its card was raised in — and only that one", async ({
  page,
}) => {
  const raised = `t-raised-${Date.now()}`;

  // The exact shape `project_event` puts on the wire for a settled dispatch:
  // structural fields plus the conversation the card was raised from, and
  // deliberately no `output` (issue #377 stopped projecting the run's prose —
  // the relay bubble already carries it into this same channel).
  const frames = [
    {
      type: "desk_task_completed",
      seq: 90_001,
      atMillis: Date.now(),
      taskId: raised,
      // The responder is an agent id, never a channel id. A console that
      // routed on this instead of on `chatId` would file the marker nowhere —
      // which is the reason the origin is carried at all.
      desk: "engineer",
      column: "paused",
      chatId: ENGINEERING,
    },
    // A card nobody raised from a conversation: opened on the board, or by a
    // scheduler. It belongs to no channel, and the console must write it to
    // none rather than guessing at one.
    {
      type: "desk_task_completed",
      seq: 90_002,
      atMillis: Date.now(),
      taskId: `t-boardonly-${Date.now()}`,
      desk: "engineer",
      column: "in_review",
    },
  ];
  await page.route("**/events", (route) =>
    route.fulfill({
      status: 200,
      headers: { "content-type": "text/event-stream", "cache-control": "no-cache" },
      body: frames.map((f) => `data: ${JSON.stringify(f)}\n\n`).join(""),
    }),
  );

  await openChannel(page, ENGINEERING);

  // The parked landing is the motivating case: the prose reads like an answer,
  // and without this line nothing says the card stopped short of finishing.
  await expect(markers(page)).toHaveCount(1, { timeout: 30_000 });
  await expect(markers(page)).toHaveText("finished → Paused");
  expect(await markers(page).getAttribute("href")).toBe(`#/tasks/${raised}`);

  // The origin-less frame wrote nothing here — and nothing anywhere else
  // either. A fallback to "whatever channel is open" is issue #368's bug, and
  // it would be worse than a missing marker: it would tell a reader a card
  // settled in a conversation that never raised it.
  await openChannel(page, CONTENT);
  await expect(markers(page)).toHaveCount(0);
});

/**
 * Raises a card from a channel the way the console does — the same
 * `originChatId` the chat "Add to board" action sends — then dispatches it by
 * moving it into the one column that fires a run.
 *
 * Driven over the API rather than through the board's drag gesture: what is
 * under test here is the settle reaching the channel, and `board-drag.spec.ts`
 * already owns the gesture. A drag here would couple this proof to that one.
 */
async function raiseAndDispatch(request: APIRequestContext, title: string, chatId: string) {
  const created = await request.post(`${SCOPE}/tasks`, {
    data: { title, originChatId: chatId },
  });
  expect(
    created.ok(),
    `raising the card failed: ${created.status()} ${await created.text()}`,
  ).toBeTruthy();
  const id = (await created.json()).id as string;

  const dispatched = await request.patch(`${SCOPE}/tasks/${id}`, {
    // The phase word the host resolves to `in_progress` (issue #1512).
    data: { column: "working" },
  });
  expect(
    dispatched.ok(),
    `dispatching failed: ${dispatched.status()} ${await dispatched.text()}`,
  ).toBeTruthy();
  return id;
}

test("the marker lands live and survives a reload exactly once", async ({ page, request }) => {
  // Needs a host that can actually run the card. Without the harness a
  // dispatch settles nothing and there is no terminal to observe.
  test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);
  // A real dispatch is a model round trip plus a settle, then a reload and a
  // second rehydration. The suite's 60s default would expire inside the first
  // wait below and report a *timeout* rather than a missing marker — which is
  // the least useful failure this spec could produce.
  test.setTimeout(240_000);

  await openChannel(page, ENGINEERING);

  const title = `dispatch-marker ${Date.now()}`;
  const id = await raiseAndDispatch(request, title, ENGINEERING);

  // Addressed by the card's own href rather than by the marker text, so a
  // marker left in this channel by an earlier run cannot be mistaken for this
  // one — the harness company's data root outlives a single test.
  const marker = page.locator(`a[href="#/tasks/${id}"]`);

  // Live: the settle reaches the open channel with no reload. *Which* column it
  // lands in depends on how the scripted run ends, and that is not what this
  // asserts — the point is that something structural says it settled, and says
  // where.
  await expect(marker).toHaveCount(1, { timeout: 90_000 });
  await expect(marker).toHaveText(/^finished → /);
  const text = await marker.textContent();

  // The reload is the half no unit test reaches. The live line came from the
  // SSE injection; this one comes from `chat/history`. Two writers, one
  // message — the shape that produced #483's duplicate — so the *count* is the
  // assertion that matters, not the presence.
  await page.reload();
  await openChannel(page, ENGINEERING);
  await expect(marker).toHaveCount(1, { timeout: 30_000 });
  expect(await marker.textContent()).toBe(text);

  // …and it settles at one rather than merely passing through it.
  await page.waitForTimeout(3_000);
  await expect(marker).toHaveCount(1);
});

test("a card raised on the board leaves no channel marker", async ({ page, request }) => {
  test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);
  // Proving an absence means waiting out the run that could have produced it,
  // so this needs headroom over the default too.
  test.setTimeout(120_000);

  await openChannel(page, ENGINEERING);
  // Settle the baseline before touching anything. This suite shares one host
  // and one data root, so a marker an earlier test legitimately left in this
  // channel's history is still arriving while the page loads — counting before
  // that stops would measure the hydration, not this card.
  const before = await settledMarkerCount(page);

  // No `originChatId`: the board's own "+" opens a card exactly like this. No
  // conversation raised it, so no conversation is told it settled.
  const created = await request.post(`${SCOPE}/tasks`, {
    data: { title: `board-only ${Date.now()}` },
  });
  expect(created.ok()).toBeTruthy();
  const id = (await created.json()).id as string;
  expect(
    (await request.patch(`${SCOPE}/tasks/${id}`, { data: { column: "working" } })).ok(),
  ).toBeTruthy();

  // Wait for the dispatch to actually *settle*, not merely for time to pass.
  //
  // A fixed sleep here would make this test pass for the wrong reason: if the
  // run were still `in_progress` when the clock ran out, no terminal has fired,
  // so no marker could exist yet and the absence below would be vacuous — green
  // whether or not an originless completion wrongly marks a channel. Polling
  // the card's own column means the assertion only runs once the event that
  // could have produced a marker has been emitted.
  await expect
    .poll(
      async () => {
        const res = await request.get(`${SCOPE}/tasks/${id}`);
        // A transient read keeps the poll waiting rather than concluding.
        if (!res.ok()) return "in_progress";
        // `GET /tasks/{id}` answers with a TaskDetail — the card is under
        // `task`, as every other board spec reads it. Deliberately not
        // defaulted: a missing column means the shape moved, and swallowing
        // that would turn a contract change back into a silent timeout.
        // The **stage**, not the phase: `working` covers both the dispatched
        // card and the parked one, so polling the phase could never observe
        // the settle this waits for (issue #1512).
        const task = (await res.json()).task as
          | { column?: string; stage?: string }
          | undefined;
        if (task?.column === undefined) throw new Error("task detail carried no column");
        return task.stage ?? task.column;
      },
      { timeout: 60_000, intervals: [1_000] },
    )
    .not.toBe("in_progress");
  // The load-bearing assertion: nothing anywhere links *this* card. It is
  // addressed by href rather than by count, so it cannot be satisfied or
  // broken by any other test's marker.
  await expect(page.locator(`[href="#/tasks/${id}"]`)).toHaveCount(0);
  // …and the channel grew nothing at all.
  await expect(markers(page)).toHaveCount(before);
});
