/**
 * What the company's coordination actually looked like: how many agents ran
 * at once, who spoke to whom, and how each episode ended.
 *
 * Two folds, both pure:
 *
 * - {@link reduceTurnBracket} keeps a ledger of the chat turn brackets
 *   (`turn_started` / `turn_settled`, issue #983), keyed by `turnId`, episode
 *   or not. Concurrency is a property of turns, and a direct-message turn
 *   overlapping an episode turn is still two models thinking at once.
 * - {@link coordinationObservations} turns the hive fold (`lib/hive.ts`) into
 *   the edges the comms graph draws: every `hive_message` is an agent reaching
 *   another — directly (one recipient) or privately on a desk (each `onlyFor`
 *   reader).
 *
 * `scripts/lib/coordination-metrics.mjs` computes the same numbers over the
 * raw SSE feed for a run nobody is watching; the two are kept in step by hand
 * and by the thresholds that script asserts.
 */

import type { TurnBracketFrame } from "@/hooks/use-events";
import type { HiveFrames } from "@/lib/hive";
import type { CommsObservation } from "@/views/comms/model";

/** One turn the ledger saw open. */
export interface OpenTurn {
  key: string;
  agentId?: string;
  /** The thread the bracket named, so presence can say which chat a turn is in. */
  chatId?: string;
  /** The hive the turn answered in, on a hive turn. */
  hiveId?: string;
  episodeId?: string;
  startedAtMillis: number;
}

/** A settled turn, kept for the pairs and the peak. */
export interface ClosedTurn extends OpenTurn {
  settledAtMillis: number;
}

/** The bracket ledger. */
export interface TurnLedger {
  open: OpenTurn[];
  /** Newest last, bounded by {@link TURN_LEDGER_CAP}. */
  closed: ClosedTurn[];
  /** The most turns open at once, ever. */
  peak: number;
  /** How many turns started while another was open. */
  overlaps: number;
  /** How many times one agent's turn started while its own was still open —
   *  the number the runtime promises is zero. */
  sameAgentOverlaps: number;
}

export const EMPTY_TURN_LEDGER: TurnLedger = Object.freeze({
  open: [],
  closed: [],
  peak: 0,
  overlaps: 0,
  sameAgentOverlaps: 0,
}) as TurnLedger;

/** How many settled turns the ledger keeps. */
export const TURN_LEDGER_CAP = 256;

function keyOf(frame: TurnBracketFrame): string {
  return frame.turnId ?? `${frame.agentId ?? "?"}:${frame.chatId ?? "?"}`;
}

/** Folds one bracket frame. Same object back for a frame that changes nothing. */
export function reduceTurnBracket(ledger: TurnLedger, frame: TurnBracketFrame): TurnLedger {
  if (frame.type === "turn_started") {
    const key = keyOf(frame);
    const sameAgent =
      frame.agentId !== undefined && ledger.open.some((turn) => turn.agentId === frame.agentId);
    const open = [
      ...ledger.open,
      {
        key,
        agentId: frame.agentId,
        chatId: frame.chatId,
        hiveId: frame.hive?.hiveId,
        episodeId: frame.hive?.episodeId,
        startedAtMillis: frame.atMillis,
      },
    ];
    return {
      open,
      closed: ledger.closed,
      peak: Math.max(ledger.peak, open.length),
      overlaps: ledger.overlaps + (ledger.open.length > 0 ? 1 : 0),
      sameAgentOverlaps: ledger.sameAgentOverlaps + (sameAgent ? 1 : 0),
    };
  }
  // A settle matches its start by turn id, else by agent (oldest first), else
  // it is a settle for a turn this console never saw start — a reconnect —
  // and there is nothing to close.
  const key = keyOf(frame);
  let index = ledger.open.findIndex((turn) => turn.key === key);
  if (index === -1 && frame.agentId !== undefined) {
    index = ledger.open.findIndex((turn) => turn.agentId === frame.agentId);
  }
  if (index === -1) return ledger;
  const turn = ledger.open[index];
  const open = ledger.open.filter((_, i) => i !== index);
  const closed = [...ledger.closed, { ...turn, settledAtMillis: frame.atMillis }].slice(
    -TURN_LEDGER_CAP,
  );
  return { ...ledger, open, closed };
}

/** The agents whose turns are open right now. */
export function workingAgents(ledger: TurnLedger): string[] {
  const out: string[] = [];
  for (const turn of ledger.open) {
    if (turn.agentId && !out.includes(turn.agentId)) out.push(turn.agentId);
  }
  return out;
}

/** The kind of contact a coordination edge records. */
export type SpokeVia = "direct" | "private";

/**
 * The comms-graph observations the hive fold implies.
 *
 * Every edge is agent → agent and was watched happen, so they are all history
 * edges (`spoke`), never structure. A line to oneself draws nothing.
 */
export function coordinationObservations(
  frames: HiveFrames,
  ledger?: TurnLedger,
): CommsObservation[] {
  const out: CommsObservation[] = [];
  for (const contact of frames.contacts) {
    for (const to of contact.to) {
      if (to === contact.from) continue;
      out.push({ kind: "spoke", from: contact.from, to, via: contact.via, atMillis: contact.atMillis });
    }
  }
  if (ledger) {
    for (const agentId of workingAgents(ledger)) out.push({ kind: "speaking", agentId });
  }
  return out;
}

/** The numbers the measurement script prints, computed the console's way. */
export interface CoordinationSummary {
  maxConcurrentTurns: number;
  overlaps: number;
  openTurns: number;
  sameAgentOverlaps: number;
  interruptedTurns: number;
  episodesOpened: number;
  episodesSettled: number;
  episodesFailed: number;
  /** `<chatId>/<episodeId>` of every episode still open. */
  episodesOpen: string[];
  /** Turns per episode, by episode id. */
  turnsPerEpisode: Record<string, number>;
  directMessages: number;
  privateLines: number;
  /** Distinct `from→to` agent pairs that spoke, sorted. */
  distinctPairs: string[];
  /** How many episodes each starter route began. */
  starterRoutes: Record<string, number>;
}

/** Folds the two ledgers into one summary — `summarize` in the script. */
export function coordinationSummary(frames: HiveFrames, ledger: TurnLedger): CoordinationSummary {
  const pairs = new Set<string>();
  let directMessages = 0;
  let privateLines = 0;
  for (const contact of frames.contacts) {
    if (contact.via === "direct") directMessages += 1;
    else privateLines += 1;
    for (const to of contact.to) pairs.add(`${contact.from}→${to}`);
  }
  const episodes = frames.order.map((id) => frames.byId[id]).filter((episode) => episode !== undefined);
  const turnsPerEpisode: Record<string, number> = {};
  for (const episode of episodes) turnsPerEpisode[episode.id] = episode.turns;
  return {
    maxConcurrentTurns: ledger.peak,
    overlaps: ledger.overlaps,
    openTurns: ledger.open.length,
    sameAgentOverlaps: ledger.sameAgentOverlaps,
    interruptedTurns: frames.interruptedTurns,
    episodesOpened: episodes.length,
    episodesSettled: episodes.filter((episode) => episode.status === "settled").length,
    episodesFailed: episodes.filter((episode) => episode.status === "failed").length,
    episodesOpen: episodes
      .filter((episode) => episode.status === "open")
      .map((episode) => `${episode.chatId ?? "?"}/${episode.id}`),
    turnsPerEpisode,
    directMessages,
    privateLines,
    distinctPairs: [...pairs].sort(),
    starterRoutes: { ...frames.starterRoutes },
  };
}
