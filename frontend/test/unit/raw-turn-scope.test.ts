// What a DM's raw turns are scoped to.
//
// The rows below are a real session, not an invented one: one live run of
// "Who should own the pricing section…" in the Deck Builder's DM, which asked
// the Strategist and got a takeover back. The host answered eight rows across
// four channels, and the DM's own two spellings match two of them.
import { describe, expect, it } from "vitest";

import type { AgentSessionMessageDto } from "../../src/api/types";
import {
  dmRawTurns,
  inDmWith,
  inPairConversationWith,
} from "../../src/views/room/rawTurnScope";

const DM_EPISODE = "167878f877924dcba304f912f054dfb4";
const DESK_EPISODE = "6b6194bd5dd6488e9cca60b3bda631a5";
const PAIR = "dm:deck_builder+strategist";

function row(
  id: string,
  sessionChannelId: string,
  text: string,
  episode?: string,
): AgentSessionMessageDto {
  return {
    id,
    channel: sessionChannelId,
    sessionChannel: sessionChannelId,
    sessionChannelId,
    author: "someone",
    text,
    atMillis: Number(id),
    mine: false,
    ...(episode ? { episode: { id: episode, kind: "post" } } : {}),
  } as AgentSessionMessageDto;
}

/** The eight rows the host actually answered with, in order. */
const session: AgentSessionMessageDto[] = [
  row("1", "engagement_delivery", "Who should own the pricing section…", DESK_EPISODE),
  row("2", "engagement_delivery", "The Strategist (me) should own…", DESK_EPISODE),
  row("3", "engagement_delivery", "The **Strategist** should own…", DESK_EPISODE),
  row("4", "deck_builder", "Who should own the pricing section… end to end?"),
  row("5", PAIR, "The operator wants you to take ownership…", DM_EPISODE),
  row("6", PAIR, "Yes, I'll take it end to end.", DM_EPISODE),
  row("7", PAIR, "concluded our conversation (thread 28).", DM_EPISODE),
  row("8", "dm:deck_builder", "Strategist confirmed they're owning it", DM_EPISODE),
];

describe("a DM's raw turns", () => {
  it("keeps the conversation the DM opened, not just the DM's own channel", () => {
    const kept = dmRawTurns(session, "deck_builder").map((r) => r.id);
    // 4 and 8 are the DM's own two spellings; 5, 6 and 7 are the exchange it
    // opened. Filtering on the channel alone returned only 4 and 8 — dropping
    // the question, the takeover and the conclusion, which is the whole of
    // what the teammate did.
    expect(kept).toEqual(["4", "5", "6", "7", "8"]);
  });

  it("leaves the teammate's work on other desks out", () => {
    const kept = dmRawTurns(session, "deck_builder").map((r) => r.id);
    // Scoped to this conversation, not to the whole session. The desk rows
    // carry a different episode, so episode scoping excludes them for free.
    expect(kept).not.toContain("1");
    expect(kept).not.toContain("2");
    expect(kept).not.toContain("3");
  });

  it("will not pull the same pair's other episodes in", () => {
    // `pair_conversation` is deterministic, so these two seats reuse one
    // channel forever. A pair row from an episode this DM never opened is
    // somebody else's transcript.
    const older = row("0", PAIR, "last week's exchange", "another-episode");
    const kept = dmRawTurns([older, ...session], "deck_builder").map((r) => r.id);
    expect(kept).not.toContain("0");
  });

  it("reads the DM under both spellings the host lists", () => {
    expect(inDmWith(row("a", "deck_builder", ""), "deck_builder")).toBe(true);
    expect(inDmWith(row("b", "dm:deck_builder", ""), "deck_builder")).toBe(true);
    expect(inDmWith(row("c", PAIR, ""), "deck_builder")).toBe(false);
  });

  it("recognises a pair channel from either side", () => {
    expect(inPairConversationWith(row("a", PAIR, ""), "deck_builder")).toBe(true);
    expect(inPairConversationWith(row("b", PAIR, ""), "strategist")).toBe(true);
    expect(inPairConversationWith(row("c", PAIR, ""), "writer")).toBe(false);
    expect(inPairConversationWith(row("d", "dm:deck_builder", ""), "deck_builder")).toBe(false);
  });
});
