import { expect, test, type APIRequestContext } from "@playwright/test";

import { HIVE, HIVE_REASON, LIVE_BRAIN, LIVE_BRAIN_REASON } from "./capabilities";
import { openChannel, say, silenceTour, SCOPE } from "./orchestration";

/**
 * **A desk answering through the company hive, watched from the console.**
 *
 * The company under test is `companies/hive_demo`. The brain is the scripted
 * mock, whose coordinator arm ends every episode turn with
 * `hivemind_complete` (after one `hivemind_send_agent` on the starter's turn
 * when the line carries `__MOCK_DM__ <agent>`) — so the episode is known to
 * settle, and what this spec asserts is that the console **shows** it:
 *
 * 1. the desk's replies are grouped as one episode (`episode-group`);
 * 2. the **settle marker** lands once `hive_episode_settled` arrives;
 * 3. `chat/history` carries the episode's rows with `hive.episodeId`, so a
 *    reload regroups the same transcript.
 */

/** How long an episode may take end to end. */
const EPISODE_TIMEOUT = 180_000;

/** The episode ids `chat/history` carries for a desk. */
async function historyEpisodes(request: APIRequestContext, desk: string): Promise<string[]> {
  const history = await request.get(`${SCOPE}/chat/history?desk=${encodeURIComponent(desk)}&limit=200`);
  expect(history.ok()).toBe(true);
  const rows = (await history.json()) as { hive?: { sequence: number; episodeId?: string } }[];
  return rows.map((row) => row.hive?.episodeId).filter((id): id is string => Boolean(id));
}

test("a desk's episode is grouped live and marked settled", async ({ page, request }) => {
  test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);
  test.skip(!HIVE, HIVE_REASON);
  test.setTimeout(EPISODE_TIMEOUT + 60_000);

  await silenceTour(page);
  await openChannel(page, "engineering");

  // The marker keeps the message unique across runs on the same data root.
  const stamp = Date.now();
  await say(page, `Plan the staging rollout __MOCK_SLOW_MS__ 500 __MOCK_DM__ ceo hive-${stamp}`);

  const marker = page.locator('[data-testid="episode-complete"][data-episode-status="settled"]');
  await expect(marker.last()).toBeVisible({ timeout: EPISODE_TIMEOUT });
  const episodeId = await marker.last().getAttribute("data-episode-id");
  expect(episodeId).toBeTruthy();

  await expect(page.locator(`[data-testid="episode-group"][data-episode-id="${episodeId}"]`)).toBeVisible();

  await expect
    .poll(async () => (await historyEpisodes(request, "engineering")).includes(episodeId!), {
      timeout: 30_000,
      message: "chat/history carries the episode's rows",
    })
    .toBe(true);
});

test("a settled episode regroups from history after a reload", async ({ page }) => {
  test.skip(!LIVE_BRAIN, LIVE_BRAIN_REASON);
  test.skip(!HIVE, HIVE_REASON);

  await silenceTour(page);
  await openChannel(page, "engineering");
  // The previous test left at least one episode on this desk. A fresh page has
  // seen no frames, so the grouping comes from `chat/history`'s `hive` field.
  await page.reload();
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
  await expect(page.getByTestId("episode-group").first()).toBeVisible({ timeout: 30_000 });
});
