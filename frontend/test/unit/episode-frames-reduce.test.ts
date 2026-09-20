import { describe, expect, it } from "vitest";

import type { EpisodeFrame, TurnBracketFrame } from "@/hooks/use-events";
import {
  allRounds,
  EMPTY_EPISODE_FRAMES,
  EPISODE_FRAME_CAP,
  episodesOf,
  reduceEpisodeFrame,
  type EpisodeFrames,
} from "@/lib/episode-frames";

/**
 * The bounded fold over the episode frames (`lib/episode-frames.ts`).
 *
 * Every rule here is one the room band depends on and a browser could only
 * report as "the lane looked wrong": a seat's bracket arriving before its
 * round's frame, a commit overriding a lost settle, a bracket with no episode
 * behind it, and the cap.
 */

const opened = (episodeId: string, seq = 1, chatId = "engineering"): EpisodeFrame => ({
  type: "episode_opened",
  seq,
  atMillis: seq * 10,
  chatId,
  episodeId,
  openedBySeq: seq - 1,
  participants: ["engineer", "ceo"],
  plan: { kind: "hive", primaryId: "engineer", invitedIds: ["ceo"] },
});
const started = (episodeId: string, revision: number, seq: number, agentIds = ["engineer", "ceo"]): EpisodeFrame => ({
  type: "round_started",
  seq,
  atMillis: seq * 10,
  chatId: "engineering",
  episodeId,
  revision,
  agentIds,
});
const turn = (
  type: "turn_started" | "turn_settled",
  agentId: string,
  seq: number,
  extra: Partial<TurnBracketFrame> = {},
): TurnBracketFrame => ({
  type,
  seq,
  atMillis: seq * 10,
  chatId: "engineering",
  agentId,
  episodeId: "ep-1",
  roundRevision: 0,
  ...extra,
});

function fold(frames: (EpisodeFrame | TurnBracketFrame)[], from: EpisodeFrames = EMPTY_EPISODE_FRAMES) {
  return frames.reduce(reduceEpisodeFrame, from);
}

describe("reduceEpisodeFrame", () => {
  it("returns the same object for a frame naming no episode", () => {
    const state = fold([opened("ep-1")]);
    const bracket = turn("turn_started", "engineer", 5, { episodeId: undefined, roundRevision: undefined });
    expect(reduceEpisodeFrame(state, bracket)).toBe(state);
    expect(reduceEpisodeFrame(EMPTY_EPISODE_FRAMES, bracket)).toBe(EMPTY_EPISODE_FRAMES);
  });

  it("opens an episode with its plan and seats, then a round with waiting lanes", () => {
    const state = fold([opened("ep-1"), started("ep-1", 0, 2)]);
    const episode = state.byId["ep-1"];
    expect(episode.plan).toEqual({ kind: "hive", primaryId: "engineer", invitedIds: ["ceo"] });
    expect(episode.participants).toEqual(["engineer", "ceo"]);
    expect(episode.status).toBe("open");
    const round = episode.rounds[0];
    expect(round.status).toBe("open");
    expect(round.agentIds).toEqual(["engineer", "ceo"]);
    expect(round.seats.engineer.status).toBe("waiting");
    expect(round.seats.ceo.status).toBe("waiting");
  });

  it("marks a seat working on its bracket and settles it by outcome", () => {
    const state = fold([
      opened("ep-1"),
      started("ep-1", 0, 2),
      turn("turn_started", "engineer", 3),
      turn("turn_started", "ceo", 4),
      turn("turn_settled", "ceo", 5, { outcome: "timed_out" }),
    ]);
    const { seats } = state.byId["ep-1"].rounds[0];
    expect(seats.engineer.status).toBe("working");
    expect(seats.engineer.startedAtMillis).toBe(30);
    expect(seats.ceo.status).toBe("timed_out");
    expect(seats.ceo.settledAtMillis).toBe(50);
  });

  it("mints the round when a seat's bracket lands before the round frame", () => {
    // The host emits the seat's bracket and the round's frame from different
    // tasks; the working state must survive the round frame landing after.
    const state = fold([opened("ep-1"), turn("turn_started", "engineer", 3), started("ep-1", 0, 4)]);
    const round = state.byId["ep-1"].rounds[0];
    expect(round.agentIds).toEqual(["engineer", "ceo"]);
    expect(round.seats.engineer.status).toBe("working");
    expect(round.seats.ceo.status).toBe("waiting");
    expect(round.startedAtMillis).toBe(30);
  });

  it("commits every seat the round names, and closes the ones it does not", () => {
    const state = fold([
      opened("ep-1"),
      started("ep-1", 0, 2),
      turn("turn_started", "engineer", 3),
      turn("turn_started", "ceo", 4),
      {
        type: "round_committed",
        seq: 6,
        atMillis: 60,
        chatId: "engineering",
        episodeId: "ep-1",
        revision: 0,
        utterances: [{ agentId: "engineer", sequence: 7, kind: "post", messageSeq: 7 }],
      },
      // A settle that lands after the commit must not demote the seat.
      turn("turn_settled", "engineer", 8, { outcome: "failed" }),
    ]);
    const round = state.byId["ep-1"].rounds[0];
    expect(round.status).toBe("committed");
    expect(round.committedAtMillis).toBe(60);
    expect(round.seats.engineer.status).toBe("committed");
    expect(round.seats.engineer.utterance).toEqual({ kind: "post", sequence: 7, messageSeq: 7, to: undefined });
    expect(round.seats.ceo.status).toBe("no_utterance");
  });

  it("records broadcasts, dms, referrals and the completion", () => {
    const state = fold([
      opened("ep-1"),
      {
        type: "broadcast_routed",
        seq: 3,
        atMillis: 30,
        chatId: "engineering",
        episodeId: "ep-1",
        revision: 1,
        agentId: "engineer",
        messageSeq: 9,
        plan: { kind: "one", primaryId: "ceo" },
        router: "jev",
      },
      { type: "dm_delivered", seq: 4, atMillis: 40, chatId: "engineering", episodeId: "ep-1", from: "ceo", to: ["engineer"], messageSeq: 10 },
      {
        type: "referral",
        seq: 5,
        atMillis: 50,
        chatId: "engineering",
        sequence: 11,
        toDesk: "content",
        target: "writer",
        asker: "engineer",
        direct: false,
        returning: false,
        episodeId: "ep-1",
        toEpisodeId: "ep-2",
      },
      { type: "episode_completed", seq: 6, atMillis: 60, chatId: "engineering", episodeId: "ep-1", revision: 2, completedBy: "ceo", rounds: 3, reason: "complete_episode", summarySeq: 12 },
    ]);
    const episode = state.byId["ep-1"];
    expect(episode.broadcasts).toHaveLength(1);
    expect(episode.broadcasts[0].router).toBe("jev");
    expect(episode.dms).toEqual([{ from: "ceo", to: ["engineer"], messageSeq: 10, atMillis: 40 }]);
    expect(episode.referrals[0].toEpisodeId).toBe("ep-2");
    expect(episode.status).toBe("completed");
    expect(episode.completedBy).toBe("ceo");
    expect(episode.roundCount).toBe(3);
    expect(episode.summarySeq).toBe(12);
  });

  it("ignores a referral that names no episode", () => {
    const state = fold([opened("ep-1")]);
    const next = reduceEpisodeFrame(state, {
      type: "referral",
      seq: 5,
      atMillis: 50,
      chatId: "engineering",
      sequence: 11,
      toDesk: "content",
      target: "writer",
      asker: "engineer",
      direct: false,
      returning: false,
    });
    expect(next).toBe(state);
  });

  it("narrows to one desk and lists every round", () => {
    const state = fold([
      opened("ep-1", 1, "engineering"),
      started("ep-1", 0, 2),
      opened("ep-2", 3, "content"),
      started("ep-2", 0, 4, ["writer", "ceo"]),
    ]);
    expect(episodesOf(state, "engineering").map((e) => e.id)).toEqual(["ep-1"]);
    expect(episodesOf(state, "content").map((e) => e.id)).toEqual(["ep-2"]);
    expect(allRounds(state).map(({ episode, round }) => `${episode.id}:${round.revision}`)).toEqual([
      "ep-1:0",
      "ep-2:0",
    ]);
  });

  it("keeps the fold under the cap, evicting completed episodes first", () => {
    let state = EMPTY_EPISODE_FRAMES;
    for (let i = 0; i < EPISODE_FRAME_CAP; i += 1) state = reduceEpisodeFrame(state, opened(`ep-${i}`, i + 1));
    // Complete the second one; it should be the first to go.
    state = reduceEpisodeFrame(state, {
      type: "episode_completed",
      seq: 500,
      atMillis: 5000,
      chatId: "engineering",
      episodeId: "ep-1",
      revision: 1,
      rounds: 1,
      reason: "complete_episode",
    });
    state = reduceEpisodeFrame(state, opened("ep-new", 600));
    expect(state.order).toHaveLength(EPISODE_FRAME_CAP);
    expect(state.byId["ep-1"]).toBeUndefined();
    expect(state.byId["ep-0"]).toBeDefined();
    expect(state.order.at(-1)).toBe("ep-new");
    // With nothing completed, the oldest goes.
    state = reduceEpisodeFrame(state, opened("ep-newer", 700));
    expect(state.byId["ep-0"]).toBeUndefined();
  });
});
