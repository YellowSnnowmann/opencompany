/**
 * What an agent is doing right now, as one word the console can draw.
 *
 * Pure: no React, no store, no clock. The Room store feeds it the live state it
 * already holds (open turns, live tool rows, the frames' agent and reply
 * markers, the turn-bracket ledger, the pending approvals) and gets back an
 * index; `room/store.ts` memoises it and exposes `useAgentPresence`.
 *
 * Precedence, strongest first: approval, working, typing, thinking, queued,
 * inactive.
 *
 * - **approval**: a pending approval names the agent, or a live tool row is
 *   parked `awaiting_approval`. It lasts while the approval is in the list, so
 *   it outlives the turn that raised it.
 * - **working**: a live tool row is running, or the agent has a card run or
 *   delegation in flight (`/tasks/inflight`; agent-wide only, a card run is not
 *   a DM). An agent seen only through the turn-bracket ledger (a hive seat,
 *   which streams no frames) reads here too: its turn is open and nothing finer
 *   is known.
 * - **typing**: the host's `replying` frame arrived and nothing has reset it
 *   (a tool call or thinking frame does, a settle clears it). Tied to the turn,
 *   deliberately not to a timer: an agent's reply can stream for longer than a
 *   person's 8-second `typing` frame.
 * - **thinking**: an open, non-queued turn with no running tool row.
 * - **queued**: every open turn attributed to the agent is waiting on the
 *   per-company serial lock. A chat turn this console did not send is known
 *   only by its bracket, which names no agent: on a DM the thread says whose it
 *   is, and it reads queued until its run's `run_status_changed` says
 *   `running` (the run id is the bracket's turn id).
 * - **inactive**: none of the above. No timer, no open turn.
 */

/** The six states, weakest last. */
export type AgentPresenceState =
  | "approval"
  | "working"
  | "typing"
  | "thinking"
  | "queued"
  | "inactive";

const RANK: Record<AgentPresenceState, number> = {
  approval: 5,
  working: 4,
  typing: 3,
  thinking: 2,
  queued: 1,
  inactive: 0,
};

/** The stronger of two states, by the precedence above. */
export function strongerPresence(a: AgentPresenceState, b: AgentPresenceState): AgentPresenceState {
  return RANK[b] > RANK[a] ? b : a;
}

/**
 * How long a turn may go without a frame before it stops counting as live.
 *
 * A proposal, tuned by nothing yet: a missed `turn_settled` leaves a turn open
 * until reload (`coordination.ts`), and a stuck "working" dot is worse than a
 * dot that gives up on a turn silent for ten minutes.
 */
export const STALE_TURN_MS = 10 * 60 * 1000;

/** What the console knows about one live turn from its frames (keyed as its rows are). */
export interface TurnMeta {
  /** The host thread the frames named. */
  chatId: string;
  /** The last frame was `replying` (nothing has reset it since). */
  replying: boolean;
  /** Wall-clock of the last frame, for the age-out. */
  lastFrameAt: number;
}

/** The slice of a live tool row presence reads. */
export interface PresenceStep {
  status?: string;
}

/** An open turn as the console's Room store holds it. */
export interface PresenceOpenTurn {
  queued: boolean;
  chatId: string;
  agentId?: string;
  /** The run row this turn is, so its bracket in the ledger is not counted twice. */
  turnId?: string;
}

/** A turn the bracket ledger saw open. */
export interface PresenceLedgerTurn {
  /** The bracket's turn id, which for a chat-route turn is its run id. */
  key?: string;
  agentId?: string;
  chatId?: string;
  startedAtMillis: number;
}

/** Everything {@link derivePresence} reads. */
export interface PresenceInputs {
  openTurns: Record<string, readonly PresenceOpenTurn[]>;
  liveStepsByThread: Record<string, readonly PresenceStep[]>;
  liveStepsByMessage: Record<string, readonly PresenceStep[]>;
  /** Who last reported on each turn, keyed like {@link turnMeta}. */
  liveAgentByTurn: Record<string, string>;
  turnMeta: Record<string, TurnMeta>;
  ledgerTurns: readonly PresenceLedgerTurn[];
  /** Agent id to how many approvals of theirs are pending. */
  approvalAgents: Record<string, number>;
  /** Agent id to how many card runs or delegations it has in flight. */
  inflightAgents: Record<string, number>;
  /** Run id to the status its last `run_status_changed` named. */
  runStatuses: Record<string, string>;
  /** Host thread id to the teammate whose DM it is. */
  threadAgents: Record<string, string>;
  now: number;
}

/** The derived answer: one state per agent, and one per (agent, chat). */
export interface PresenceIndex {
  byAgent: ReadonlyMap<string, AgentPresenceState>;
  byAgentChat: ReadonlyMap<string, AgentPresenceState>;
  /** Agents with at least one pending approval, for the per-chat lookup. */
  approving: ReadonlySet<string>;
  /** Host thread id to the teammate whose DM it is, for {@link presenceChatKey}. */
  threadAgents: Readonly<Record<string, string>>;
}

/** The `byAgentChat` key. */
export function presenceKey(agentId: string, chatId: string): string {
  return `${agentId}\u0000${chatId}`;
}

/**
 * One spelling for a DM, whichever the thread id arrived in.
 *
 * A DM has two: the console addresses an ordinary teammate's DM by its **bare**
 * id (`dmThreadId`), so the chat route's turns and frames name `ceo`, while the
 * hive seat that answers it journals its turn brackets under its desk id,
 * `dm:ceo` (`hive/host.rs`). Keyed as they arrived, a real DM turn lit
 * `(ceo, dm:ceo)` and the DM row, which asks for `(ceo, ceo)`, stayed dark.
 *
 * Folded to `dm:<teammate>`, which cannot collide with a desk: a thread the
 * roster says is a teammate's DM maps to it, and a `dm:<id>` single-seat key is
 * already in that form. A pair conversation (`dm:a+b`) is not a teammate's DM
 * and keeps its own key. The one teammate addressed prefixed (a General
 * spelling, `dm:general`) folds to itself, and the bare `general` channel stays
 * #general because the roster never maps it.
 */
export function presenceChatKey(chatId: string, threadAgents: Readonly<Record<string, string>>): string {
  const owner = threadAgents[chatId];
  return owner === undefined ? chatId : `dm:${owner}`;
}

/** Folds the live state into a {@link PresenceIndex}. */
export function derivePresence(inputs: PresenceInputs): PresenceIndex {
  const byAgent = new Map<string, AgentPresenceState>();
  const byAgentChat = new Map<string, AgentPresenceState>();
  const fresh = (at: number) => inputs.now - at < STALE_TURN_MS;
  const chatKey = (chat: string) => presenceChatKey(chat, inputs.threadAgents);
  const bump = (agent: string, chat: string | undefined, state: AgentPresenceState) => {
    byAgent.set(agent, strongerPresence(byAgent.get(agent) ?? "inactive", state));
    if (chat === undefined) return;
    const key = presenceKey(agent, chatKey(chat));
    byAgentChat.set(key, strongerPresence(byAgentChat.get(key) ?? "inactive", state));
  };

  // 1. Turns the frames speak for: the agent is whoever the frames named, which
  //    is truer than the one the host started the turn on.
  const framedChats = new Set<string>();
  const framedAgents = new Set<string>();
  const framedPairs = new Set<string>();
  const lastFrameByAgent = new Map<string, number>();
  for (const [key, meta] of Object.entries(inputs.turnMeta)) {
    const agent = inputs.liveAgentByTurn[key];
    if (!agent || !fresh(meta.lastFrameAt)) continue;
    framedChats.add(chatKey(meta.chatId));
    framedAgents.add(agent);
    framedPairs.add(presenceKey(agent, chatKey(meta.chatId)));
    lastFrameByAgent.set(agent, Math.max(lastFrameByAgent.get(agent) ?? 0, meta.lastFrameAt));
    const steps = inputs.liveStepsByMessage[key] ?? inputs.liveStepsByThread[key] ?? [];
    let state: AgentPresenceState = meta.replying ? "typing" : "thinking";
    if (steps.some((s) => s.status === "running")) state = "working";
    if (steps.some((s) => s.status === "awaiting_approval")) state = "approval";
    bump(agent, meta.chatId, state);
  }

  // 2. Open turns nobody has framed yet (just accepted, or queued): the guess
  //    the host recorded, else the teammate whose DM this is.
  const ownTurnIds = new Set<string>();
  for (const turns of Object.values(inputs.openTurns)) {
    for (const turn of turns) {
      if (turn.turnId) ownTurnIds.add(turn.turnId);
      if (framedChats.has(chatKey(turn.chatId))) continue;
      const agent = turn.agentId ?? inputs.threadAgents[turn.chatId];
      if (!agent) continue;
      bump(agent, turn.chatId, turn.queued ? "queued" : "thinking");
    }
  }

  // 3. The bracket ledger, for turns this console did not send.
  //
  //    A seat's bracket names its agent and the seat streams no frames: open
  //    and nothing finer is known, so "working", unless frames already describe
  //    that agent in that same chat. Skipped per (agent, chat), not per agent:
  //    the agent framed on another desk says nothing about this one.
  //
  //    A chat-route bracket names no agent (the host has not routed it yet). On
  //    a DM the thread says whose it is, and its run's status says whether it
  //    holds the lock: queued until `running`, then thinking, and nothing once
  //    it has ended, in case the settle itself was missed. A turn this console
  //    holds in `openTurns` is step 2's, and is not counted twice.
  for (const turn of inputs.ledgerTurns) {
    if (turn.key !== undefined && ownTurnIds.has(turn.key)) continue;
    const seat = turn.agentId !== undefined;
    const agent =
      turn.agentId ?? (turn.chatId === undefined ? undefined : inputs.threadAgents[turn.chatId]);
    if (!agent) continue;
    const framed =
      turn.chatId === undefined
        ? framedAgents.has(agent)
        : seat
          ? framedPairs.has(presenceKey(agent, chatKey(turn.chatId)))
          : framedChats.has(chatKey(turn.chatId));
    if (framed) continue;
    if (!fresh(Math.max(turn.startedAtMillis, lastFrameByAgent.get(agent) ?? 0))) continue;
    if (seat) {
      bump(agent, turn.chatId, "working");
      continue;
    }
    const status = turn.key === undefined ? undefined : inputs.runStatuses[turn.key];
    if (status === undefined || status === "pending") bump(agent, turn.chatId, "queued");
    else if (status === "running") bump(agent, turn.chatId, "thinking");
  }

  // 4. Card runs and delegations in flight: busy, but not in any conversation,
  //    so agent-wide only (a DM row stays about the DM).
  for (const [agent, count] of Object.entries(inputs.inflightAgents)) {
    if (count > 0) bump(agent, undefined, "working");
  }

  // 5. Pending approvals outlive the turn, so they are read last and win.
  const approving = new Set<string>();
  for (const [agent, count] of Object.entries(inputs.approvalAgents)) {
    if (count <= 0) continue;
    approving.add(agent);
    byAgent.set(agent, "approval");
  }
  return { byAgent, byAgentChat, approving, threadAgents: inputs.threadAgents };
}

/** One agent's state across every chat. */
export function presenceOf(index: PresenceIndex, agentId: string): AgentPresenceState {
  return index.byAgent.get(agentId) ?? "inactive";
}

/**
 * One agent's state in one chat: what it is doing there, or "approval" when it
 * has a pending approval anywhere (an approval is the operator's to act on
 * wherever they are looking, so it is not scoped to the conversation).
 */
export function presenceIn(index: PresenceIndex, agentId: string, chatId: string): AgentPresenceState {
  if (index.approving.has(agentId)) return "approval";
  const key = presenceKey(agentId, presenceChatKey(chatId, index.threadAgents));
  return index.byAgentChat.get(key) ?? "inactive";
}

/**
 * `meta` without the entries `drop` names, or the same object when none match
 * (so the store's identity check skips the notify).
 */
export function dropTurnMeta(
  meta: Record<string, TurnMeta>,
  drop: (key: string, entry: TurnMeta) => boolean,
): Record<string, TurnMeta> {
  let next: Record<string, TurnMeta> | null = null;
  for (const [key, entry] of Object.entries(meta)) {
    if (!drop(key, entry)) continue;
    next ??= { ...meta };
    delete next[key];
  }
  return next ?? meta;
}

/**
 * What one live frame says about its turn, for the presence dot: the thread it
 * named, whether the agent is now writing its reply (`replying`), and when. A
 * tool or thinking frame ends a run of text, so it resets the flag. No expiry:
 * it holds until the next frame, a settle or the reply.
 */
export function frameTurnMeta(chatId: string, frameType: string, now: number): TurnMeta {
  return { chatId, replying: frameType === "replying", lastFrameAt: now };
}

/**
 * What a `turn_settled` that names `chatId` retires: every turn the frames
 * described in that conversation, whichever spelling of it they used. A DM's
 * frames name the bare teammate id while its seat settles under `dm:<id>`, so
 * a raw comparison left a `dm:rae` settle unable to clear `rae`-keyed state.
 */
export function settledInChat(
  chatId: string,
  threadAgents: Readonly<Record<string, string>>,
): (key: string, entry: TurnMeta) => boolean {
  const settled = presenceChatKey(chatId, threadAgents);
  return (_key, entry) => presenceChatKey(entry.chatId, threadAgents) === settled;
}

/**
 * What clearing a thread's live state retires from {@link TurnMeta}.
 *
 * Always the thread's own bucket. The per-query buckets that name the thread
 * only when the turn they describe is over (its reply landed, its POST ended):
 * starting a *second* question on the thread must not blank the first one's
 * dot while it is still running, and a query key is never reused, so it has
 * nothing stale to reset.
 */
export function clearedOnThread(
  threadId: string,
  { queries }: { queries: boolean },
): (key: string, entry: TurnMeta) => boolean {
  return (key, entry) => key === threadId || (queries && entry.chatId === threadId);
}

/** Whether `entry` has gone {@link STALE_TURN_MS} without a frame, for the garbage collection tick. */
export function staleTurnMeta(now: number): (key: string, entry: TurnMeta) => boolean {
  return (_key, entry) => now - entry.lastFrameAt >= STALE_TURN_MS;
}

/** The slice of an approval summary presence reads. */
export interface PresenceApproval {
  id?: string;
  agent?: string | null;
  /** The hive seat that raised it; `seat` is the roster id. */
  episode?: { seat: string } | null;
}

/**
 * Counts pending approvals per asking agent.
 *
 * The asker is `agent`, else the episode seat that raised it (a seat's approval
 * can arrive without `agent`). One no agent raised counts for nobody. `resolved`
 * names approvals a `approval_resolved` frame already settled (an expiry
 * included) while the feed still lists them: the dot goes out on the frame,
 * not a poll later.
 */
export function approvalAgentCounts(
  approvals: readonly PresenceApproval[],
  resolved?: Readonly<Record<string, unknown>>,
): Record<string, number> {
  const out: Record<string, number> = {};
  for (const approval of approvals) {
    if (approval.id !== undefined && resolved && approval.id in resolved) continue;
    const asker = approval.agent || approval.episode?.seat;
    if (asker) out[asker] = (out[asker] ?? 0) + 1;
  }
  return out;
}

/** Counts in-flight card runs and delegations per agent (`/tasks/inflight`). */
export function inflightAgentCounts(runs: readonly { agentId?: string | null }[]): Record<string, number> {
  const out: Record<string, number> = {};
  for (const run of runs) {
    if (run.agentId) out[run.agentId] = (out[run.agentId] ?? 0) + 1;
  }
  return out;
}

/** How many `run_status_changed` words {@link recordRunStatus} keeps. */
export const RUN_STATUS_CAP = 200;

/**
 * Records a chat turn's `run_status_changed`, bounded to the newest
 * {@link RUN_STATUS_CAP} runs so a console left open does not grow without end.
 * Same object back when nothing changed.
 */
export function recordRunStatus(
  statuses: Record<string, string>,
  runId: string,
  status: string,
): Record<string, string> {
  if (statuses[runId] === status) return statuses;
  const next = { ...statuses };
  delete next[runId];
  next[runId] = status;
  const keys = Object.keys(next);
  for (let i = 0; i < keys.length - RUN_STATUS_CAP; i++) delete next[keys[i]];
  return next;
}

/** Whether two count maps say the same thing, so a poll that changed nothing changes nothing. */
export function sameCounts(a: Record<string, number>, b: Record<string, number>): boolean {
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every((k) => a[k] === b[k]);
}
