import { expect, test, type Page } from "@playwright/test";

import { dmRow, mockCompany, ROSTER } from "./agent-states-mock";

/**
 * Every agent state, drawn: waiting for approval, working, typing, thinking,
 * queued, and inactive (no dot at all).
 *
 * Frames are pushed through a gated SSE stream (`agent-states-mock.ts`), the
 * technique of `chat-presence.spec.ts`. The proofs worth a browser are the ones
 * the pure `agent-presence` rules cannot reach: that the dot is on the DM row of
 * the right teammate, in the right shape, in both colour schemes, and that it
 * does not take the `presence-dot` testid a person's row is counted by.
 */

// Tall enough that all fifteen rows are on screen, so a screenshot shows the
// whole rail and a dot on any of them.
test.use({ viewport: { width: 1280, height: 1000 } });

const RAE = ROSTER[0];
const ADA = ROSTER[1];

const dotOf = (page: Page, name: string) => dmRow(page, name).getByTestId("agent-status-dot");

async function open(page: Page) {
  await page.goto("/#/chat");
  await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
  await expect(dmRow(page, RAE.name)).toBeVisible();
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`${scheme} scheme`, () => {
    test.use({ colorScheme: scheme });

    test("a thinking, a working and a typing agent each wear their own dot", async ({ page }) => {
      const sse = await mockCompany(page);
      await open(page);
      await expect(dotOf(page, RAE.name)).toHaveCount(0);

      // Thinking: a thinking frame and nothing else.
      sse.push({ type: "thinking", seq: 1, agentId: RAE.id, chatId: RAE.id });
      await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "thinking");
      // The row is announced name first: the dot is decorative there and the
      // state rides after the name, not "Thinking Rae Ceo".
      await expect(dmRow(page, RAE.name)).toHaveAccessibleName(/^Rae Ceo\W+Thinking$/);
      await expect(dotOf(page, RAE.name)).toHaveAttribute("aria-hidden", "true");
      await page.screenshot({ path: test.info().outputPath(`dot-thinking-${scheme}.png`) });

      // Working: a tool call is running.
      sse.push({
        type: "tool_call",
        seq: 2,
        agentId: RAE.id,
        chatId: RAE.id,
        toolCallId: "c1",
        label: "Search",
        status: "running",
      });
      await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "working");
      await page.screenshot({ path: test.info().outputPath(`dot-working-${scheme}.png`) });

      // Typing: the tool finished and the reply text began. Working gives way
      // because nothing is running any more.
      sse.push({
        type: "tool_result",
        seq: 3,
        agentId: RAE.id,
        chatId: RAE.id,
        toolCallId: "c1",
        label: "Search",
        status: "ok",
      });
      sse.push({ type: "replying", seq: 4, agentId: RAE.id, chatId: RAE.id });
      await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "typing");
      await page.screenshot({ path: test.info().outputPath(`dot-typing-${scheme}.png`) });

      // The reply landing clears it: inactive is no dot, not an "offline" one.
      sse.push({
        type: "agent_reply",
        seq: 5,
        atMillis: Date.now(),
        chatId: RAE.id,
        agentId: RAE.id,
        text: "Done.",
      });
      await expect(dotOf(page, RAE.name)).toHaveCount(0);
    });

    test("a hive seat's dm:<id> bracket lights the bare-id DM row and its pill", async ({ page }) => {
      // The shape a real DM turn has: the console addresses Rae's DM by the
      // bare id, and the hive seat that answers it brackets its turn under its
      // desk id, `dm:<id>`, streaming no frames. Every other case in this file
      // uses `chatId: agent.id`, which is how this mismatch hid.
      const sse = await mockCompany(page);
      await page.goto(`/#/chat/dm:${RAE.id}`);
      await expect(page.getByPlaceholder(/^Message /)).toBeVisible({ timeout: 30_000 });
      // The pill over the transcript is where the header's dot went.
      const pill = page.getByTestId("channel-pill");
      const header = pill.getByTestId("agent-status-dot");
      const seat = { chatId: `dm:${RAE.id}`, turnId: "seat-turn-1", agentId: RAE.id, episodeId: "ep-1", roundRevision: 1 };
      sse.push({ type: "turn_started", seq: 1, atMillis: Date.now(), ...seat });
      await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "working");
      await expect(header).toHaveAttribute("data-state", "working");
      // Decorative there, as on the row: the pill is the control, named for
      // the conversation, and the dot is not announced a second time.
      await expect(header).toHaveAttribute("aria-hidden", "true");
      await expect(pill).toHaveAccessibleName(RAE.name);
      await page.screenshot({ path: test.info().outputPath(`dot-seat-working-${scheme}.png`) });
      sse.push({ type: "turn_settled", seq: 2, atMillis: Date.now(), outcome: "committed", ...seat });
      await expect(dotOf(page, RAE.name)).toHaveCount(0);
      await expect(header).toHaveCount(0);
    });

    test("a DM turn another console sent reads queued, then thinking once its run holds the lock", async ({ page }) => {
      // The chat route brackets a turn with no agent; the DM's thread says whose
      // it is, and the run id is the bracket's turn id.
      const sse = await mockCompany(page);
      await open(page);
      const turn = { chatId: RAE.id, turnId: "run-elsewhere-1" };
      sse.push({ type: "turn_started", seq: 1, atMillis: Date.now(), ...turn });
      await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "queued");
      sse.push({
        type: "run_status_changed",
        seq: 2,
        atMillis: Date.now(),
        runId: turn.turnId,
        attempt: 1,
        status: "running",
        from: "pending",
      });
      await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "thinking");
      sse.push({ type: "turn_settled", seq: 3, atMillis: Date.now(), outcome: "committed", ...turn });
      await expect(dotOf(page, RAE.name)).toHaveCount(0);
    });

    test("the @ picker wears the dot on an agent's face, with the state after the name", async ({ page }) => {
      const sse = await mockCompany(page);
      await open(page);
      sse.push({ type: "thinking", seq: 1, agentId: RAE.id, chatId: RAE.id });
      await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "thinking");
      await page.getByPlaceholder(/^Message /).fill("@");
      const picker = page.getByTestId("mention-picker");
      await expect(picker).toBeVisible();
      const option = picker.getByRole("option").filter({ hasText: RAE.name });
      await expect(option.getByTestId("agent-status-dot")).toHaveAttribute("data-state", "thinking");
      await expect(option.getByTestId("agent-status-dot")).toHaveAttribute("aria-hidden", "true");
      await expect(option).toHaveAccessibleName(new RegExp(`${RAE.name}.*, Thinking$`));
      await page.screenshot({ path: test.info().outputPath(`dot-mention-picker-${scheme}.png`) });
    });

    test("typing has no expiry: it holds past the person-typing window", async ({ page }) => {
      const sse = await mockCompany(page);
      await open(page);
      sse.push({ type: "replying", seq: 1, agentId: RAE.id, chatId: RAE.id });
      await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "typing");
      // A person's `typing` frame lapses after 8s; an agent's reply can stream
      // for longer, and flipping back to "thinking" mid-reply would be a lie.
      await page.waitForTimeout(9_000);
      await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "typing");
    });

    test("a pending approval shows on the asking teammate and clears on resolve", async ({ page }) => {
      let approvals: unknown[] = [];
      const sse = await mockCompany(page, { approvals: () => approvals });
      await open(page);

      approvals = [
        { id: "ap-1", kind: "payment.send", amount_usd: 12, at_millis: Date.now(), agent: ADA.id, thread: null },
      ];
      sse.push({ type: "approval_parked", seq: 1, atMillis: Date.now(), approvalId: "ap-1", kind: "payment.send" });
      await expect(dotOf(page, ADA.name)).toHaveAttribute("data-state", "approval");
      await expect(dmRow(page, ADA.name)).toHaveAccessibleName(
        /^Ada Lovelace\W+Waiting for your approval$/,
      );
      await page.screenshot({ path: test.info().outputPath(`dot-approval-${scheme}.png`) });

      approvals = [];
      sse.push({ type: "approval_resolved", seq: 2, atMillis: Date.now(), approvalId: "ap-1", verdict: "approved" });
      await expect(dotOf(page, ADA.name)).toHaveCount(0);
    });

    test("an approval that expires clears on the frame, before the feed drops it", async ({ page }) => {
      // The feed keeps listing it on purpose: the dot must go out on the
      // `approval_resolved` frame itself (`automatic: true` is an expiry), not
      // on whichever poll finally stops returning the row.
      const approvals = [
        { id: "ap-2", kind: "payment.send", amount_usd: 12, at_millis: Date.now(), agent: ADA.id, thread: null },
      ];
      const sse = await mockCompany(page, { approvals: () => approvals });
      await open(page);
      await expect(dotOf(page, ADA.name)).toHaveAttribute("data-state", "approval");
      sse.push({
        type: "approval_resolved",
        seq: 1,
        atMillis: Date.now(),
        approvalId: "ap-2",
        verdict: "deny",
        automatic: true,
      });
      await expect(dotOf(page, ADA.name)).toHaveCount(0);
    });

    test("a queued turn reads queued, and no agent wears the person presence testid", async ({ page }) => {
      const runs = [{ id: "run-q", chatId: ADA.id, status: "pending", agentId: ADA.id }];
      await mockCompany(page, { runs: () => runs });
      await open(page);
      await expect(dotOf(page, ADA.name)).toHaveAttribute("data-state", "queued");
      await expect(dotOf(page, RAE.name)).toHaveCount(0);
      await page.screenshot({ path: test.info().outputPath(`dot-queued-${scheme}.png`) });
      // `chat-presence.spec.ts` counts `presence-dot` against the person rows;
      // an agent never wears it.
      await expect(page.getByTestId("room-rail-slot").getByTestId("presence-dot")).toHaveCount(0);
    });
  });
}

test.describe("reduced motion", () => {
  test.use({ contextOptions: { reducedMotion: "reduce" } });

  test("working, typing and thinking stay distinguishable without motion", async ({ page }) => {
    const sse = await mockCompany(page);
    await open(page);
    const shapes: string[] = [];

    sse.push({ type: "thinking", seq: 1, agentId: RAE.id, chatId: RAE.id });
    await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "thinking");
    shapes.push(await dotOf(page, RAE.name).innerHTML());

    sse.push({
      type: "tool_call",
      seq: 2,
      agentId: RAE.id,
      chatId: RAE.id,
      toolCallId: "c1",
      label: "Search",
      status: "running",
    });
    await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "working");
    shapes.push(await dotOf(page, RAE.name).innerHTML());

    sse.push({
      type: "tool_result",
      seq: 3,
      agentId: RAE.id,
      chatId: RAE.id,
      toolCallId: "c1",
      label: "Search",
      status: "ok",
    });
    sse.push({ type: "replying", seq: 4, agentId: RAE.id, chatId: RAE.id });
    await expect(dotOf(page, RAE.name)).toHaveAttribute("data-state", "typing");
    shapes.push(await dotOf(page, RAE.name).innerHTML());

    expect(new Set(shapes).size).toBe(3);
  });
});
