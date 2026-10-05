// What a DM's raw turns are scoped to: the DM's own channel, under both
// spellings the host lists, and nothing from the teammate's other channels.
import { describe, expect, it } from "vitest";

import type { AgentSessionMessageDto } from "../../src/api/types";
import { dmRawTurns, inDmWith } from "../../src/views/room/rawTurnScope";

function row(id: string, sessionChannelId: string, text: string): AgentSessionMessageDto {
  return {
    id,
    channel: sessionChannelId,
    sessionChannel: sessionChannelId,
    sessionChannelId,
    author: "someone",
    text,
    atMillis: Number(id),
    mine: false,
  } as AgentSessionMessageDto;
}

const session: AgentSessionMessageDto[] = [
  row("1", "engagement_delivery", "Who should own the pricing section…"),
  row("2", "engagement_delivery", "The Strategist should own…"),
  row("3", "deck_builder", "Who should own the pricing section… end to end?"),
  row("4", "dm:deck_builder", "Strategist confirmed they're owning it"),
];

describe("a DM's raw turns", () => {
  it("keeps the DM's own channel under both spellings", () => {
    expect(dmRawTurns(session, "deck_builder").map((r) => r.id)).toEqual(["3", "4"]);
  });

  it("leaves the teammate's work on other desks out", () => {
    const kept = dmRawTurns(session, "deck_builder").map((r) => r.id);
    expect(kept).not.toContain("1");
    expect(kept).not.toContain("2");
  });

  it("reads the DM under both spellings the host lists", () => {
    expect(inDmWith(row("a", "deck_builder", ""), "deck_builder")).toBe(true);
    expect(inDmWith(row("b", "dm:deck_builder", ""), "deck_builder")).toBe(true);
    expect(inDmWith(row("c", "dm:deck_builder+strategist", ""), "deck_builder")).toBe(false);
  });
});
