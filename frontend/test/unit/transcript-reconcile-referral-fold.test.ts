import { describe, expect, it } from "vitest";

import type { ReferralConversationDto } from "@/api/types";
import {
  type ChatMessage,
  mergeHistoryInOrder,
  reconcileTranscript,
} from "@/lib/chat";

/**
 * A crossing's exchange folds onto the row that ASKED — a row the transcript
 * already holds — so a re-read that only appends unseen ids drops precisely the
 * update it was issued to fetch.
 *
 * That is why an agent-to-agent DM rendered only after a reload: the live
 * `referral` frame triggered the re-read, the host returned the asking row with
 * its `referralConversation` folded in, and the id filter discarded it as
 * already-known (CodeRabbit, #2341).
 */

const row = (id: string, extra: Partial<ChatMessage> = {}): ChatMessage =>
  ({
    id,
    author: "planner",
    text: "what is the lag budget?",
    mine: false,
    // `mergeHistoryInOrder` orders on it, so a fixture without one is not a
    // row that path can actually fold.
    at: 1_000 + Number(id.replace(/\D/g, "")),
    ...extra,
  }) as ChatMessage;

describe("reconcileTranscript", () => {
  it("applies a fold that landed on a row already held", () => {
    const asked = row("h12");
    const existing = [row("h11"), asked];
    const crossing: ReferralConversationDto = {
      askerId: "planner",
      otherId: "sre",
      otherDeskId: "eng",
      otherDeskName: "Engineering",
      direct: true,
      lines: [
        {
          authorId: "planner",
          authorLabel: "",
          text: "what is the lag budget?",
          outbound: true,
        },
        {
          authorId: "sre",
          authorLabel: "sre",
          text: "which path?",
          outbound: false,
        },
      ],
    };
    const folded = row("h12", { referralConversation: crossing });

    const merged = reconcileTranscript(existing, [row("h11"), folded]);

    expect(merged).not.toBe(existing);
    expect(merged).toHaveLength(2);
    expect(merged[1]).toBe(folded);
    expect(merged[1].referralConversation).toEqual(crossing);
  });

  it("still appends rows the transcript has never seen", () => {
    const existing = [row("h11")];

    const merged = reconcileTranscript(existing, [row("h11"), row("h12")]);

    expect(merged.map((m) => m.id)).toEqual(["h11", "h12"]);
  });

  it("keeps a local row the durable read does not carry", () => {
    // A live row for a turn still running is not in `chat/history` yet, and a
    // re-read must not delete it.
    const live = row("m99");
    const existing = [row("h11"), live];

    const merged = reconcileTranscript(existing, [row("h11")]);

    expect(merged).toBe(existing);
    expect(merged).toContain(live);
  });

  it("returns the same array when nothing changed, so React can skip", () => {
    const existing = [row("h11"), row("h12")];

    // Distinct objects that are equal by value — which is exactly what a
    // round trip produces, since `fromHistory` re-parses every row. Comparing
    // identity here would report a change every single time and make the skip
    // dead code.
    const merged = reconcileTranscript(existing, [row("h11"), row("h12")]);

    expect(merged).toBe(existing);
  });
});

/**
 * The 5-second poll is the path that matters most, and it had the same fault
 * in mirror image: `reconcileTranscript` kept the held copy when it was equal,
 * while `mergeHistoryInOrder` kept the held copy *always*.
 *
 * So the poll fetched the finished crossing and discarded it in favour of the
 * one-message version caught mid-exchange. Only a full reload ever showed the
 * whole exchange — which is exactly how it presented live.
 */
describe("mergeHistoryInOrder", () => {
  const crossing = (messages: number): ReferralConversationDto => ({
    askerId: "cancellations",
    otherId: "amendments",
    otherDeskId: "order_ops",
    otherDeskName: "Order Operations",
    direct: true,
    lines: Array.from({ length: messages }, (_, i) => ({
      authorId: i % 2 === 0 ? "cancellations" : "amendments",
      authorLabel: "",
      text: `line ${i}`,
      outbound: i % 2 === 0,
    })),
  });

  it("takes the host's updated fold over the stale one already on screen", () => {
    const midExchange = row("h4", { referralConversation: crossing(1) });
    const finished = row("h4", { referralConversation: crossing(4) });

    const merged = mergeHistoryInOrder([midExchange], [finished]);

    expect(merged).toHaveLength(1);
    expect(merged[0].referralConversation?.lines).toHaveLength(4);
  });

  it("still keeps the held object when nothing changed, so React can bail", () => {
    const held = row("h4", { referralConversation: crossing(4) });
    const same = row("h4", { referralConversation: crossing(4) });

    const merged = mergeHistoryInOrder([held], [same]);

    expect(merged[0]).toBe(held);
  });
});
