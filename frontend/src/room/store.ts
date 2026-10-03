// The Room's own state, out of the shell.
//
// A module-level store read through React 18's `useSyncExternalStore`, modelled
// on `connections/registry.ts` — no state library, for the same reason that file
// gives: the console has none, and the absence of a global query cache is what
// stops two hosts' data getting mixed.
//
// # Why this exists
//
// Every one of these fields lived in `app-shell.tsx`, and had to: the shell
// mounts and unmounts the Room per route, so component-local state there is
// discarded on every trip away and back. The cost was a ~50-prop call site and,
// more tellingly, two `useRef` mirrors (`transcriptsRef`, `openTurnsRef`) kept
// only so stable callbacks could read state outside React's render cycle. Those
// refs are the tell: what the shell wanted was a store, and it was hand-rolling
// half of one.
//
// # One scope at a time, and no cache
//
// Keyed by (connection, company), and switching scope **resets** rather than
// stashing the old one. That is deliberate and it is the registry's own warning
// applied here: "a cache keyed on anything less than (connection, company) is
// exactly how two hosts' data gets mixed, and the way to not have that bug is to
// not have the cache". It also matches what the shell already did — it cleared
// transcripts by hand on every company change.
//
// # The setters keep `useState` semantics on purpose
//
// Each field exposes a `useX()` reader and a `setX(next | updater)` writer whose
// contract is identical to the `useState` setter it replaces. That is what makes
// the migration a swap rather than a rewrite: ~50 call sites in the shell move
// across untouched, and the twelve live-reply / detach / hydration specs that
// guard the invariants keep passing with no edit. Semantic writers ("a reply
// landed", "a send failed") would be a nicer API and a much worse diff; they can
// come later, on top of a store whose behaviour is already pinned.

import { useSyncExternalStore } from "react";

import type { TurnStep } from "@/api/types";
import {
  derivePresence,
  presenceIn,
  presenceOf,
  type AgentPresenceState,
  type PresenceIndex,
  type PresenceLedgerTurn,
  type TurnMeta,
} from "@/lib/agent-presence";
import type { OpenTurn } from "@/lib/live-reply";
import type { ChatReceipt } from "@/views/room/ChatLiveReceipt";
import {
  HISTORY_UNSTARTED,
  type HistoryHydration,
  type Transcripts,
} from "@/views/room/model";

/** A live tool row: a `TurnStep` plus the transient id its running→done flip uses. */
export type LiveStep = TurnStep & { toolCallId?: string };

/** Everything the Room needs that must outlive its unmount. */
export interface RoomState {
  transcripts: Transcripts;
  hydration: HistoryHydration;
  chatChannelByThread: Record<string, string>;
  lastViewedChannel: Record<string, number>;
  unreadSince: number;
  liveStepsByThread: Record<string, LiveStep[]>;
  liveStepsByMessage: Record<string, LiveStep[]>;
  receiptByThread: Record<string, ChatReceipt>;
  /**
   * Who last reported on each open turn, keyed exactly as its rows are — by
   * query where the host stamped `messageSeq`, by thread otherwise.
   *
   * The live answer to "who is working", as distinct from `openTurns`' answer.
   * An `OpenTurn` names the agent the host *started* the turn on and never
   * revises it, which is right for a single responder and wrong the moment the
   * floor moves: a desk hand-off runs the delegate under the same query, and a
   * deliberating room passes the floor between seats for the whole episode.
   * Only the frames say which seat is speaking now, and until this they said it
   * to nothing — `agentId` arrived on every frame and was dropped.
   */
  liveAgentByTurn: Record<string, string>;
  openTurns: Record<string, OpenTurn[]>;
  /**
   * What each live turn's frames last said, keyed exactly as `liveAgentByTurn`
   * is: the thread they named and whether the last one was the host's
   * `replying` marker. The half of "typing" that `liveStepsBy*` cannot hold,
   * because a reply with no tool call leaves no row behind.
   */
  turnMeta: Record<string, TurnMeta>;
  /** The turn-bracket ledger's open turns, mirrored from the shell's reducer. */
  ledgerTurns: PresenceLedgerTurn[];
  /** Agent id to how many of its approvals are pending, mirrored from the feed. */
  approvalAgents: Record<string, number>;
  /** Agent id to how many card runs or delegations it has in flight, from `/tasks/inflight`. */
  inflightAgents: Record<string, number>;
  /** Chat run id to the status its last `run_status_changed` named. */
  runStatuses: Record<string, string>;
  /** Host thread id to the teammate whose DM it is, from the roster. */
  threadAgents: Record<string, string>;
  /** Wall-clock the presence age-out reads; ticked by the shell. */
  presenceNow: number;
}

/**
 * The empty arrays and objects readers fall back to.
 *
 * Module constants rather than fresh literals, and this is load-bearing:
 * `useSyncExternalStore` compares snapshots by identity, so a reader returning
 * `?? []` would hand React a new array every render and loop forever.
 */
const NO_STEPS: LiveStep[] = [];
const NO_TURNS: OpenTurn[] = [];
const NO_MESSAGES: Transcripts[string] = [];

function emptyState(): RoomState {
  return {
    transcripts: {},
    hydration: HISTORY_UNSTARTED,
    chatChannelByThread: {},
    lastViewedChannel: {},
    // The floor for a channel never looked at. Set when the scope opens, so a
    // company switched into does not inherit the previous one's reading point.
    unreadSince: Date.now(),
    liveStepsByThread: {},
    liveStepsByMessage: {},
    receiptByThread: {},
    liveAgentByTurn: {},
    openTurns: {},
    turnMeta: {},
    ledgerTurns: [],
    approvalAgents: {},
    inflightAgents: {},
    runStatuses: {},
    threadAgents: {},
    presenceNow: Date.now(),
  };
}

let scopeKey: string | null = null;
let state: RoomState = emptyState();
let listeners: Array<() => void> = [];

function emit(): void {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.push(listener);
  return () => {
    listeners = listeners.filter((l) => l !== listener);
  };
}

/**
 * Point the store at a (connection, company) pair, clearing it if that is a
 * change.
 *
 * Idempotent: re-entering the scope you are already in is a no-op, so an effect
 * that runs on every render cannot wipe a transcript mid-conversation.
 */
export function enterScope(key: string): void {
  if (scopeKey === key) return;
  scopeKey = key;
  state = emptyState();
  emit();
}

/** The scope the store currently holds, for tests and assertions. */
export function currentScope(): string | null {
  return scopeKey;
}

/** Drop everything and forget the scope. Tests use this; the app does not. */
export function resetStore(): void {
  scopeKey = null;
  state = emptyState();
  emit();
}

/** The whole state, synchronously — the replacement for the shell's refs. */
export function readRoom(): RoomState {
  return state;
}

type Updater<T> = T | ((prev: T) => T);

function apply<T>(prev: T, next: Updater<T>): T {
  return typeof next === "function" ? (next as (p: T) => T)(prev) : next;
}

/**
 * Build the reader/writer pair for one field.
 *
 * One factory rather than nine hand-written pairs, so every field is guaranteed
 * the same two properties: a writer that skips the notify when nothing changed,
 * and a reader whose snapshot identity is the field's own — not the state
 * object's. Without the second, a `tool_call` frame touching `liveStepsByThread`
 * would re-render every subscriber of every other field, and the store would be
 * strictly worse than the prop drill it replaced.
 */
function field<K extends keyof RoomState>(key: K) {
  const set = (next: Updater<RoomState[K]>): void => {
    const value = apply(state[key], next);
    if (Object.is(value, state[key])) return;
    state = { ...state, [key]: value };
    emit();
  };
  const read = (): RoomState[K] => state[key];
  const use = (): RoomState[K] => useSyncExternalStore(subscribe, read, read);
  return { set, read, use };
}

const transcripts = field("transcripts");
const hydration = field("hydration");
const chatChannelByThread = field("chatChannelByThread");
const lastViewedChannel = field("lastViewedChannel");
const unreadSince = field("unreadSince");
const liveStepsByThread = field("liveStepsByThread");
const liveStepsByMessage = field("liveStepsByMessage");
const receiptByThread = field("receiptByThread");
const liveAgentByTurn = field("liveAgentByTurn");
const openTurns = field("openTurns");
const turnMeta = field("turnMeta");
const ledgerTurns = field("ledgerTurns");
const approvalAgents = field("approvalAgents");
const inflightAgents = field("inflightAgents");
const runStatuses = field("runStatuses");
const threadAgents = field("threadAgents");
const presenceNow = field("presenceNow");

/* ---- writers: `useState` semantics, so call sites move across untouched ---- */

export const setTranscripts = transcripts.set;
export const setHydration = hydration.set;
export const setChatChannelByThread = chatChannelByThread.set;
export const setLastViewedChannel = lastViewedChannel.set;
export const setUnreadSince = unreadSince.set;
export const setLiveStepsByThread = liveStepsByThread.set;
export const setLiveStepsByMessage = liveStepsByMessage.set;
export const setReceiptByThread = receiptByThread.set;
export const setLiveAgentByTurn = liveAgentByTurn.set;
export const setOpenTurns = openTurns.set;
export const setTurnMeta = turnMeta.set;
export const setLedgerTurns = ledgerTurns.set;
export const setApprovalAgents = approvalAgents.set;
export const setInflightAgents = inflightAgents.set;
export const setRunStatuses = runStatuses.set;
export const setThreadAgents = threadAgents.set;
export const setPresenceNow = presenceNow.set;

/**
 * Writers bound to the scope that created an asynchronous operation.
 *
 * A route switch replaces the module-global state synchronously. Callbacks
 * retained by the old shell must therefore become no-ops rather than applying
 * their result to the replacement company's identically named channel.
 */
export function writersForScope(key: string) {
  const guard = <T>(writer: (next: Updater<T>) => void) => (next: Updater<T>): void => {
    if (scopeKey === key) writer(next);
  };
  return {
    setTranscripts: guard(setTranscripts),
    setHydration: guard(setHydration),
    setChatChannelByThread: guard(setChatChannelByThread),
    setLastViewedChannel: guard(setLastViewedChannel),
    setUnreadSince: guard(setUnreadSince),
    setLiveStepsByThread: guard(setLiveStepsByThread),
    setLiveStepsByMessage: guard(setLiveStepsByMessage),
    setReceiptByThread: guard(setReceiptByThread),
    setLiveAgentByTurn: guard(setLiveAgentByTurn),
    setOpenTurns: guard(setOpenTurns),
    setTurnMeta: guard(setTurnMeta),
    setLedgerTurns: guard(setLedgerTurns),
    setApprovalAgents: guard(setApprovalAgents),
    setInflightAgents: guard(setInflightAgents),
    setRunStatuses: guard(setRunStatuses),
    setThreadAgents: guard(setThreadAgents),
    setPresenceNow: guard(setPresenceNow),
  };
}

/* ---- readers ---- */

export const useTranscripts = transcripts.use;
export const useHydration = hydration.use;
export const useChatChannelByThread = chatChannelByThread.use;
export const useLastViewedChannel = lastViewedChannel.use;
export const useUnreadSince = unreadSince.use;
export const useLiveStepsByThread = liveStepsByThread.use;
export const useLiveStepsByMessage = liveStepsByMessage.use;
export const useReceiptByThread = receiptByThread.use;
export const useLiveAgentByTurn = liveAgentByTurn.use;
export const useOpenTurns = openTurns.use;

/**
 * One channel's rows, with a stable identity while other channels change.
 *
 * The narrow reader the timeline should use. Every writer replaces the
 * `transcripts` record but leaves untouched channels' arrays alone, so this
 * returns the same array — and the same React subtree stays un-rendered — while
 * a busy neighbour channel takes a hundred frames.
 */
export function useTranscript(channelId: string | null): Transcripts[string] {
  const read = (): Transcripts[string] =>
    (channelId === null ? undefined : state.transcripts[channelId]) ?? NO_MESSAGES;
  return useSyncExternalStore(subscribe, read, read);
}

/** One thread's live tool rows, with the same identity guarantee. */
export function useLiveSteps(threadId: string | null): LiveStep[] {
  const read = (): LiveStep[] =>
    (threadId === null ? undefined : state.liveStepsByThread[threadId]) ?? NO_STEPS;
  return useSyncExternalStore(subscribe, read, read);
}

/** One thread's open turns, head-first, with the same identity guarantee. */
export function useThreadTurns(threadId: string | null): OpenTurn[] {
  const read = (): OpenTurn[] =>
    (threadId === null ? undefined : state.openTurns[threadId]) ?? NO_TURNS;
  return useSyncExternalStore(subscribe, read, read);
}

/**
 * The presence index, recomputed only when one of its inputs is replaced.
 *
 * Every subscriber of `useAgentPresence` reads the snapshot on every store
 * change, so without this each frame would re-derive the whole index once per
 * avatar on screen. Inputs are compared by identity, which is what the field
 * setters preserve for an unchanged field.
 */
let presenceCache: { deps: unknown[]; index: PresenceIndex } | null = null;

function presenceIndex(): PresenceIndex {
  const deps: unknown[] = [
    state.openTurns,
    state.liveStepsByThread,
    state.liveStepsByMessage,
    state.liveAgentByTurn,
    state.turnMeta,
    state.ledgerTurns,
    state.approvalAgents,
    state.inflightAgents,
    state.runStatuses,
    state.threadAgents,
    state.presenceNow,
  ];
  if (presenceCache && presenceCache.deps.every((d, i) => Object.is(d, deps[i]))) {
    return presenceCache.index;
  }
  const index = derivePresence({
    openTurns: state.openTurns,
    liveStepsByThread: state.liveStepsByThread,
    liveStepsByMessage: state.liveStepsByMessage,
    liveAgentByTurn: state.liveAgentByTurn,
    turnMeta: state.turnMeta,
    ledgerTurns: state.ledgerTurns,
    approvalAgents: state.approvalAgents,
    inflightAgents: state.inflightAgents,
    runStatuses: state.runStatuses,
    threadAgents: state.threadAgents,
    now: state.presenceNow,
  });
  presenceCache = { deps, index };
  return index;
}

/**
 * What an agent is doing, as one of six words (see `lib/agent-presence.ts`).
 *
 * Across every chat by default. Pass `chatId` for a surface that is about one
 * conversation (a DM row, a DM header), so an agent busy in another channel does
 * not light this one. A plain string, so the snapshot has a stable identity with
 * no constant objects to hold.
 */
export function useAgentPresence(agentId: string | null | undefined, chatId?: string | null): AgentPresenceState {
  const read = (): AgentPresenceState => {
    if (!agentId) return "inactive";
    const index = presenceIndex();
    return chatId ? presenceIn(index, agentId, chatId) : presenceOf(index, agentId);
  };
  return useSyncExternalStore(subscribe, read, read);
}

/** Presence states, strongest first — the order `lib/agent-presence.ts` ranks them in. */
const PRESENCE_RANK: readonly AgentPresenceState[] = [
  "approval",
  "working",
  "typing",
  "thinking",
  "queued",
  "inactive",
];

/**
 * The busiest of several agents in one chat, for a row that stands for a group
 * (a channel in the sidebar): who is doing the most there, and what.
 *
 * `null` when every one of them is inactive. The snapshot is a string
 * (`"<agentId>\u0000<state>"`) so it keeps a stable identity across store
 * changes that move nothing, and is split back apart here.
 */
export function useBusiestPresence(
  agentIds: readonly string[] | undefined,
  chatId: string | null,
): { agentId: string; state: AgentPresenceState } | null {
  const read = (): string => {
    if (!agentIds?.length || !chatId) return "";
    const index = presenceIndex();
    let best = "";
    let bestRank = PRESENCE_RANK.length - 1;
    for (const id of agentIds) {
      const rank = PRESENCE_RANK.indexOf(presenceIn(index, id, chatId));
      if (rank < bestRank) {
        bestRank = rank;
        best = `${id}\u0000${PRESENCE_RANK[rank]}`;
      }
    }
    return best;
  };
  const snapshot = useSyncExternalStore(subscribe, read, read);
  if (!snapshot) return null;
  const [agentId, state] = snapshot.split("\u0000");
  return { agentId, state: state as AgentPresenceState };
}
