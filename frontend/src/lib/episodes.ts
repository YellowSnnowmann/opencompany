/**
 * Episodes and rounds, folded out of a desk's transcript and its live frames.
 *
 * # Two sources, one shape
 *
 * A committed utterance is an ordinary `agent_reply` row that carries
 * `episode: {id, revision, kind, to?}` — so after a reload the transcript alone
 * says which rows belong to which round, and in what order. That is the durable
 * half, and it is enough to draw every band a finished episode ever had.
 *
 * What the rows cannot say is the present tense: the seats a round opened with
 * before any of them spoke, which seat is still working, which one timed out.
 * That is the live half, held by `lib/episode-frames.ts`. This fold joins the
 * two: rows first, frames layered on. A row is never contradicted by a frame —
 * a seat with a row is committed whatever its last bracket said — and a frame
 * is never invented from a row.
 *
 * # Why it is pure
 *
 * Same reason `views/room/timeline.ts` is: every interesting case — a seat with
 * no row yet, an episode seen only in frames, a completed episode seen only in
 * rows — is a function of two lists, and a test can state each one in a dozen
 * lines with no browser and no host.
 */

import type { RoutingPlanDto, UtteranceKind } from "@/api/types";
import { hostMessageId, type ChatMessage } from "@/lib/chat";
import {
  episodesOf,
  type ConversationRecord,
  type EpisodeFrames,
  type EpisodeReferral,
  type EpisodeState,
  type SeatStatus,
} from "@/lib/episode-frames";

export type { ConversationRecord, SeatStatus } from "@/lib/episode-frames";

/** One seat of a round: who, what they are doing, and what they said. */
export interface EpisodeSeat {
  agentId: string;
  status: SeatStatus;
  /** The console id of the row this seat's utterance became, once it did. */
  messageId?: string;
  utterance?: { kind: UtteranceKind; to?: string[] };
  startedAt?: number;
  settledAt?: number;
}

/** One round: the seats that ran together and the rows they produced. */
export interface EpisodeRound {
  episodeId: string;
  revision: number;
  status: "open" | "committed";
  /** Lane order: the host's `agentIds`, then any seat only a row named. */
  seats: EpisodeSeat[];
  /** Console ids of the rows committed in this round, transcript order. */
  messageIds: string[];
  /** When the round opened, from the frame or the first row. */
  startedAt?: number;
  committedAt?: number;
}

/** One episode a desk ran, or is running. */
export interface Episode {
  id: string;
  chatId?: string;
  /** The console id of the message that opened it, when the frames said. */
  rootMessageId?: string;
  parentId?: string;
  participants: string[];
  plan?: RoutingPlanDto;
  status: "open" | "completed";
  rounds: EpisodeRound[];
  /** Every row the episode produced, across its rounds. */
  messageIds: string[];
  openedAt?: number;
  completedAt?: number;
  completedBy?: string;
  reason?: string;
  /** Rounds the host counted, else the rounds seen here. */
  roundCount: number;
  referrals: EpisodeReferral[];
  /** The private exchanges this episode opened, oldest first.
   *
   * Only the frames carry these: a conversation's own rows are in the pair
   * channel, so a fold over this desk's transcript alone never sees them. */
  conversations: ConversationRecord[];
  /** Whether any part of it came from live frames rather than rows alone. */
  live: boolean;
}

interface Bucket {
  episode: Episode;
  rounds: Map<number, EpisodeRound>;
  seatByRound: Map<number, Map<string, EpisodeSeat>>;
}

/**
 * Folds a desk's rows and the live frames into episodes, oldest first.
 *
 * `messages` is the whole transcript, thread replies included — a seat's row
 * can be a reply to the operator message that opened the episode, and
 * `buildTimeline` would have folded it out. `chatId` narrows the frames to
 * this desk; the rows are already the desk's own.
 */
export function foldEpisodes(
  messages: ChatMessage[],
  frames?: EpisodeFrames,
  chatId?: string,
): Episode[] {
  const buckets = new Map<string, Bucket>();
  const bucketFor = (id: string): Bucket => {
    let bucket = buckets.get(id);
    if (!bucket) {
      bucket = {
        episode: {
          id,
          chatId,
          participants: [],
          status: "open",
          rounds: [],
          messageIds: [],
          roundCount: 0,
          referrals: [],
          conversations: [],
          live: false,
        },
        rounds: new Map(),
        seatByRound: new Map(),
      };
      buckets.set(id, bucket);
    }
    return bucket;
  };
  const roundFor = (bucket: Bucket, revision: number): EpisodeRound => {
    let round = bucket.rounds.get(revision);
    if (!round) {
      round = {
        episodeId: bucket.episode.id,
        revision,
        status: "open",
        seats: [],
        messageIds: [],
      };
      bucket.rounds.set(revision, round);
      bucket.seatByRound.set(revision, new Map());
    }
    return round;
  };
  const seatFor = (bucket: Bucket, revision: number, agentId: string): EpisodeSeat => {
    const round = roundFor(bucket, revision);
    const seats = bucket.seatByRound.get(revision)!;
    let seat = seats.get(agentId);
    if (!seat) {
      seat = { agentId, status: "waiting" };
      seats.set(agentId, seat);
      round.seats.push(seat);
    }
    return seat;
  };

  // The rows: every committed utterance, in transcript order.
  for (const message of messages) {
    const meta = message.episode;
    if (!meta || message.from !== "company" || !message.channel) continue;
    const bucket = bucketFor(meta.id);
    const round = roundFor(bucket, meta.revision);
    // A row is a commit: the driver only journals an utterance it accepted.
    round.status = "committed";
    round.messageIds.push(message.id);
    round.startedAt = Math.min(round.startedAt ?? message.at, message.at);
    round.committedAt = Math.max(round.committedAt ?? message.at, message.at);
    const seat = seatFor(bucket, meta.revision, message.channel);
    seat.status = "committed";
    seat.messageId = message.id;
    seat.utterance = { kind: meta.kind, to: meta.to };
    bucket.episode.messageIds.push(message.id);
    if (!bucket.episode.participants.includes(message.channel)) {
      bucket.episode.participants.push(message.channel);
    }
    if (meta.kind === "complete_episode") {
      bucket.episode.status = "completed";
      bucket.episode.completedBy = message.channel;
      bucket.episode.completedAt = message.at;
      bucket.episode.reason = bucket.episode.reason ?? "complete_episode";
    }
  }

  // The frames: what is open, who is working, what the host decided.
  const live = frames ? (chatId ? episodesOf(frames, chatId) : allEpisodes(frames)) : [];
  for (const state of live) layerFrames(bucketFor(state.id), state);

  const out = [...buckets.values()].map(({ episode, rounds }) => {
    episode.rounds = [...rounds.values()].sort((a, b) => a.revision - b.revision);
    episode.roundCount = Math.max(episode.roundCount, episode.rounds.length);
    if (episode.openedAt === undefined) episode.openedAt = episode.rounds[0]?.startedAt;
    return episode;
  });
  return out.sort((a, b) => (a.openedAt ?? 0) - (b.openedAt ?? 0));
}

function allEpisodes(frames: EpisodeFrames): EpisodeState[] {
  return frames.order
    .map((id) => frames.byId[id])
    .filter((episode): episode is EpisodeState => !!episode);
}

/** Layers one episode's live state onto its bucket. Rows win where they exist. */
function layerFrames(bucket: Bucket, state: EpisodeState): void {
  const { episode } = bucket;
  episode.live = true;
  episode.chatId = state.chatId || episode.chatId;
  if (state.openedBySeq !== undefined) episode.rootMessageId = hostMessageId(String(state.openedBySeq));
  episode.parentId = state.parentId;
  episode.plan = state.plan;
  for (const id of state.participants) {
    if (!episode.participants.includes(id)) episode.participants.push(id);
  }
  episode.openedAt = state.openedAtMillis ?? episode.openedAt;
  episode.referrals = state.referrals;
  episode.conversations = Object.values(state.conversations).sort(
    (one, two) => one.root - two.root,
  );
  if (state.status === "completed") {
    episode.status = "completed";
    episode.completedAt = state.completedAtMillis ?? episode.completedAt;
    episode.completedBy = state.completedBy ?? episode.completedBy;
    episode.reason = state.reason ?? episode.reason;
    episode.roundCount = Math.max(episode.roundCount, state.roundCount ?? 0);
  }

  for (const liveRound of Object.values(state.rounds)) {
    const round = bucket.rounds.get(liveRound.revision);
    const held = round?.seats ?? [];
    const fresh: EpisodeRound =
      round ??
      ({
        episodeId: episode.id,
        revision: liveRound.revision,
        status: "open",
        seats: [],
        messageIds: [],
      } satisfies EpisodeRound);
    if (!round) {
      bucket.rounds.set(liveRound.revision, fresh);
      bucket.seatByRound.set(liveRound.revision, new Map());
    }
    fresh.startedAt = Math.min(fresh.startedAt ?? liveRound.startedAtMillis, liveRound.startedAtMillis);
    // The frame's word on the round wins over the rows': a seat's reply is
    // journaled as it lands, before the driver commits the round, so a round
    // with one row in the transcript is still open until `round_committed`.
    fresh.status = liveRound.status;
    if (liveRound.status === "committed") {
      fresh.committedAt = fresh.committedAt ?? liveRound.committedAtMillis;
    }
    // Lane order is the host's: the seats named by `round_started`, in that
    // order, followed by any seat only a row named.
    const seats = bucket.seatByRound.get(liveRound.revision)!;
    const ordered: EpisodeSeat[] = [];
    for (const agentId of liveRound.agentIds) {
      const liveSeat = liveRound.seats[agentId];
      let seat = seats.get(agentId);
      if (!seat) {
        seat = { agentId, status: "waiting" };
        seats.set(agentId, seat);
      }
      // A row already proved the commit; the bracket cannot demote it.
      if (seat.status !== "committed") seat.status = liveSeat?.status ?? seat.status;
      seat.startedAt = liveSeat?.startedAtMillis;
      seat.settledAt = liveSeat?.settledAtMillis;
      if (!seat.utterance && liveSeat?.utterance) {
        seat.utterance = { kind: liveSeat.utterance.kind, to: liveSeat.utterance.to };
        if (liveSeat.utterance.messageSeq !== undefined) {
          seat.messageId = hostMessageId(String(liveSeat.utterance.messageSeq));
        }
      }
      ordered.push(seat);
    }
    for (const seat of held) if (!ordered.includes(seat)) ordered.push(seat);
    fresh.seats = ordered;
  }
}

/** The round a row belongs to, or `undefined` for a row outside every episode. */
export function roundOf(
  episodes: Episode[],
  messageId: string,
): { episode: Episode; round: EpisodeRound } | undefined {
  for (const episode of episodes) {
    for (const round of episode.rounds) {
      if (round.messageIds.includes(messageId)) return { episode, round };
    }
  }
  return undefined;
}

/** Whether any round of any episode is still open — the desk is answering. */
export function anyOpen(episodes: Episode[]): boolean {
  return episodes.some((episode) => episode.status === "open");
}

/** The seats working right now, across every open round, for a working row. */
export function workingSeats(episodes: Episode[]): string[] {
  const out: string[] = [];
  for (const episode of episodes) {
    if (episode.status !== "open") continue;
    for (const round of episode.rounds) {
      if (round.status !== "open") continue;
      for (const seat of round.seats) {
        if (seat.status === "working" && !out.includes(seat.agentId)) out.push(seat.agentId);
      }
    }
  }
  return out;
}
