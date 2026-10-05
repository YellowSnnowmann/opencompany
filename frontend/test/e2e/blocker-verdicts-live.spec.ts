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
 * "the request was accepted" would have passed against the bug. This reaches
 * each live decision control and waits for the host to remove the matching
 * blocker from its parked queue. Hive seats receive a consolidated release
 * note through the coordinator, so they do not publish the legacy per-DM
 * resume note.
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
/** A real teammate DM; all four verdicts run one at a time in this session. */
const threadFor = "dm:writer";

type Verdict = "retry" | "amend" | "skip" | "cancel";

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
 * channels that each see exactly one message sidesteps that outright.
 *
 * `detach: true` so each post returns before its turn finishes; the parks
 * happen inside that turn, so the queue is polled for them before moving on
 * to the next verdict. Each question carries a per-run stamp because this
 * suite shares one host and one data root — an approval another spec
 * legitimately parked must not be mistaken for one of these.
 */
async function askAndPark(
  request: APIRequestContext,
  verdict: Verdict,
  stamp: number,
): Promise<string> {
  const question = (verdict: Verdict) => `which cluster for ${verdict}-${stamp}?`;
  const call = { name: "escalate_to_human", arguments: { question: question(verdict) } };
  // The marker goes AFTER the payload, so two runs with identical steps stay
  // two plans rather than sharing one cursor.
  const directive = `__MOCK_PLAN__ ${JSON.stringify([[call], []])} blockers-${stamp}-${verdict}`;
  const posted = await request.post(`${SCOPE}/chat`, {
    data: { text: directive, chat: threadFor, detach: true },
  });
  expect(
    posted.ok(),
    `asking the ${verdict} question failed: ${posted.status()} ${await posted.text()}`,
  ).toBeTruthy();

  let id: string | undefined;
  await expect
    .poll(
      async () => {
        id = (await parkedIds(request, question))[verdict];
        return Boolean(id);
      },
      {
        timeout: 180_000,
        message: `the ${verdict} question never parked as a blocker (stamp ${stamp})`,
      },
    )
    .toBe(true);
  return id!;
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

  // A company agent has one durable session, so park and resolve one question
  // at a time in the same real teammate DM. This still exercises every verdict
  // against a live blocker while allowing the next turn to run after release.
  const stamp = Date.now();
  await page.goto("/#/approvals");

  // Park and resolve each verdict before starting the next agent turn.
  for (const verdict of VERDICTS) {
    const id = await askAndPark(request, verdict, stamp);
    const footer = card(page, id).getByTestId("approval-decide");
    await expect(footer).toBeVisible({ timeout: 30_000 });
    await expect(footer.getByRole("button", { name: /^Retry/ })).toBeVisible();
    await expect(footer.getByRole("button", { name: /^Answer this question/ })).toBeVisible();
    await expect(footer.getByRole("button", { name: /^Skip this step/ })).toBeVisible();
    await expect(footer.getByRole("button", { name: /^Cancel run/ })).toBeVisible();
    await expect(footer.getByRole("button", { name: /^Approve:/ })).toHaveCount(0);
    await expect(footer.getByRole("button", { name: /^Decline:/ })).toHaveCount(0);

    if (verdict === "retry") {
      await clickClearOfToasts(footer.getByRole("button", { name: /^Retry/ }));
    } else if (verdict === "amend") {
      await clickClearOfToasts(footer.getByRole("button", { name: /^Answer this question/ }));
      const send = footer.getByRole("button", { name: /^Send this answer/ });
      await expect(send, "a blank amend cannot be sent").toBeDisabled();
      await footer.getByRole("textbox", { name: /^Answer:/ }).fill(ANSWER);
      await expect(send).toBeEnabled();
      await clickClearOfToasts(send);
    } else if (verdict === "skip") {
      await clickClearOfToasts(footer.getByRole("button", { name: /^Skip this step/ }));
    } else {
      await clickClearOfToasts(footer.getByRole("button", { name: /^Cancel run/ }));
    }
    await awaitResolvedByHost(request, id, verdict);
    await leaveQueue(page);
    await expect(card(page, id)).toHaveCount(0, { timeout: 30_000 });
  }
});
