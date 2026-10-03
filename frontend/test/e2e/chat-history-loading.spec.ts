import { expect, test, type Page } from "@playwright/test";

/**
 * End-to-end proof for issue #934 — a channel never claims to be empty before
 * the host has said so.
 *
 * Reloading a DM that had history rendered "This is the start of your direct
 * message with …" and no messages, for as long as `chat/history` took to
 * answer. Nothing was lost — switching away and back showed the full thread —
 * but the copy is a positive claim about an empty channel, and an operator has
 * no way to tell a false one from real data loss.
 *
 * # Why this is an e2e spec and not a unit test
 *
 * `historyReady()` — the decision itself — *is* unit-tested, in
 * `test/unit/chat-history-hydration.test.ts`. What cannot be tested there is
 * the thing that actually broke: two independent fetches (`RoomView`'s own desk
 * list, `AppShell`'s hydration pass) racing to paint a pane. That race only
 * exists in a browser with a real host behind it, and the bug lived precisely
 * in the window between them.
 *
 * # Why the host's history is delayed rather than merely slow
 *
 * The defect is a timing window, and a timing window observed by luck is a
 * flaky test in both directions — it passes on a fast loopback host whether or
 * not the fix is present. So the DM's `chat/history` is held open on a latch
 * the test opens by hand: the loading state is asserted while the request is
 * provably still in flight, and the settled state after it provably is not.
 * Every other channel's history is left completely alone, which also proves the
 * hold is per channel and not a pane-wide freeze.
 *
 * Like the rest of `test/e2e` this drives a running host — see
 * `playwright.config.ts`.
 */

/** The harness roster's agent ids double as their DM thread ids. */
const TEAMMATE = "engineer";
/** …and a DM's console-local channel id is `dm:<teammate id>` (issue #364). */
const DM_CHANNEL = `dm:${TEAMMATE}`;

/** The claim under test. It must not appear until the host has justified it. */
/**
 * The copy the bug put up over a thread still on the wire. The intro line that
 * carried it has since been removed outright, so an empty conversation now
 * makes no claim at all — these assertions stay to keep it that way.
 */
const EMPTY_CLAIM = /This is the start of your direct message/;

/**
 * The first-run product tour renders a modal over the console and swallows
 * every click beneath it. Answer "already skipped" for whatever company id the
 * host resolves to rather than hard-coding the harness's.
 */
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
});

/**
 * Hold this one teammate's `chat/history` open, and answer it with `messages`
 * only when the returned latch is called.
 *
 * Matching on the `desk` query parameter is what keeps the hold surgical: the
 * shell fires one of these per channel on load, and delaying all of them would
 * prove nothing about which pane was waiting for which answer.
 */
async function holdHistory(page: Page, threadId: string, messages: unknown[]) {
  let release!: () => void;
  const latch = new Promise<void>((resolve) => {
    release = resolve;
  });
  let seen = false;
  await page.route(
    (url) =>
      url.pathname.endsWith("/chat/history") &&
      url.searchParams.get("desk") === threadId,
    async (route) => {
      seen = true;
      await latch;
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(messages),
      });
    },
  );
  return {
    /** Resolves once the console has actually asked for this channel's history. */
    requested: async () => {
      await expect.poll(() => seen, { timeout: 30_000 }).toBe(true);
    },
    release: () => release(),
  };
}

/** One journaled line, in the shape `GET .../chat/history` returns. */
function line(seq: number, from: "operator" | "agent", text: string) {
  return {
    seq,
    id: String(seq),
    role: from === "operator" ? "user" : "assistant",
    author: from === "operator" ? "operator" : TEAMMATE,
    text,
    at: "2026-08-17T09:00:00Z",
    desk: TEAMMATE,
  };
}

test("a reloaded DM waits for its history instead of calling itself new", async ({ page }) => {
  const history = await holdHistory(page, TEAMMATE, [
    line(1, "operator", "Where did we land on the migration?"),
    line(2, "agent", "Behind the flag, shipping Thursday."),
  ]);

  await page.goto(`/#/chat/${DM_CHANNEL}`);

  // The pane is up — composer rendered, channel resolved — and the history it
  // is going to render is still on the wire. This is the exact moment the bug
  // was visible.
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
  await history.requested();

  // The whole issue, in one assertion.
  await expect(page.getByText(EMPTY_CLAIM)).toHaveCount(0);
  // And something says so, rather than the pane just sitting blank.
  await expect(page.getByRole("status").filter({ hasText: /Loading messages/ })).toBeVisible();

  history.release();

  // In the transcript: the rail's preview of the DM says the last line too.
  const transcript = page.getByTestId("channel-transcript");
  await expect(transcript.getByText("Behind the flag, shipping Thursday.")).toBeVisible({
    timeout: 30_000,
  });
  await expect(transcript.getByText("Where did we land on the migration?")).toBeVisible();
  await expect(page.getByRole("status").filter({ hasText: /Loading messages/ })).toHaveCount(0);
  await expect(page.getByText(EMPTY_CLAIM)).toHaveCount(0);
});

test("a DM the host reports empty settles instead of loading forever", async ({ page }) => {
  // The other half of the fix: an empty answer is still an answer. Without
  // this, the guard could regress into a spinner that never resolves and no
  // test would notice.
  const history = await holdHistory(page, TEAMMATE, []);

  await page.goto(`/#/chat/${DM_CHANNEL}`);
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
  await history.requested();
  await expect(page.getByText(EMPTY_CLAIM)).toHaveCount(0);

  await expect(page.getByRole("status").filter({ hasText: /Loading messages/ })).toBeVisible();

  history.release();

  await expect(page.getByRole("status").filter({ hasText: /Loading messages/ })).toHaveCount(0, {
    timeout: 30_000,
  });
  await expect(page.getByText(EMPTY_CLAIM)).toHaveCount(0);
});
