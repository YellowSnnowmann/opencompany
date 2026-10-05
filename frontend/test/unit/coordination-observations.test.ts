import { describe, expect, it } from "vitest";

import type { HiveFrame, TurnBracketFrame } from "@/hooks/use-events";
import {
  coordinationObservations,
  coordinationSummary,
  EMPTY_TURN_LEDGER,
  reduceTurnBracket,
  workingAgents,
} from "@/lib/coordination";
import { EMPTY_HIVE_FRAMES, reduceHiveFrame, type HiveFoldInput } from "@/lib/hive";

/**
 * `lib/coordination.ts`: the turn ledger's peak and overlap counts, and the
 * comms edges the hive fold implies. These are the console's copy of the
 * numbers `scripts/lib/coordination-metrics.mjs` prints — the last test feeds
 * both the same frames and pins that they agree.
 */

const bracket = (
  type: "turn_started" | "turn_settled",
  agentId: string,
  seq: number,
  extra: Partial<TurnBracketFrame> = {},
): TurnBracketFrame => ({ type, seq, atMillis: seq * 10, chatId: "engineering", agentId, ...extra });

describe("reduceTurnBracket", () => {
  it("counts the peak of open turns and closes them by turn id", () => {
    const ledger = [
      bracket("turn_started", "engineer", 1, { turnId: "t1" }),
      bracket("turn_started", "writer", 2, { turnId: "t2" }),
      bracket("turn_settled", "engineer", 3, { turnId: "t1" }),
      bracket("turn_started", "ceo", 4, { turnId: "t3" }),
      bracket("turn_settled", "writer", 5, { turnId: "t2" }),
      bracket("turn_settled", "ceo", 6, { turnId: "t3" }),
    ].reduce(reduceTurnBracket, EMPTY_TURN_LEDGER);
    expect(ledger.peak).toBe(2);
    expect(ledger.overlaps).toBe(2);
    expect(ledger.open).toEqual([]);
    expect(ledger.closed.map((t) => `${t.agentId}:${t.startedAtMillis}-${t.settledAtMillis}`)).toEqual([
      "engineer:10-30",
      "writer:20-50",
      "ceo:40-60",
    ]);
    expect(ledger.sameAgentOverlaps).toBe(0);
  });

  it("falls back to the agent when a settle carries no turn id, and ignores a stray settle", () => {
    const ledger = [
      bracket("turn_started", "engineer", 1),
      bracket("turn_settled", "engineer", 2),
    ].reduce(reduceTurnBracket, EMPTY_TURN_LEDGER);
    expect(ledger.open).toEqual([]);
    expect(reduceTurnBracket(ledger, bracket("turn_settled", "ceo", 3))).toBe(ledger);
  });

  it("keeps the thread and the hive ref a bracket named on the open turn", () => {
    const ledger = reduceTurnBracket(
      EMPTY_TURN_LEDGER,
      bracket("turn_started", "engineer", 1, {
        turnId: "t1",
        chatId: "dm-thread",
        hive: { hiveId: "engineering", episodeId: "ep-1" },
      }),
    );
    expect(ledger.open[0]).toMatchObject({ chatId: "dm-thread", hiveId: "engineering", episodeId: "ep-1" });
  });

  it("counts a second turn opening on an agent whose first is still open", () => {
    const ledger = [
      bracket("turn_started", "ceo", 1, { turnId: "a" }),
      bracket("turn_started", "ceo", 2, { turnId: "b" }),
    ].reduce(reduceTurnBracket, EMPTY_TURN_LEDGER);
    expect(ledger.sameAgentOverlaps).toBe(1);
    expect(workingAgents(ledger)).toEqual(["ceo"]);
  });
});

/** One small company run: an accepted line, two direct lines, a private one, two settles. */
const RUN: (HiveFrame | TurnBracketFrame)[] = [
  { type: "hive_accepted", seq: 1, atMillis: 10, messageId: "m1", sequence: 1, chatId: "engineering", starters: ["engineer"], route: "jev" },
  { type: "turn_started", seq: 2, atMillis: 20, turnId: "a", agentId: "engineer", hive: { hiveId: "engineering", episodeId: "ep-1" } },
  {
    type: "hive_message",
    seq: 3,
    atMillis: 30,
    sequence: 2,
    sender: "engineer",
    destination: { type: "agent", id: "ceo" },
    text: "Can you check the budget?",
    episodeId: "ep-1",
  },
  { type: "turn_started", seq: 4, atMillis: 40, turnId: "b", agentId: "ceo" },
  {
    type: "hive_message",
    seq: 5,
    atMillis: 50,
    sequence: 3,
    sender: "ceo",
    destination: { type: "agent", id: "engineer" },
    text: "Budget is fine.",
  },
  { type: "turn_settled", seq: 6, atMillis: 60, turnId: "b", agentId: "ceo", outcome: "committed" },
  {
    type: "hive_message",
    seq: 7,
    atMillis: 70,
    sequence: 4,
    sender: "engineer",
    destination: { type: "hive", id: "engineering" },
    text: "Private note for the writer.",
    episodeId: "ep-1",
    onlyFor: ["writer", "engineer"],
  },
  { type: "turn_settled", seq: 8, atMillis: 80, turnId: "a", agentId: "engineer", outcome: "committed" },
  { type: "hive_episode_settled", seq: 9, atMillis: 90, episodeId: "ep-1", chatId: "engineering", openedAt: 10 },
  { type: "hive_episode_settled", seq: 10, atMillis: 100, episodeId: "ep-2", chatId: "content", openedAt: 95, failure: "turn failed" },
  { type: "hive_turn_interrupted", seq: 11, atMillis: 110, agentId: "writer", reason: "restart" },
];

const frames = RUN.reduce((state, frame) => reduceHiveFrame(state, frame as HiveFoldInput), EMPTY_HIVE_FRAMES);
const ledger = RUN.filter(
  (frame): frame is TurnBracketFrame => frame.type === "turn_started" || frame.type === "turn_settled",
).reduce(reduceTurnBracket, EMPTY_TURN_LEDGER);

describe("coordinationObservations", () => {
  it("draws a direct line to its recipient and a private line to each reader but the sender", () => {
    expect(coordinationObservations(frames)).toEqual([
      { kind: "spoke", from: "engineer", to: "ceo", via: "direct", atMillis: 30 },
      { kind: "spoke", from: "ceo", to: "engineer", via: "direct", atMillis: 50 },
      { kind: "spoke", from: "engineer", to: "writer", via: "private", atMillis: 70 },
    ]);
  });

  it("adds a speaking observation per agent with an open turn", () => {
    const open = reduceTurnBracket(EMPTY_TURN_LEDGER, bracket("turn_started", "writer", 9));
    expect(coordinationObservations(frames, open).at(-1)).toEqual({ kind: "speaking", agentId: "writer" });
  });
});

describe("coordinationSummary", () => {
  it("summarises the way the measurement script does", () => {
    expect(coordinationSummary(frames, ledger)).toEqual({
      maxConcurrentTurns: 2,
      overlaps: 1,
      openTurns: 0,
      sameAgentOverlaps: 0,
      interruptedTurns: 1,
      episodesOpened: 2,
      episodesSettled: 1,
      episodesFailed: 1,
      episodesOpen: [],
      turnsPerEpisode: { "ep-1": 1, "ep-2": 0 },
      directMessages: 2,
      privateLines: 1,
      distinctPairs: ["ceo→engineer", "engineer→ceo", "engineer→writer"],
      starterRoutes: { jev: 1 },
    });
  });

  it("agrees with scripts/lib/coordination-metrics.mjs on the same frames", async () => {
    // A variable specifier keeps TypeScript from resolving the untyped script.
    const path = "../../../scripts/lib/coordination-metrics.mjs";
    const metrics = await import(/* @vite-ignore */ path);
    const scriptLedger = RUN.reduce((acc, frame) => metrics.foldFrame(acc, frame), metrics.createLedger());
    const script = metrics.summarize(scriptLedger);
    const console_ = coordinationSummary(frames, ledger);
    expect(console_.maxConcurrentTurns).toBe(script.maxConcurrentTurns);
    expect(console_.overlaps).toBe(script.overlaps);
    expect(console_.openTurns).toBe(script.openTurns);
    expect(console_.sameAgentOverlaps).toBe(script.sameAgentOverlaps);
    expect(console_.interruptedTurns).toBe(script.interruptedTurns);
    expect(console_.episodesOpened).toBe(script.episodesOpened);
    expect(console_.episodesSettled).toBe(script.episodesSettled);
    expect(console_.episodesFailed).toBe(script.episodesFailed);
    expect(console_.episodesOpen).toEqual(script.episodesOpen);
    expect(console_.turnsPerEpisode).toEqual(script.turnsPerEpisode);
    expect(console_.directMessages).toBe(script.directMessages);
    expect(console_.privateLines).toBe(script.privateLines);
    expect(console_.distinctPairs).toEqual(script.distinctPairs);
    expect(console_.starterRoutes).toEqual(script.starterRoutes);
  });
});
