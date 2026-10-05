/**
 * The company hive (OC-2), as the console sees it: a bounded fold over the
 * hive frames, and the per-desk episode grouping the room draws.
 *
 * # What the frames say that the rows do not
 *
 * A desk's public lines are ordinary `agent_reply` rows carrying
 * `hive.episodeId`, so the transcript alone groups them by episode after a
 * reload. What the rows cannot say is **how an episode ended** (only
 * `hive_episode_settled` says that), **who started it** (`hive_accepted`), and
 * **who spoke to whom off the desk** (`hive_message`: a direct line between
 * two agents, or a private desk line read only by `onlyFor`). This fold keeps
 * exactly those.
 *
 * Pure, so every rule is unit-testable without a browser, and bounded: a
 * console left open on a busy company sees frames for every desk forever, so
 * {@link HIVE_EPISODE_CAP} episodes are kept (settled ones evicted first) and
 * {@link HIVE_CONTACT_CAP} contacts.
 *
 * `scripts/lib/coordination-metrics.mjs` folds the same frames over the raw
 * SSE feed; `lib/coordination.ts` turns this fold into the same numbers.
 */

import type { MessageHiveDto } from "@/api/types";
import type { HiveFrame, TurnBracketFrame } from "@/hooks/use-events";
import type { ChatMessage } from "@/lib/chat";

/** How an episode stands, as the frames have said so far. */
export type HiveEpisodeStatus = "open" | "settled" | "failed";

/** Everything the frames have said about one episode. */
export interface HiveEpisodeState {
  id: string;
  /** The desk (hive) it ran on, once a frame named it. */
  chatId?: string;
  openedAtMillis: number;
  status: HiveEpisodeStatus;
  settledAtMillis?: number;
  /** Why it failed, when `hive_episode_settled` carried a failure. */
  failure?: string;
  /** Turns the brackets said ran for it. */
  turns: number;
}

/**
 * One agent reaching others off the desk: a `hive_message`. `direct` is a
 * line to one agent's inbox; `private` a desk line only `to` may read.
 */
export interface HiveContact {
  from: string;
  /** The readers, never including the sender. Empty for a line to oneself. */
  to: string[];
  via: "direct" | "private";
  atMillis: number;
  episodeId?: string;
}

/** The fold. */
export interface HiveFrames {
  byId: Readonly<Record<string, HiveEpisodeState>>;
  /** Episode ids, oldest first. */
  order: readonly string[];
  /** Newest last, bounded by {@link HIVE_CONTACT_CAP}. */
  contacts: readonly HiveContact[];
  /** How many episodes each `hive_accepted.route` started. */
  starterRoutes: Readonly<Record<string, number>>;
  /** Turns lost to a restart (`hive_turn_interrupted`). */
  interruptedTurns: number;
}

export const EMPTY_HIVE_FRAMES: HiveFrames = Object.freeze({
  byId: Object.freeze({}),
  order: Object.freeze([]),
  contacts: Object.freeze([]),
  starterRoutes: Object.freeze({}),
  interruptedTurns: 0,
}) as HiveFrames;

/** How many episodes the fold keeps. */
export const HIVE_EPISODE_CAP = 256;
/** How many contacts the fold keeps. */
export const HIVE_CONTACT_CAP = 512;

/**
 * What the fold reads: the hive frames, the turn brackets (a bracket with
 * `hive.episodeId` counts a turn for that episode), and an `agent_reply`
 * carrying `hive.episodeId` (an episode the console first hears of through a
 * row still counts as opened).
 */
export type HiveFoldInput =
  | HiveFrame
  | TurnBracketFrame
  | { type: "agent_reply"; chatId: string; atMillis: number; hive?: MessageHiveDto };

function withEpisode(
  state: HiveFrames,
  id: string,
  chatId: string | undefined,
  atMillis: number,
  change: (episode: HiveEpisodeState) => HiveEpisodeState,
): HiveFrames {
  const held = state.byId[id];
  const base: HiveEpisodeState = held ?? { id, openedAtMillis: atMillis, status: "open", turns: 0 };
  const next = change(!base.chatId && chatId ? { ...base, chatId } : base);
  const byId = { ...state.byId, [id]: next };
  let order = held ? state.order : [...state.order, id];
  if (order.length > HIVE_EPISODE_CAP) {
    const evict = order.find((key) => byId[key]?.status !== "open") ?? order[0];
    order = order.filter((key) => key !== evict);
    delete byId[evict];
  }
  return { ...state, byId, order };
}

/** Folds one frame. Same object back for a frame that changes nothing. */
export function reduceHiveFrame(state: HiveFrames, frame: HiveFoldInput): HiveFrames {
  switch (frame.type) {
    case "turn_started": {
      const id = frame.hive?.episodeId;
      if (!id) return state;
      return withEpisode(state, id, frame.hive?.hiveId, frame.atMillis, (episode) => ({
        ...episode,
        turns: episode.turns + 1,
      }));
    }
    case "turn_settled":
      return state;
    case "agent_reply": {
      const id = frame.hive?.episodeId;
      if (!id) return state;
      if (state.byId[id]?.chatId) return state;
      return withEpisode(state, id, frame.chatId, frame.atMillis, (episode) => episode);
    }
    case "hive_message": {
      const destination = frame.destination;
      let next = state;
      if (frame.episodeId) {
        const chatId = destination?.type === "hive" ? destination.id : undefined;
        next = withEpisode(next, frame.episodeId, chatId, frame.atMillis, (episode) => episode);
      }
      const contact: HiveContact =
        destination?.type === "agent"
          ? {
              from: frame.sender,
              to: destination.id === frame.sender ? [] : [destination.id],
              via: "direct",
              atMillis: frame.atMillis,
              episodeId: frame.episodeId,
            }
          : {
              from: frame.sender,
              to: (frame.onlyFor ?? []).filter((id) => id !== frame.sender),
              via: "private",
              atMillis: frame.atMillis,
              episodeId: frame.episodeId,
            };
      return { ...next, contacts: [...next.contacts, contact].slice(-HIVE_CONTACT_CAP) };
    }
    case "hive_accepted": {
      if (!frame.route) return state;
      const route = frame.route;
      return {
        ...state,
        starterRoutes: { ...state.starterRoutes, [route]: (state.starterRoutes[route] ?? 0) + 1 },
      };
    }
    case "hive_episode_settled":
      return withEpisode(state, frame.episodeId, frame.chatId, frame.atMillis, (episode) =>
        episode.status !== "open"
          ? episode
          : {
              ...episode,
              status: frame.failure ? "failed" : "settled",
              failure: frame.failure,
              settledAtMillis: frame.atMillis,
            },
      );
    case "hive_turn_interrupted":
      return { ...state, interruptedTurns: state.interruptedTurns + 1 };
    default:
      return state;
  }
}

/** One episode a desk ran, as the room groups it. */
export interface DeskEpisode {
  id: string;
  chatId?: string;
  /** The console ids of the rows it produced on this desk, transcript order. */
  messageIds: string[];
  /** `open` until a `hive_episode_settled` frame says otherwise. */
  status: HiveEpisodeStatus;
  failure?: string;
  openedAtMillis?: number;
  settledAtMillis?: number;
}

/**
 * Groups a desk's rows by `hive.episodeId`, oldest first, with each episode's
 * status from the frames.
 *
 * `messages` is the whole transcript, thread replies included. `chatId`
 * narrows the frames to this desk; an episode the frames settled here that
 * produced no row on it (one that failed before posting) is still returned,
 * so the room can say it failed.
 *
 * After a reload without frames an episode reads `open`: the transcript alone
 * cannot say whether it ended.
 */
export function deskEpisodes(
  messages: readonly ChatMessage[],
  frames: HiveFrames = EMPTY_HIVE_FRAMES,
  chatId?: string,
): DeskEpisode[] {
  const out = new Map<string, DeskEpisode>();
  const of = (id: string): DeskEpisode => {
    let episode = out.get(id);
    if (!episode) {
      const known = frames.byId[id];
      episode = {
        id,
        chatId: known?.chatId ?? chatId,
        messageIds: [],
        status: known?.status ?? "open",
        failure: known?.failure,
        openedAtMillis: known?.openedAtMillis,
        settledAtMillis: known?.settledAtMillis,
      };
      out.set(id, episode);
    }
    return episode;
  };
  for (const message of messages) {
    const id = message.hive?.episodeId;
    if (!id) continue;
    const episode = of(id);
    episode.messageIds.push(message.id);
    if (episode.openedAtMillis === undefined || message.at < episode.openedAtMillis) {
      episode.openedAtMillis = message.at;
    }
  }
  if (chatId) {
    for (const id of frames.order) {
      const known = frames.byId[id];
      if (known?.chatId === chatId && known.status !== "open") of(id);
    }
  }
  return [...out.values()];
}
