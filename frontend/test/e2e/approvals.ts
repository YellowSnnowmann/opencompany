import { expect, type APIRequestContext } from "@playwright/test";

import { SCOPE } from "./orchestration";

/**
 * Reads of the host's own approval queue, for specs that decide a blocker from
 * the console and then need to know the host agreed.
 *
 * A click on a decide control is settled the moment the DOM event dispatches.
 * The host's side of that verdict is four steps — drop the approval from the
 * parked set, journal it, mint the grant, run the follow-up turn — and the
 * console learns none of it except by polling. So "the card left the screen" is
 * a render, some poll cycles behind a decision that may or may not have been
 * taken; what the host itself says is here.
 */

/**
 * Waits until the host no longer lists `id` among the parked approvals.
 *
 * `GET …/approvals` answers with the **parked** queue, and dropping the
 * approval from it is the host's first step in resolving one — so the id going
 * absent is the host saying it took the verdict, not the console saying it sent
 * one.
 */
export async function awaitResolvedByHost(
  request: APIRequestContext,
  id: string,
  verdict: string,
  timeout = 120_000,
): Promise<void> {
  await expect
    .poll(
      async () => {
        const queue = await request.get(`${SCOPE}/approvals`);
        // A queue that cannot be read has not resolved anything. Failing here,
        // on the host, names the host — rather than letting an unreachable one
        // surface later as a card that never left the screen.
        if (!queue.ok()) return false;
        return !((await queue.json()) as { id: string }[]).some((a) => a.id === id);
      },
      { timeout, message: `the host still lists the ${verdict} blocker (${id}) as parked` },
    )
    .toBe(true);
}
