import { expect, test, type Page } from "@playwright/test";

import { channelInfo, openChannelInfo } from "./chat-helpers";

/**
 * End-to-end proof for presence and typing indicators.
 *
 * Both features are streams of timed frames, and the parts worth proving in a
 * browser are the ones the pure rules cannot reach:
 *
 * - a presence frame actually moves a dot in the members pane;
 * - **no dot is not the same as an offline dot** — presence is replica-local,
 *   so somebody this host has never heard from is *unknown*, and claiming they
 *   are offline would be a confident lie;
 * - a typing frame renders a line under the composer, and that line clears on
 *   its own when the renewals stop.
 *
 * Frames are pushed through a mocked SSE response rather than by driving a
 * second browser: the interesting inputs are a frame at a chosen instant and
 * then deliberate silence, and a second real console can produce neither on
 * demand.
 */

const COMPANY = "acme";

const DESKS = [
  { id: "engineering", name: "Engineering", description: "Ships it", members: ["ceo"] },
];
const ROSTER = [{ id: "ceo", name: "Rae", role: "Chief Executive" }];

const PEOPLE = [
  { id: "u-ada", label: "Ada Lovelace", slug: "ada-lovelace" },
  { id: "u-grace", label: "Grace Hopper", slug: "grace-hopper" },
];

const MENTIONABLES = {
  agents: ROSTER,
  people: PEOPLE,
  desks: [{ id: "engineering", name: "Engineering", memberIds: ["ceo"] }],
  everyone: { label: "everyone", aliases: ["everyone", "channel", "here"] },
};

/** Presence rows the host reports on the initial read. */
type Seed = Array<{ userId: string; status: string; atMillis: number }>;

async function mockApi(page: Page, opts: { seed?: Seed; frames?: string[] } = {}) {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });

  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const status = { id: COMPANY, name: "Acme", lifecycle: "running", pending_approvals: 0 };

    if (path === "/api/v1/companies") return json([status]);
    if (path === `/api/v1/companies/${COMPANY}`) return json(status);
    if (path.endsWith("/desks")) return json(DESKS);
    if (path.endsWith("/team")) return json(ROSTER);
    if (path.endsWith("/chat/mentionables")) return json(MENTIONABLES);
    if (path.endsWith("/chat/read-state")) return json({ markers: [] });
    if (path.endsWith("/chat/history")) return json([]);
    if (path.endsWith("/presence")) {
      if (route.request().method() === "GET") return json({ people: opts.seed ?? [] });
      return route.fulfill({ status: 204, body: "" });
    }
    if (path.endsWith("/chat/typing")) return route.fulfill({ status: 204, body: "" });
    if (path.endsWith("/events")) {
      // One SSE response carrying the scripted frames, then held open.
      const body = (opts.frames ?? []).map((f) => `data: ${f}\n\n`).join("");
      return route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        headers: { "cache-control": "no-cache" },
        body,
      });
    }
    if (path.endsWith("/me")) return json({ id: "op", email: "op@example.com", role: "member" });
    return json([]);
  });
}

async function openChannel(page: Page, channelId: string) {
  await page.goto(`/#/chat/${channelId}`);
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
}

/** The channel's details panel — its People section carries the dots. */
const pane = channelInfo;
const openPane = openChannelInfo;

/** One person's row in the People section. */
function personRow(page: Page, label: string) {
  return pane(page).getByTestId("person-row").filter({ hasText: label });
}

test("a present person shows an online dot; an absent one shows no status", async ({ page }) => {
  await mockApi(page, {
    seed: [{ userId: "u-ada", status: "online", atMillis: Date.now() }],
  });
  await openChannel(page, "engineering");
  await openPane(page);

  await expect(personRow(page, "Ada Lovelace")).toBeVisible();
  await expect(personRow(page, "Ada Lovelace").getByTestId("presence-dot")).toHaveAttribute(
    "data-status",
    "online",
  );

  // Grace is in the directory but this replica has never heard from her.
  // "Unknown", not "offline" — presence is replica-local, and claiming she is
  // offline would be a confident statement the console cannot support.
  await expect(personRow(page, "Grace Hopper").getByTestId("presence-dot")).toHaveAttribute(
    "data-status",
    "unknown",
  );
  await expect(personRow(page, "Grace Hopper").getByTestId("presence-dot")).toHaveAttribute(
    "aria-label",
    /no recent activity/i,
  );
});

test("an away status renders distinctly from online", async ({ page }) => {
  await mockApi(page, {
    seed: [
      { userId: "u-ada", status: "online", atMillis: Date.now() },
      { userId: "u-grace", status: "away", atMillis: Date.now() },
    ],
  });
  await openChannel(page, "engineering");
  await openPane(page);

  await expect(personRow(page, "Ada Lovelace").getByTestId("presence-dot")).toHaveAttribute(
    "data-status",
    "online",
  );
  await expect(personRow(page, "Grace Hopper").getByTestId("presence-dot")).toHaveAttribute(
    "data-status",
    "away",
  );
});

/** Teammates are not people: an agent has no session and no machine to be at. */
test("an agent row carries no presence dot", async ({ page }) => {
  await mockApi(page, { seed: [{ userId: "u-ada", status: "online", atMillis: Date.now() }] });
  await openChannel(page, "engineering");
  await openPane(page);

  const agent = pane(page).getByText("Rae", { exact: false }).first();
  await expect(agent).toBeVisible();
  // Exactly as many dots as there are people rows — none of them on a teammate.
  const dots = await pane(page).getByTestId("presence-dot").count();
  const rows = await pane(page).getByTestId("person-row").count();
  expect(dots).toBe(rows);
});

test("a typing frame renders a line, and it clears when the renewals stop", async ({ page }) => {
  const now = Date.now();
  await mockApi(page, {
    seed: [{ userId: "u-ada", status: "online", atMillis: now }],
    frames: [
      JSON.stringify({
        type: "typing",
        userId: "u-ada",
        chatId: "engineering",
        atMillis: now,
      }),
    ],
  });
  await openChannel(page, "engineering");

  await expect(page.getByTestId("typing-line")).toHaveText(/Ada Lovelace is typing/);

  // No renewal arrives, so the indicator expires on its own. That is the whole
  // design: there is no "stopped typing" frame, and a console that closed
  // mid-word must clear itself with no teardown.
  await expect(page.getByTestId("typing-line")).toHaveCount(0, { timeout: 15_000 });
});

test("a typing frame for another channel does not leak into this one", async ({ page }) => {
  const now = Date.now();
  await mockApi(page, {
    seed: [{ userId: "u-ada", status: "online", atMillis: now }],
    frames: [
      JSON.stringify({
        type: "typing",
        userId: "u-ada",
        chatId: "some-other-desk",
        atMillis: now,
      }),
    ],
  });
  await openChannel(page, "engineering");

  // Give the frame time to arrive and be (correctly) ignored.
  await page.waitForTimeout(1_000);
  await expect(page.getByTestId("typing-line")).toHaveCount(0);
});

test("a host with no presence route degrades to no dots at all", async ({ page }) => {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
  await page.route("**/api/v1/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    const status = { id: COMPANY, name: "Acme", lifecycle: "running", pending_approvals: 0 };
    if (path === "/api/v1/companies") return json([status]);
    if (path === `/api/v1/companies/${COMPANY}`) return json(status);
    if (path.endsWith("/desks")) return json(DESKS);
    if (path.endsWith("/team")) return json(ROSTER);
    if (path.endsWith("/chat/mentionables")) return json(MENTIONABLES);
    if (path.endsWith("/chat/read-state")) return json({ markers: [] });
    if (path.endsWith("/chat/history")) return json([]);
    // The pre-feature host.
    if (path.endsWith("/presence")) return json({ error: "not_found" }, 404);
    if (path.endsWith("/events")) {
      return route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
    }
    if (path.endsWith("/me")) return json({ id: "op", email: "op@example.com", role: "member" });
    return json([]);
  });

  await openChannel(page, "engineering");
  await openPane(page);

  // People still list; they simply carry no live status, which is honest.
  await expect(personRow(page, "Ada Lovelace")).toBeVisible();
  await expect(personRow(page, "Ada Lovelace").getByTestId("presence-dot")).toHaveAttribute(
    "data-status",
    "unknown",
  );
  await expect(page.getByTestId("typing-line")).toHaveCount(0);
});
