import { expect, type Locator, type Page } from "@playwright/test";

/**
 * Shared locators for the chat e2e specs (issue #1000). These used to be
 * redefined in each of the three specs that need them; a change to
 * `article[data-message-id]` or to the `working-indicator` test id therefore
 * required the same edit in every file. Kept here next to `./capabilities` so
 * the specs import one definition instead of drifting apart.
 */

/**
 * Opens one channel by id and waits for its transcript to be on screen.
 */
export async function openChannel(page: Page, channelId: string) {
  await page.goto(`/#/chat/${channelId}`);
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
}

/** Every non-system bubble currently rendered in the open channel. */
export function bubbles(page: Page): Locator {
  return page.locator("article[data-message-id]");
}

/**
 * The bubble carrying the reply to `marker`, in the open transcript.
 *
 * Scoped to a bubble, not matched across the page: the rail renders a one-line
 * preview of each channel's last message, so a bare
 * `getByText("You said: <marker>")` resolves to two elements the moment the
 * preview catches up. Found while standing up the live-brain lane (#467).
 */
export function reply(page: Page, marker: string): Locator {
  return bubbles(page).filter({ hasText: `You said: ${marker}` });
}

/** The working/queued row, however it is currently worded. */
export function workingRow(page: Page): Locator {
  return page.getByTestId("working-indicator");
}

/**
 * The live receipt for a turn this console just sent (issue #1934), which now
 * rides a detached turn past its 202 into the queued/working window (issue
 * #2021) — so on a same-session send it, not the bare {@link workingRow}, is the
 * in-flight indicator.
 */
export function liveReceipt(page: Page): Locator {
  return page.getByTestId("chat-live-receipt");
}

/**
 * The pill floating over the top of the transcript — the open conversation's
 * face and name, where the channel header bar used to be. It toggles the info
 * panel and says which way through `aria-expanded`.
 */
export function channelPill(page: Page): Locator {
  return page.getByTestId("channel-pill");
}

/**
 * The info panel on the right: the conversation's details and members, which
 * the header and its members pane used to carry.
 */
export function channelInfo(page: Page): Locator {
  return page.getByTestId("channel-info-panel");
}

/**
 * Open the info panel if it is shut.
 *
 * Not a plain click: the panel's open state lives in `RoomView` and survives a
 * hash-only channel switch, so a second click would close what the first
 * opened.
 */
export async function openChannelInfo(page: Page): Promise<Locator> {
  const pill = channelPill(page);
  await expect(pill).toBeVisible({ timeout: 30_000 });
  if ((await pill.getAttribute("aria-expanded")) !== "true") await pill.click();
  await expect(channelInfo(page)).toBeVisible();
  return channelInfo(page);
}
