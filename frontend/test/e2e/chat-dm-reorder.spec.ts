import { expect, test, type Page } from "@playwright/test";

import { dmRow, mockCompany, NAMES, railRows, ROSTER } from "./agent-states-mock";

/**
 * The Direct messages list at its real cap: fifteen teammates, the newest
 * message on top, and the row that just spoke sliding there.
 *
 * Order follows `latestMessageAt`, so an `agent_reply` frame for the last DM
 * re-sorts the list. Three properties, none of which the pure hook tests can
 * see in a real layout:
 *
 *  - the row lands on top, in both colour schemes, with long names truncated
 *    rather than wrapped in the 15rem rail;
 *  - it snaps (no animation) under `prefers-reduced-motion`;
 *  - the order is held while the pointer is inside the rail, so a row cannot
 *    slide under a click (#1414), and reconciles when the pointer leaves.
 */

// Tall enough that all fifteen rows are on screen, so a screenshot shows the
// whole rail and a dot on any of them.
test.use({ viewport: { width: 1280, height: 1000 } });

const LAST = ROSTER[ROSTER.length - 1];

async function open(page: Page) {
  await page.goto("/#/chat");
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
  // Every roster member is a DM row: the cap, not a sample.
  await expect(dmRow(page, NAMES[0])).toBeVisible();
  await expect(dmRow(page, LAST.name)).toBeVisible();
}

const reply = (agent: { id: string }, seq: number) => ({
  type: "agent_reply",
  seq,
  atMillis: Date.now() + seq,
  chatId: agent.id,
  agentId: agent.id,
  text: `Hello from ${agent.id}`,
});

async function firstRowName(page: Page) {
  // The row's name span, not its text: the avatar tile's initials come first.
  return (await railRows(page).first().locator("span.truncate").first().innerText()).trim();
}

/** Counts Web Animations started on rail rows, so a test can assert none (or some). */
async function countRailAnimations(page: Page) {
  await page.evaluate(() => {
    const w = window as unknown as { __railAnimations: number };
    w.__railAnimations = 0;
    const original = Element.prototype.animate;
    Element.prototype.animate = function patched(this: Element, ...args: Parameters<Element["animate"]>) {
      if (this.closest('[data-testid="room-rail-slot"]')) w.__railAnimations += 1;
      return original.apply(this, args);
    };
  });
  return () =>
    page.evaluate(() => (window as unknown as { __railAnimations: number }).__railAnimations);
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`${scheme} scheme`, () => {
    test.use({ colorScheme: scheme });

    test("a new message moves the last DM to the top; long names stay on one line", async ({ page }) => {
      const sse = await mockCompany(page);
      await open(page);
      // Park the pointer outside the rail so nothing holds the order.
      await page.mouse.move(700, 400);
      await page.screenshot({ path: test.info().outputPath(`rail-15-before-${scheme}.png`) });

      const before = await dmRow(page, LAST.name).boundingBox();
      sse.push(reply(LAST, 1));
      await expect.poll(() => firstRowName(page)).toBe(LAST.name);
      // The slide starts from the old slot, so wait for it to land: a bounding
      // box read at the first frame is still where the row used to be.
      await expect
        .poll(async () => (await dmRow(page, LAST.name).boundingBox())!.y, { timeout: 5_000 })
        .toBeLessThan(before!.y);
      await page.screenshot({ path: test.info().outputPath(`rail-15-after-${scheme}.png`) });

      // The longest name is clipped to one line, not wrapped onto a second
      // (the row itself is two lines: the name, then the latest message).
      const name = dmRow(page, NAMES[NAMES.length - 1]).getByTestId("channel-name");
      const nameBox = await name.boundingBox();
      const lineHeight = await name.evaluate((el) => parseFloat(getComputedStyle(el).lineHeight));
      expect(nameBox!.height).toBeLessThanOrEqual(Math.ceil(lineHeight) + 1);
    });

    test("a channel appearing above does not slide the DM rows; a re-sort still does", async ({ page }) => {
      // The height above the DM rows changes without anything re-sorting. This
      // used to be collapsing the Channels section, then creating a channel from
      // the rail's own "+" menu; both are gone (desks are made on the org chart
      // now), so the channel arrives the way one made elsewhere does — the host
      // lists it and a membership frame tells the console to re-read. It lands
      // above every DM and pushes them down by one row.
      let desks: unknown[] = [];
      const sse = await mockCompany(page, { desks: () => desks });
      await open(page);
      await page.mouse.move(700, 400);
      const started = await countRailAnimations(page);
      const before = (await dmRow(page, LAST.name).boundingBox())!.y;

      desks = [{ id: "launch-week", name: "Launch week", kind: "desk", members: [ROSTER[0].id] }];
      sse.push({
        type: "desk_members_changed",
        seq: 1,
        atMillis: Date.now(),
        deskId: "launch-week",
        added: [ROSTER[0].id],
        removed: [],
      });
      await expect(railRows(page).first()).toHaveAccessibleName(/^launch-week/);

      // The whole DM list moved DOWN, and moving is all it did: nothing re-sorted.
      await expect
        .poll(async () => (await dmRow(page, LAST.name).boundingBox())!.y)
        .toBeGreaterThan(before);
      expect(await started()).toBe(0);
      await page.screenshot({ path: test.info().outputPath(`rail-15-channel-added-${scheme}.png`) });

      // The new channel is the first row now, so the DM that spoke lands second.
      sse.push(reply(LAST, 2));
      await expect
        .poll(async () =>
          (await railRows(page).nth(1).locator("span.truncate").first().innerText()).trim(),
        )
        .toBe(LAST.name);
      expect(await started()).toBeGreaterThan(0);
    });

    test("holds the order while the pointer is in the rail, then reconciles", async ({ page }) => {
      const sse = await mockCompany(page);
      await open(page);
      const head = await firstRowName(page);
      await page.getByTestId("room-rail-slot").hover();
      sse.push(reply(LAST, 1));
      // Give the frame time to land: the order must not have moved under the pointer.
      await page.waitForTimeout(1_000);
      expect(await firstRowName(page)).toBe(head);
      await page.mouse.move(700, 400);
      await expect.poll(() => firstRowName(page)).toBe(LAST.name);
    });
  });
}

test("a clicked row's focus does not freeze the order once the pointer leaves", async ({ page }) => {
  const sse = await mockCompany(page);
  await open(page);
  // A mouse click opens the conversation and hands focus to its composer, so
  // you can type straight away — the row does not keep it. Either way only
  // keyboard focus in the rail may hold the order on its own.
  await dmRow(page, NAMES[3]).click();
  await page.mouse.move(700, 400);
  await expect(page.getByPlaceholder(`Message ${NAMES[3]}`)).toBeFocused();
  sse.push(reply(LAST, 1));
  await expect.poll(() => firstRowName(page)).toBe(LAST.name);
});

test("keyboard focus in the rail still holds the order", async ({ page }) => {
  const sse = await mockCompany(page);
  await open(page);
  await page.mouse.move(700, 400);
  const head = await firstRowName(page);
  await dmRow(page, NAMES[3]).focus();
  await page.keyboard.press("Tab");
  sse.push(reply(LAST, 1));
  await page.waitForTimeout(1_000);
  expect(await firstRowName(page)).toBe(head);
});

test("an ordinary re-sort plays the slide", async ({ page }) => {
  const sse = await mockCompany(page);
  await open(page);
  await page.mouse.move(700, 400);
  const started = await countRailAnimations(page);
  sse.push(reply(LAST, 1));
  await expect.poll(() => firstRowName(page)).toBe(LAST.name);
  expect(await started()).toBeGreaterThan(0);
});

test.describe("reduced motion", () => {
  test.use({ contextOptions: { reducedMotion: "reduce" } });

  test("re-sorts without starting an animation", async ({ page }) => {
    const sse = await mockCompany(page);
    await open(page);
    await page.mouse.move(700, 400);
    const started = await countRailAnimations(page);
    sse.push(reply(LAST, 1));
    await expect.poll(() => firstRowName(page)).toBe(LAST.name);
    expect(await started()).toBe(0);
  });
});
