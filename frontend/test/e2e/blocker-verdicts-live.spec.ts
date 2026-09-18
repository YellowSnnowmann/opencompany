import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

import { awaitResolvedByHost } from "./approvals";
import { LIVE_BRAIN, LIVE_BRAIN_REASON } from "./capabilities";
import { clickClearOfToasts } from "./toasts";

/**
 * The live half of #2028: four **real** parked blockers, four real clicks, four
 * real resolves. Nothing about the decision is stubbed.
 *
 * Its sibling `approval-blocker-verdicts.spec.ts` intercepts the resolve and
 * asserts the body the console composes; it runs in every lane because it needs
 * no agent. This one lets each request reach the host and proves the other
 * half: the host accepts all four verdicts, and the one it acts on is the one
 * the operator clicked — not the two-value `approve`/`deny` the click rides on.
 *
 * That distinction is the whole issue. #2027 made all four verdicts work at the
 * engine while an operator could still reach only two, so a test that stops at
 * "the request was accepted" would have passed against the bug. The assertion
 * that does not is the resume note: the host writes a different sentence into
 * the blocker's thread for each verdict, so reading it back says which of the
 * four actually ran.
 *
 * ## The fixture is the product
 *
 * Each blocker is parked by asking the agent a question it cannot answer —
 * `escalate_to_human`, scripted through the mock brain. That is the real door:
 * a real turn, the real approval-request queue, a real park with a real thread
 * behind it. Nothing is written into the host's journal from outside, and
 * nothing has to be laid down before the run, so this proves the same thing on
 * a CI runner as it does on a laptop.
 *
 * ## Why it is gated
 *
 * An agent has to be able to call a tool for any of that to happen, which needs
 * the harness compiled in and an inference backend behind it. That is
 * {@link LIVE_BRAIN}, and the `Console E2E (live brain)` lane declares it.
 *
 * ## What this does NOT prove
 *
 * `escalate_to_human` parks with no {@link BlockerStep} by construction — the
 * tool has no run or card behind it — so this exercises the verdict surface and
 * the host's handling of it, not a workflow node re-entering. The node-level
 * consequences (a retry re-running the node once, a skip proceeding without
 * spending a turn, a cancel starting no run) are covered by the host's own
 * tests, which drive a real `BlockerStep::Node`. There is no console-reachable
 * way to park one of those, so it is not something this spec can honestly
 * assert.
 */

const SCOPE = "/api/v1/company";
/** The DM the questions are asked in, and where the resume notes land — one
 * channel per verdict, each seeing exactly one message (see `askAndParkAll`
 * for why a shared channel does not work here). */
const threadFor = (verdict: Verdict) => `dm:writer-${verdict}`;

/**
 * What the host writes back into the blocker's thread for each verdict —
 * mirrors `blocker_resume_note` in `src/company/runtime.rs`.
 *
 * Four different sentences is what makes this spec able to tell the verdicts
 * apart. Three of the four ride an `approve`, so a spec that only checked the
 * response could not distinguish a skip that worked from a skip silently
 * flattened into a retry — which is exactly the bug.
 */
const RESUME_NOTE = {
  retry: "Got it — picking that back up now.",
  amend: "Thanks — using that and carrying on from where it stopped.",
  skip: "Okay — skipping that and moving on.",
  cancel: "Okay — cancelled.",
} as const;

type Verdict = keyof typeof RESUME_NOTE;

/** The four, in the order the controls read. */
const VERDICTS: Verdict[] = ["retry", "amend", "skip", "cancel"];

const ANSWER = "use the staging cluster";

test.beforeEach(async ({ page }) => {
  // The first-run tour would open a modal over every click below.
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

/**
 * Parks one real blocker per verdict by asking the agent four questions it
 * cannot answer, and returns their approval ids keyed by verdict.
 *
 * **Four turns, one call each.** `escalate_to_human` now establishes the same
 * turn boundary `request_approval` always has (issue #2231): once a turn has
 * asked the operator something, every later sibling call in that turn is
 * refused rather than parked — "stop and wait for the decision" — even a
 * second, different question. Four calls bundled into one `__MOCK_PLAN__`
 * step, the way this used to work, now parks only the first and refuses the
 * other three, so each question needs its own turn.
 *
 * **One thread per verdict**, not one shared thread four times over: the host
 * quotes a channel's own prior messages back into the next prompt sent for
 * it, so a second `__MOCK_TOOL_CALL__` directive posted to the same channel is
 * a coin flip on which directive the mock brain actually serves. Four
 * channels that each see exactly one message sidesteps that outright, and
 * `noteCounts` below reads each verdict's resume note back off its own
 * channel to match.
 *
 * `detach: true` so each post returns before its turn finishes; the parks
 * happen inside that turn, so the queue is polled for them before moving on
 * to the next verdict. Each question carries a per-run stamp because this
 * suite shares one host and one data root — an approval another spec
 * legitimately parked must not be mistaken for one of these.
 */
async function askAndParkAll(
  request: APIRequestContext,
  stamp: number,
): Promise<Record<Verdict, string>> {
  const question = (verdict: Verdict) => `which cluster for ${verdict}-${stamp}?`;

  const ids: Partial<Record<Verdict, string>> = {};
  for (const verdict of VERDICTS) {
    const call = { name: "escalate_to_human", arguments: { question: question(verdict) } };
    // The marker goes AFTER the payload, so two runs with identical steps stay
    // two plans rather than sharing one cursor.
    const directive = `__MOCK_PLAN__ ${JSON.stringify([[call], []])} blockers-${stamp}-${verdict}`;
    const posted = await request.post(`${SCOPE}/chat`, {
      data: { text: directive, chat: threadFor(verdict), detach: true },
    });
    expect(
      posted.ok(),
      `asking the ${verdict} question failed: ${posted.status()} ${await posted.text()}`,
    ).toBeTruthy();

    await expect
      .poll(
        async () => {
          const found = await parkedIds(request, question);
          if (found[verdict]) ids[verdict] = found[verdict];
          return Boolean(ids[verdict]);
        },
        {
          timeout: 180_000,
          message: `the ${verdict} question never parked as a blocker (stamp ${stamp})`,
        },
      )
      .toBe(true);
  }
  return ids as Record<Verdict, string>;
}

/** The parked blocker ids raised by this run's questions, keyed by verdict. */
async function parkedIds(
  request: APIRequestContext,
  question: (verdict: Verdict) => string,
): Promise<Partial<Record<Verdict, string>>> {
  const queue = await request.get(`${SCOPE}/approvals`);
  if (!queue.ok()) return {};
  const parked = (await queue.json()) as {
    id: string;
    kind: string;
    payload?: { reason?: string };
  }[];
  const found: Partial<Record<Verdict, string>> = {};
  for (const verdict of VERDICTS) {
    const mine = parked.find(
      (a) => a.kind.startsWith("blocker.") && a.payload?.reason?.includes(question(verdict)),
    );
    if (mine) found[verdict] = mine.id;
  }
  return found;
}

/** How many of each verdict's resume note that verdict's own thread currently
 * holds — one thread per verdict now (see `askAndParkAll`), so each is read
 * back separately rather than filtered out of one shared history. */
async function noteCounts(request: APIRequestContext): Promise<Record<Verdict, number>> {
  const counts = {} as Record<Verdict, number>;
  for (const verdict of VERDICTS) {
    const history = await request.get(
      `${SCOPE}/chat/history?desk=${encodeURIComponent(threadFor(verdict))}&limit=500`,
    );
    const lines = history.ok()
      ? ((await history.json()) as { text?: string }[]).map((m) => m.text ?? "")
      : [];
    counts[verdict] = lines.filter((line) => line.includes(RESUME_NOTE[verdict])).length;
  }
  return counts;
}

const card = (page: Page, id: string) => page.locator(`[data-approval-id="${id}"]`);

/**
 * Move the operator's attention off the queue.
 *
 * `useStableList` freezes the rendered order — and holds removals — while the
 * pointer is over the queue or focus is inside it, and a decide click leaves
 * both true. This is what "moving away" means to that hook, so a decided card
 * can drop.
 */
async function leaveQueue(page: Page) {
  await page.mouse.move(0, 0);
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
}

test("every one of the four verdicts is reachable, and the host acts on the one clicked", async ({
  page,
  request,
}) => {
  // Parking a blocker means an agent calling a tool, which needs the harness
  // and something for it to think with.
  test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);
  // Four real agent turns, four resolves, and a poll for each. The suite's
  // default would expire inside the first park and report a timeout rather than
  // the behaviour under test.
  test.setTimeout(600_000);

  // Park one blocker per verdict, each asking something distinguishable, so the
  // right card can be addressed on a queue this suite shares.
  const stamp = Date.now();
  const ids = await askAndParkAll(request, stamp);

  // Counted as a DELTA, not from zero. This suite shares one host, and a
  // re-run or a retry against it would leave earlier notes in the thread — a
  // baseline of zero would then fail for a reason that has nothing to do with
  // the behaviour under test.
  const before = await noteCounts(request);

  await page.goto("/#/approvals");

  // The whole issue, on real cards: four controls, and not the two that
  // flattened them.
  for (const verdict of VERDICTS) {
    const footer = card(page, ids[verdict]).getByTestId("approval-decide");
    await expect(footer).toBeVisible({ timeout: 30_000 });
    await expect(footer.getByRole("button", { name: /^Retry/ })).toBeVisible();
    await expect(footer.getByRole("button", { name: /^Answer this question/ })).toBeVisible();
    await expect(footer.getByRole("button", { name: /^Skip this step/ })).toBeVisible();
    await expect(footer.getByRole("button", { name: /^Cancel run/ })).toBeVisible();
    await expect(footer.getByRole("button", { name: /^Approve:/ })).toHaveCount(0);
    await expect(footer.getByRole("button", { name: /^Decline:/ })).toHaveCount(0);
  }

  // Each verdict is settled the same way: click, then wait for the HOST to say
  // it took the decision, and only then read the card. Asked in that order the
  // last assertion is a render — one poll cycle of `useCompany` plus a paint —
  // rather than a whole resolve, which is a drop, a journal entry, a grant and
  // a follow-up turn observed through a five-second poll.

  // Retry.
  await clickClearOfToasts(card(page, ids.retry).getByRole("button", { name: /^Retry/ }));
  await awaitResolvedByHost(request, ids.retry, "retry");
  await leaveQueue(page);
  await expect(card(page, ids.retry)).toHaveCount(0, { timeout: 30_000 });

  // Amend — and the answer has to be typed before it can be sent at all.
  const amend = card(page, ids.amend);
  await clickClearOfToasts(amend.getByRole("button", { name: /^Answer this question/ }));
  const send = amend.getByRole("button", { name: /^Send this answer/ });
  await expect(send, "a blank amend cannot be sent").toBeDisabled();
  await amend.getByRole("textbox", { name: /^Answer:/ }).fill(ANSWER);
  await expect(send).toBeEnabled();
  await clickClearOfToasts(send);
  await awaitResolvedByHost(request, ids.amend, "amend");
  await leaveQueue(page);
  await expect(amend).toHaveCount(0, { timeout: 30_000 });

  // Skip.
  await clickClearOfToasts(card(page, ids.skip).getByRole("button", { name: /^Skip this step/ }));
  await awaitResolvedByHost(request, ids.skip, "skip");
  await leaveQueue(page);
  await expect(card(page, ids.skip)).toHaveCount(0, { timeout: 30_000 });

  // Cancel.
  await clickClearOfToasts(card(page, ids.cancel).getByRole("button", { name: /^Cancel run/ }));
  await awaitResolvedByHost(request, ids.cancel, "cancel");
  await leaveQueue(page);
  await expect(card(page, ids.cancel)).toHaveCount(0, { timeout: 30_000 });

  // The assertion that would have failed against the bug. Four clicks, four
  // different sentences: the host acted on the verdict the operator chose
  // rather than on the approve or deny it travelled under. Before #2028 all
  // three approves banked a retry, so this would read three "picking that back
  // up" notes and no skip.
  //
  // Exactly one new note per verdict, so this also catches a click recorded
  // twice or a verdict that fanned somewhere it should not have.
  await expect
    .poll(
      async () => {
        const now = await noteCounts(request);
        return VERDICTS.map((verdict) => now[verdict] - before[verdict]);
      },
      { timeout: 120_000, message: "the host's per-verdict resume notes never all landed" },
    )
    .toEqual(VERDICTS.map(() => 1));
});
