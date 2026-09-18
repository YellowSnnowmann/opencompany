/**
 * Folding a desk transcript back into the episodes it recorded.
 *
 * A deliberating desk journals nothing but its own turns: one reply per turn
 * under the teammate that spoke, then one closing row under `hive-report`
 * (`docs/spec/runtime/hivemind.md`, "What lands in the journal"). There is no
 * episode record and no second store — **the transcript is the episode**, which
 * is what makes the standings impossible to disagree with the conversation they
 * were folded from.
 *
 * So the console reconstructs rather than reads. This module is that fold: pure,
 * over the `ChatMessage[]` the transcript already holds, producing the topics,
 * supporters, citations and ending a room reached.
 *
 * # Everything here is an approximation, and says so
 *
 * The host folds with `tinyhivemind_hive`, which this file deliberately does not
 * reimplement in full — it has an attention market, a salience model and a
 * windowed quorum that a console has no business restating. What it reproduces
 * is the part an operator reads: who proposed what, who backed it with what
 * grounds, who objected to whom, and how it ended.
 *
 * Every episode is therefore marked {@link Episode.derived}. Where the host's own
 * closing sentence is present it is the authority, and {@link Episode.ending} is
 * taken from it verbatim rather than from our count — our standings are shown
 * beside it, never in place of it. {@link Episode.disagrees} records when the two
 * differ, so a mismatch is visible rather than silently rendered as fact.
 *
 * When the typed episode frames land this module keeps its shape and loses its
 * guesswork: `derived` flips false and the ending arrives rather than being
 * parsed.
 */

import { toHostMessageId, type ChatMessage } from "@/lib/chat";
import { looksDemoted, moveOf, type Move, type MoveKind } from "@/lib/hive/grammar";

/**
 * The author of a room's own closing summary.
 *
 * Hyphenated on purpose, exactly as `workflow-report` is: the host's id minter
 * rejects a hyphen, so no roster teammate can ever hold this id and the room's
 * summary can never be misattributed to one.
 */
export const HIVE_REPORT_AUTHOR = "hive-report";

/**
 * The author an answer carried back from another desk arrives under.
 *
 * It folds as a system row, which is load-bearing in both directions: a system
 * row is never counted as a supporter, and is never hidden by a blind round. A
 * referral informs the room without ever authoring a position in it.
 */
export const HIVE_REFERRAL_AUTHOR = "hive-referral";

/** One turn a member took inside an episode. */
export interface EpisodeTurn {
  /** The console id of the message that carried it (`h<seq>`). */
  messageId: string;
  /** The host sequence, which is what a `^N` citation names. */
  seq: number | null;
  agentId: string;
  at: number;
  /** The move it opened with, or `null` for prose that deposited no trace. */
  move: Move | null;
  /**
   * Whether it belongs to the opening round, in which each member wrote before
   * it could read its peers.
   *
   * Inferred while {@link Episode.derived} is true — see {@link foldEpisodes} —
   * and rendered with that uncertainty rather than as a claim.
   */
  blind: boolean;
  /**
   * The move this line appears to have attempted with its `!` stripped, i.e. a
   * move its seat may not make. Annotation only; never counted.
   */
  demoted: MoveKind | null;
  /** A `hive-report` note saying a member's turn did not finish. */
  failed: boolean;
  /** Set when the row is an answer carried home from another desk. */
  referral: boolean;
}

/** Where one option stands. */
export interface TopicStanding {
  id: string;
  proposedBy: string | null;
  /** Distinct members backing it, in first-support order, minus the silenced. */
  supporters: string[];
  /**
   * Backers whose support cited nothing.
   *
   * Kept apart rather than dropped: on a `require_evidential` desk these do not
   * count, and a transcript that reads as though they did is the failure worth
   * showing.
   */
  ungrounded: string[];
  /** Advocates silenced by a grounded objection, in first-silenced order. */
  silenced: string[];
  /** Distinct members who refuted it. */
  refuters: string[];
  /** Sequences deposited as grounds for it. */
  evidence: number[];
  /** Whether it cleared the quorum in force. */
  carried: boolean;
}

export type EpisodeEnding =
  | { kind: "converged"; topic: string; supporters: string[]; turns: number }
  | { kind: "deadlocked"; topics: string[]; turns: number }
  | { kind: "exhausted"; turns: number }
  | { kind: "idle"; turns: number };

/** One room, folded back out of the conversation it happened in. */
export interface Episode {
  /** Stable across reloads: the trigger's own message id. */
  key: string;
  /** The operator message that opened it, when one is in the loaded window. */
  triggerId: string | null;
  triggerSeq: number | null;
  turns: EpisodeTurn[];
  topics: TopicStanding[];
  /** How many turns the opening blind round ran for. */
  blindCount: number;
  /** The ending, from the host's own sentence when present. */
  ending: EpisodeEnding | null;
  /** The closing row's id and text, rendered verbatim — never paraphrased. */
  reportId: string | null;
  reportText: string | null;
  /** Turns that did not finish, as the report named them. */
  failed: EpisodeTurn[];
  /** Answers carried home from another desk. */
  referrals: EpisodeTurn[];
  /** The quorum used to decide `carried`, and where it came from. */
  quorum: number;
  quorumDerived: boolean;
  /**
   * The turns this room may spend before it reports itself exhausted.
   *
   * What makes a *running* episode legible: without it a reader watching turns
   * land has no idea whether the room is a third of the way through or about to
   * run out, and "exhausted" arrives as a surprise. The host derives it from the
   * membership when the manifest names none, and so does this — flagged, because
   * a derived budget is a guess about a desk whose `hive` block the console may
   * not have read.
   */
  turnBudget: number;
  turnBudgetDerived: boolean;
  /** True while any of this was parsed rather than received as typed frames. */
  derived: boolean;
  /**
   * Set when our fold and the host's closing sentence name different outcomes.
   *
   * Rendered as a note beside the host's verdict, never as a contradiction: the
   * report wins on screen, and this says the console could not reproduce it.
   */
  disagrees: boolean;
  /**
   * Set when segmentation could not confidently separate this episode from
   * another running in the same thread.
   *
   * Two concurrent episodes on one desk are not separable from the journal —
   * the host keeps that boundary in memory and durable rows carry no instance
   * id — so this is flagged rather than guessed.
   */
  ambiguous: boolean;
}

/** The host sequence a `^N` citation would name for this message. */
function seqOf(message: ChatMessage): number | null {
  const host = toHostMessageId(message.id);
  if (host === null) return null;
  const n = Number(host);
  return Number.isFinite(n) ? n : null;
}

/** Whether a row is a `hive-report` note about one member's failed turn. */
function isFailureNote(text: string): boolean {
  return /^@[^']+'s turn did not finish:/.test(text.trimStart());
}

/**
 * The ending a closing `hive-report` sentence names.
 *
 * The four sentences are stable and quoted verbatim in
 * `docs/spec/runtime/hivemind.md`; each names its ending word. Matched as a
 * prefix rather than the whole string, because two postscripts may follow any of
 * them — how many turns did not finish, and which lines were demoted.
 */
export function parseEndingReport(text: string): EpisodeEnding | null {
  const line = text.trimStart();

  const converged =
    /^The desk (?:settled on #(\S+) after (\d+) turns? \(backed by ([^)]*)\)\.|settled after (\d+) turns? \(#(\S+), backed by ([^)]*)\):)/.exec(
      line,
    );
  if (converged) {
    const backing = (converged[3] ?? converged[6]).trim();
    return {
      kind: "converged",
      topic: converged[1] ?? converged[5],
      // "the room" is the host's stand-in for an empty supporter list, and is
      // not a member id — carrying it through would invent a teammate.
      supporters: backing === "the room" || backing === "" ? [] : backing.split(", "),
      turns: Number(converged[2] ?? converged[4]),
    };
  }

  const deadlocked = /^The desk deadlocked after (\d+) turns?: (.+?) carried together and nobody broke the tie\./.exec(
    line,
  );
  if (deadlocked)
    return {
      kind: "deadlocked",
      topics: deadlocked[2].split(" and ").map((t) => t.replace(/^#/, "")),
      turns: Number(deadlocked[1]),
    };

  const exhausted = /^The desk spent its (\d+)-turn budget without reaching a decision\./.exec(
    line,
  );
  if (exhausted) return { kind: "exhausted", turns: Number(exhausted[1]) };

  if (/^Nobody on the desk had anything to add, so the room did not open\./.test(line))
    return { kind: "idle", turns: 0 };

  return null;
}

/**
 * Fold the moves of one episode into per-topic standings.
 *
 * The rules that are not obvious, each from the spec:
 *
 * - **A proposal counts as its own author's support.** That is not a rounding
 *   error, it is why a room of seats that may all propose votes instead of
 *   deliberating.
 * - **An objection is local**: it silences the advocate of the message it
 *   targets, on that advocate's topics. A refutation is global and caps the
 *   topic itself.
 * - **A refuted topic keeps its supporters and stays in the standings**, so a
 *   reader can audit the refutation back to the message it cites.
 */
export function foldStandings(turns: EpisodeTurn[], quorum: number): TopicStanding[] {
  const byId = new Map<string, TopicStanding>();
  const authorOfSeq = new Map<number, string>();
  // Which option a message was advocating, so an objection aimed at it can be
  // applied where it belongs. An objection is **local** — it removes one
  // advocate from one topic — where a refutation is global and caps the option
  // itself. Silencing an advocate everywhere would quietly make every objection
  // a refutation, and a room would lose support nobody argued against.
  const topicOfSeq = new Map<number, string>();
  for (const turn of turns) {
    if (turn.seq === null) continue;
    authorOfSeq.set(turn.seq, turn.agentId);
    const move = turn.move;
    if (move?.topic && (move.kind === "propose" || move.kind === "support"))
      topicOfSeq.set(turn.seq, move.topic);
  }

  const ensure = (id: string): TopicStanding => {
    let standing = byId.get(id);
    if (!standing) {
      standing = {
        id,
        proposedBy: null,
        supporters: [],
        ungrounded: [],
        silenced: [],
        refuters: [],
        evidence: [],
        carried: false,
      };
      byId.set(id, standing);
    }
    return standing;
  };

  // Support first, then objections and refutations, so the result does not
  // depend on the order traces arrived in — the same discipline the host's fold
  // documents.
  for (const turn of turns) {
    const move = turn.move;
    if (!move?.topic) continue;
    const standing = ensure(move.topic);
    if (move.kind === "propose") {
      standing.proposedBy ??= turn.agentId;
      if (!standing.supporters.includes(turn.agentId)) standing.supporters.push(turn.agentId);
    } else if (move.kind === "support") {
      if (!standing.supporters.includes(turn.agentId)) standing.supporters.push(turn.agentId);
      if (move.cites.length === 0 && !standing.ungrounded.includes(turn.agentId))
        standing.ungrounded.push(turn.agentId);
    } else if (move.kind === "evidence") {
      if (turn.seq !== null) standing.evidence.push(turn.seq);
    } else if (move.kind === "refute") {
      if (!standing.refuters.includes(turn.agentId)) standing.refuters.push(turn.agentId);
    }
  }

  for (const turn of turns) {
    const move = turn.move;
    if (move?.kind !== "object" || move.target === undefined) continue;
    const silencedAgent = authorOfSeq.get(move.target);
    // The topic the targeted line was advocating. An objection at a line that
    // advocated nothing — a question, a piece of evidence — silences nobody,
    // because there is no support to withdraw.
    const topic = topicOfSeq.get(move.target);
    if (!silencedAgent || !topic) continue;
    const standing = byId.get(topic);
    if (!standing || !standing.supporters.includes(silencedAgent)) continue;
    standing.supporters = standing.supporters.filter((id) => id !== silencedAgent);
    if (!standing.silenced.includes(silencedAgent)) standing.silenced.push(silencedAgent);
  }

  for (const standing of byId.values()) {
    const grounded = standing.supporters.filter((id) => !standing.ungrounded.includes(id));
    standing.carried = grounded.length >= quorum;
  }

  return [...byId.values()].sort((a, b) => b.supporters.length - a.supporters.length);
}

/**
 * Every episode in one channel's transcript, oldest first.
 *
 * Returns `[]` for a conversation that contains no marker line and no
 * `hive-report` row — which is every DM, every `#general`, the Operator feed and
 * every single-responder desk. **That is the whole rule**: whether the room
 * affordances appear is a question about the data, never about the channel kind
 * or the desk's config, so a hive desk that answered in one ordinary turn renders
 * as an ordinary conversation, which is the truth about it.
 */
/**
 * The body to read a move out of: what the model wrote, not what the operator
 * is shown.
 *
 * The two differ only on a desk that deliberates, where the host rewrites
 * `!support #topic ^3` into prose before it reaches a chat bubble. The fold
 * counts the grammar, so reading the operator-facing body would make the whole
 * deliberation panel depend on the host not having tidied it — and on the
 * reload path, where the host always tidies it, the panel would never appear at
 * all.
 *
 * Falls back to `text` for a host that predates `cueText`, and for every row
 * with no move, where the two are equal anyway.
 */
function bodyOf(m: ChatMessage): string {
  return m.cueText ?? m.text;
}

export function foldEpisodes(
  messages: ChatMessage[],
  options: { quorum?: number; members?: number; turnBudget?: number } = {},
): Episode[] {
  // Hive turns and their closing report are replies to the triggering operator
  // message. Parentage is a thread relationship, not evidence that a row is
  // outside the episode.
  const rows = messages;
  const hasRoom = rows.some(
    (m) =>
      m.channel === HIVE_REPORT_AUTHOR ||
      m.channel === HIVE_REFERRAL_AUTHOR ||
      (m.from === "company" && moveOf(bodyOf(m)) !== null),
  );
  if (!hasRoom) return [];

  const derivedQuorum = options.quorum === undefined;
  const derivedBudget = options.turnBudget === undefined;
  // Three turns per member, the host's own default: an opening position, a reply
  // to the room, and a commit.
  const turnBudget = options.turnBudget ?? Math.max(2, options.members ?? 3) * 3;
  const quorum =
    options.quorum ??
    Math.max(1, Math.min(Math.floor(Math.max(2, options.members ?? 3) / 2) + 1, Math.max(2, options.members ?? 3) - 1));

  const episodes: Episode[] = [];
  let open: {
    triggerId: string | null;
    triggerSeq: number | null;
    turns: EpisodeTurn[];
    failed: EpisodeTurn[];
    referrals: EpisodeTurn[];
    sawOperator: boolean;
  } | null = null;

  const start = (trigger: ChatMessage | null) => ({
    triggerId: trigger?.id ?? null,
    triggerSeq: trigger ? seqOf(trigger) : null,
    turns: [] as EpisodeTurn[],
    failed: [] as EpisodeTurn[],
    referrals: [] as EpisodeTurn[],
    sawOperator: trigger !== null,
  });

  const close = (report: ChatMessage | null, ending: EpisodeEnding | null) => {
    if (!open) return;
    const blindCount = blindRoundLength(open.turns);
    open.turns.forEach((turn, index) => {
      turn.blind = index < blindCount;
    });
    const topics = foldStandings(open.turns, quorum);
    const carried = topics.filter((t) => t.carried);
    // Our own reading, kept only to detect a disagreement with the host's.
    const ourKind =
      open.turns.length === 0
        ? "idle"
        : carried.length === 1
          ? "converged"
          : carried.length > 1
            ? "deadlocked"
            : "exhausted";
    episodes.push({
      key: open.triggerId ?? report?.id ?? `episode-${episodes.length}`,
      triggerId: open.triggerId,
      triggerSeq: open.triggerSeq,
      turns: open.turns,
      topics,
      blindCount,
      ending,
      reportId: report?.id ?? null,
      reportText: report?.text ?? null,
      failed: open.failed,
      referrals: open.referrals,
      quorum,
      quorumDerived: derivedQuorum,
      turnBudget,
      turnBudgetDerived: derivedBudget,
      derived: true,
      disagrees: ending !== null && ending.kind !== ourKind,
      ambiguous: !open.sawOperator,
    });
    open = null;
  };

  for (const message of rows) {
    const isOperator = message.from === "you" || message.byPerson === true;

    if (isOperator) {
      // A second operator line inside an open room is a follow-up asked while
      // the desk was still working. Segmentation cannot tell that from two
      // concurrent episodes, so the open one is closed and flagged rather than
      // silently absorbing turns that may not be its own.
      if (open && open.turns.length > 0) close(null, null);
      open = start(message);
      continue;
    }

    if (message.channel === HIVE_REPORT_AUTHOR) {
      const turn: EpisodeTurn = {
        messageId: message.id,
        seq: seqOf(message),
        agentId: HIVE_REPORT_AUTHOR,
        at: message.at,
        move: null,
        blind: false,
        demoted: null,
        failed: true,
        referral: false,
      };
      if (isFailureNote(message.text)) {
        open ??= start(null);
        open.failed.push(turn);
        continue;
      }
      open ??= start(null);
      close(message, parseEndingReport(message.text));
      continue;
    }

    if (message.from !== "company") continue;

    const move = moveOf(bodyOf(message));
    const referral = message.channel === HIVE_REFERRAL_AUTHOR;
    if (!move && !referral) {
      // Ordinary prose from a teammate. Inside a room that has already
      // produced a real move or referral, it is a turn that deposited no
      // trace. Before that, `open` only records that an operator spoke in a
      // channel that has hosted a room *at some point* — it is not yet
      // evidence that *this* exchange is one, so an ordinary reply must not
      // commit the pending trigger. Without this guard, any plain follow-up
      // question on a desk that has ever deliberated would sprout a
      // misleading "still deliberating" block around its ordinary reply.
      if (!open || open.turns.length === 0) continue;
    }
    open ??= start(null);
    const turn: EpisodeTurn = {
      messageId: message.id,
      seq: seqOf(message),
      agentId: message.channel ?? "",
      at: message.at,
      move,
      blind: false,
      demoted: move ? null : looksDemoted(message.text),
      failed: false,
      referral,
    };
    if (referral) open.referrals.push(turn);
    else open.turns.push(turn);
  }

  // A room still talking has no report yet, and must still render.
  if (open) close(null, null);

  return episodes;
}

/**
 * How many opening turns ran blind.
 *
 * The opening round is one turn per member before anybody speaks twice, so the
 * round ends at the first repeat. This is an **inference** — the flag lives in
 * the host's projection, not in the transcript — and its caller renders it with
 * that uncertainty while {@link Episode.derived} holds.
 */
export function blindRoundLength(turns: EpisodeTurn[]): number {
  const seen = new Set<string>();
  let count = 0;
  for (const turn of turns) {
    if (seen.has(turn.agentId)) break;
    seen.add(turn.agentId);
    count += 1;
  }
  // One speaker is not a round.
  return count > 1 ? count : 0;
}
