import { describe, expect, it, vi } from "vitest";

import { handleEvent } from "@/hooks/use-events";

/**
 * The room's grammar has to survive every hop between the host and the episode
 * fold, and the hops are not type-checked into existence: `cueText` is optional
 * at each one, so a boundary that forgets to copy it still compiles and still
 * type-checks. It simply arrives `undefined`, the fold sees no moves, and a live
 * deliberation renders as ordinary chat.
 *
 * This dispatcher is one such boundary — it rebuilds the callback payload field
 * by field rather than spreading the event — and it dropped the field on the
 * first attempt (Codex P1, PR #2329).
 */
describe("the live agent_reply dispatcher", () => {
  it("forwards the body the model wrote, not only the one the operator reads", () => {
    const onAgentReply = vi.fn();

    handleEvent(
      {
        type: "agent_reply",
        seq: 17,
        atMillis: 1_700_000_000_000,
        chatId: "returns",
        agentId: "refunds",
        text: "the swap is the customer's first preference",
        cueText: "!support #kettle ^16 the swap is the customer's first preference",
      },
      { onAgentReply } as Parameters<typeof handleEvent>[1],
    );

    expect(onAgentReply).toHaveBeenCalledTimes(1);
    expect(onAgentReply.mock.calls[0][0]).toMatchObject({
      text: "the swap is the customer's first preference",
      cueText: "!support #kettle ^16 the swap is the customer's first preference",
    });
  });

  it("leaves it absent when the host sends none", () => {
    // An older host, and every reply carrying no move — the fold falls back to
    // `text`, where the two are equal anyway.
    const onAgentReply = vi.fn();

    handleEvent(
      {
        type: "agent_reply",
        seq: 3,
        atMillis: 1_700_000_000_000,
        chatId: "general",
        agentId: "ceo",
        text: "here is the summary you asked for",
      },
      { onAgentReply } as Parameters<typeof handleEvent>[1],
    );

    expect(onAgentReply.mock.calls[0][0].cueText).toBeUndefined();
  });
});
