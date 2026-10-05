import { describe, expect, it } from "vitest";

import type { ChatMessage } from "@/lib/chat";
import {
  deskEpisodes,
  EMPTY_HIVE_FRAMES,
  HIVE_CONTACT_CAP,
  reduceHiveFrame,
  type HiveFoldInput,
} from "@/lib/hive";
import { buildTimelineItems, type TimelineEntry } from "@/views/room/model";

/**
 * `lib/hive.ts`: the fold over the company-hive frames, and the per-desk
 * grouping of rows by `hive.episodeId` the room draws.
 */

const fold = (frames: HiveFoldInput[]) =>
  frames.reduce((state, frame) => reduceHiveFrame(state, frame), EMPTY_HIVE_FRAMES);

describe("reduceHiveFrame", () => {
  it("marks an episode settled, or failed with its reason, once", () => {
    const state = fold([
      { type: "hive_episode_settled", seq: 1, atMillis: 10, episodeId: "a", chatId: "eng", openedAt: 1 },
      { type: "hive_episode_settled", seq: 2, atMillis: 20, episodeId: "b", chatId: "eng", openedAt: 2, failure: "boom" },
      { type: "hive_episode_settled", seq: 3, atMillis: 30, episodeId: "a", chatId: "eng", openedAt: 1, failure: "late" },
    ]);
    expect(state.byId.a).toMatchObject({ status: "settled", settledAtMillis: 10, chatId: "eng" });
    expect(state.byId.b).toMatchObject({ status: "failed", failure: "boom" });
    expect(state.order).toEqual(["a", "b"]);
  });

  it("returns the same object for a frame that changes nothing", () => {
    const turn = { type: "turn_started", seq: 1, atMillis: 1, agentId: "x" } as const;
    expect(reduceHiveFrame(EMPTY_HIVE_FRAMES, turn)).toBe(EMPTY_HIVE_FRAMES);
    const reply = { type: "agent_reply", chatId: "eng", atMillis: 1 } as const;
    expect(reduceHiveFrame(EMPTY_HIVE_FRAMES, reply)).toBe(EMPTY_HIVE_FRAMES);
  });

  it("counts a turn for its episode and remembers the hive it ran in", () => {
    const state = fold([
      { type: "turn_started", seq: 1, atMillis: 1, agentId: "x", hive: { hiveId: "eng", episodeId: "e" } },
      { type: "turn_started", seq: 2, atMillis: 2, agentId: "y", hive: { episodeId: "e" } },
    ]);
    expect(state.byId.e).toMatchObject({ turns: 2, chatId: "eng", status: "open" });
  });

  it("keeps a bounded tail of contacts", () => {
    const frames: HiveFoldInput[] = Array.from({ length: HIVE_CONTACT_CAP + 5 }, (_, i) => ({
      type: "hive_message",
      seq: i,
      atMillis: i,
      sequence: i,
      sender: "a",
      destination: { type: "agent", id: "b" },
      text: "hi",
    }));
    const state = fold(frames);
    expect(state.contacts).toHaveLength(HIVE_CONTACT_CAP);
    expect(state.contacts.at(-1)?.atMillis).toBe(HIVE_CONTACT_CAP + 4);
  });
});

const ROWS: ChatMessage[] = [
  { id: "h1", from: "you", at: 1, text: "Question?" },
  { id: "h2", from: "company", channel: "engineer", at: 2, text: "One", hive: { sequence: 2, episodeId: "ep" } },
  { id: "h3", from: "company", channel: "ceo", at: 3, text: "Two", hive: { sequence: 3, episodeId: "ep" } },
  { id: "h4", from: "company", channel: "ceo", at: 4, text: "Outside" },
];

describe("deskEpisodes", () => {
  it("groups a desk's rows by hive.episodeId, open until a settle frame says otherwise", () => {
    expect(deskEpisodes(ROWS, EMPTY_HIVE_FRAMES, "eng")).toEqual([
      { id: "ep", chatId: "eng", messageIds: ["h2", "h3"], status: "open", openedAtMillis: 2 },
    ]);
    const settled = fold([
      { type: "hive_episode_settled", seq: 9, atMillis: 9, episodeId: "ep", chatId: "eng", openedAt: 1 },
    ]);
    expect(deskEpisodes(ROWS, settled, "eng")[0]).toMatchObject({ status: "settled", settledAtMillis: 9 });
  });

  it("keeps an episode that settled on this desk without a row, and ignores another desk's", () => {
    const frames = fold([
      { type: "hive_episode_settled", seq: 1, atMillis: 5, episodeId: "quiet", chatId: "eng", openedAt: 1, failure: "x" },
      { type: "hive_episode_settled", seq: 2, atMillis: 6, episodeId: "other", chatId: "content", openedAt: 1 },
    ]);
    expect(deskEpisodes([], frames, "eng").map((e) => e.id)).toEqual(["quiet"]);
  });
});

describe("buildTimelineItems with hive episodes", () => {
  const entries: TimelineEntry[] = ROWS.map((message) => ({
    message,
    sender: { key: message.channel ?? "you", name: message.channel ?? "You", kind: "agent" },
    continuation: false,
    replies: [],
    replySenders: [],
  }));

  it("collapses an episode's rows into one item and adds the settle marker after it", () => {
    const frames = fold([
      { type: "hive_episode_settled", seq: 9, atMillis: 3, episodeId: "ep", chatId: "eng", openedAt: 1 },
    ]);
    const items = buildTimelineItems(entries, [], {}, deskEpisodes(ROWS, frames, "eng"));
    expect(items.map((item) => item.kind)).toEqual(["message", "episode", "episode_complete", "message"]);
    const group = items[1];
    expect(group.kind === "episode" && group.items.map((row) => row.key)).toEqual(["h2", "h3"]);
  });

  it("draws no marker for an open episode", () => {
    const items = buildTimelineItems(entries, [], {}, deskEpisodes(ROWS, EMPTY_HIVE_FRAMES, "eng"));
    expect(items.map((item) => item.kind)).toEqual(["message", "episode", "message"]);
  });
});
