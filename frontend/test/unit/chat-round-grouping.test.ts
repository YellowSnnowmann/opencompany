import { describe, expect, it } from "vitest";

import type { EpisodeFrame } from "@/hooks/use-events";
import type { ChatMessage } from "@/lib/chat";
import { EMPTY_EPISODE_FRAMES, reduceEpisodeFrame } from "@/lib/episode-frames";
import { foldEpisodes } from "@/lib/episodes";
import { buildTimeline, buildTimelineItems, type Channel } from "@/views/room/model";

/**
 * `buildTimelineItems` with episodes: a round's rows collapse into one `round`
 * item at the position of the first row, a round the frames opened before any
 * row landed takes the moment it opened, a completed episode gets its marker
 * after its last round, and everything outside an episode is untouched.
 */

const CHANNEL: Channel = { id: "engineering", name: "engineering", voice: "Engineering desk", kind: "channel", purpose: "" };

const ROWS: ChatMessage[] = [
  { id: "h1", from: "you", byPerson: true, at: 0, text: "Ship it?" },
  { id: "h2", from: "company", channel: "engineer", at: 10, text: "Staging first.", episode: { id: "ep-1", revision: 0, kind: "post" } },
  { id: "h3", from: "company", channel: "ceo", at: 11, text: "How long?", episode: { id: "ep-1", revision: 0, kind: "post" } },
  { id: "h4", from: "company", channel: "engineer", at: 20, text: "Two days.", episode: { id: "ep-1", revision: 1, kind: "broadcast" } },
  { id: "h5", from: "company", channel: "ceo", at: 30, text: "Decision: staging.", episode: { id: "ep-1", revision: 2, kind: "complete_episode" } },
  { id: "h6", from: "you", byPerson: true, at: 40, text: "thanks" },
];

const kinds = (rows: ChatMessage[], ...rest: Parameters<typeof foldEpisodes>) =>
  buildTimelineItems(buildTimeline(rows, CHANNEL, []), [], {}, foldEpisodes(rows, ...rest.slice(1) as [never, never])).map((item) =>
    item.kind === "round"
      ? `round:${item.round.revision}[${item.items.map((r) => r.key).join(",")}]`
      : item.kind === "episode_complete"
        ? `complete:${item.episode.id}`
        : `${item.kind}:${item.key}`,
  );

describe("round grouping", () => {
  it("leaves a transcript with no episodes exactly as it was", () => {
    const plain = ROWS.filter((row) => !row.episode);
    const items = buildTimelineItems(buildTimeline(plain, CHANNEL, []), [], {}, foldEpisodes(plain));
    expect(items.map((item) => item.kind)).toEqual(["message", "message"]);
  });

  it("collapses each round's rows into one item, in transcript order, then the marker", () => {
    expect(kinds(ROWS, ROWS)).toEqual([
      "message:h1",
      "round:0[h2,h3]",
      "round:1[h4]",
      "round:2[h5]",
      "complete:ep-1",
      "message:h6",
    ]);
  });

  it("places a live round with no rows after the previous round's replies", () => {
    const frames = (
      [
        { type: "episode_opened", seq: 1, atMillis: 5, chatId: "engineering", episodeId: "ep-1", openedBySeq: 1, participants: ["engineer", "ceo"], plan: { kind: "hive", primaryId: "engineer", invitedIds: ["ceo"] } },
        { type: "round_started", seq: 2, atMillis: 8, chatId: "engineering", episodeId: "ep-1", revision: 0, agentIds: ["engineer", "ceo"] },
        { type: "round_committed", seq: 5, atMillis: 12, chatId: "engineering", episodeId: "ep-1", revision: 0, utterances: [{ agentId: "engineer", sequence: 2, kind: "post", messageSeq: 2 }, { agentId: "ceo", sequence: 3, kind: "post", messageSeq: 3 }] },
        { type: "round_started", seq: 6, atMillis: 15, chatId: "engineering", episodeId: "ep-1", revision: 1, agentIds: ["engineer"] },
      ] as EpisodeFrame[]
    ).reduce(reduceEpisodeFrame, EMPTY_EPISODE_FRAMES);
    const rows = ROWS.slice(0, 3);
    const items = buildTimelineItems(buildTimeline(rows, CHANNEL, []), [], {}, foldEpisodes(rows, frames, "engineering"));
    expect(items.map((item) => item.kind)).toEqual(["message", "round", "round"]);
    const live = items[2];
    expect(live.kind === "round" && live.round.status).toBe("open");
    expect(live.kind === "round" && live.items).toEqual([]);
    expect(live.at).toBe(15);
    // No marker: the episode is still open.
    expect(items.some((item) => item.kind === "episode_complete")).toBe(false);
  });

  it("keeps an approval raised mid-episode in the channel at its own time", () => {
    const approvals = [
      { id: "a1", kind: "shell.run", at_millis: 15, agent: "engineer", thread: "engineering" },
    ] as never[];
    const items = buildTimelineItems(buildTimeline(ROWS, CHANNEL, []), approvals, {}, foldEpisodes(ROWS));
    const kindsOut = items.map((item) => item.kind);
    expect(kindsOut).toEqual(["message", "round", "approval", "round", "round", "episode_complete", "message"]);
  });

  it("puts the completion marker after the last round even when the frames say when", () => {
    const frames = (
      [
        { type: "episode_completed", seq: 9, atMillis: 31, chatId: "engineering", episodeId: "ep-1", revision: 3, completedBy: "ceo", rounds: 3, reason: "complete_episode" },
      ] as EpisodeFrame[]
    ).reduce(reduceEpisodeFrame, EMPTY_EPISODE_FRAMES);
    const items = buildTimelineItems(buildTimeline(ROWS, CHANNEL, []), [], {}, foldEpisodes(ROWS, frames, "engineering"));
    const marker = items.find((item) => item.kind === "episode_complete");
    expect(marker?.at).toBe(31);
    expect(items.indexOf(marker!)).toBe(items.length - 2);
  });
});
