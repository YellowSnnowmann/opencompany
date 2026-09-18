import { useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Users } from "lucide-react";

import { BlindRoundBand } from "@/components/hive/BlindRoundBand";
import { StandingsRail } from "@/components/hive/StandingsRail";
import { VerdictCard } from "@/components/hive/VerdictCard";

import { referralWorkingLabel } from "./channels";
import { useDeskCrossing } from "./referral-running";
import { cn } from "@/lib/utils";
import type { TimelineItem } from "@/views/room/model";

/**
 * One room, in the transcript.
 *
 * A desk of two or more does not answer with a reply — it opens an **episode**
 * and runs a bounded sequence of single turns until it converges, deadlocks,
 * spends its budget, or finds it has nothing to say. Rendered as a flat list
 * those turns are indistinguishable from three agents talking, which is what the
 * console did before this block existed and is precisely the thing an operator
 * cannot afford to misread: three agents agreeing in parallel is not
 * deliberation.
 *
 * So the block draws the three facts a flat list cannot carry:
 *
 * - **which turns were blind**, because independence is the property a room
 *   actually buys over a single responder;
 * - **what the turns added up to**, so "is this decided yet" does not require
 *   holding six citations in your head;
 * - **how it ended**, in the desk's own words.
 *
 * The closing report is rendered *only* as the verdict card. It is excluded from
 * the row list rather than shown twice — as a grey system line and again as the
 * verdict — which is what it looked like before the exclusion.
 */
export function EpisodeBlock({
  item,
  renderRow,
  onSelectTopic,
  deskId,
  agentNames,
}: {
  item: Extract<TimelineItem, { kind: "episode" }>;
  renderRow: (row: TimelineItem) => ReactNode;
  onSelectTopic?: (topic: string) => void;
  /**
   * The desk this room sat on, so the block can offer its grammar.
   *
   * This is where an operator learns the table is wrong — a seat demoted for a
   * move it does not hold, or a room that spent its budget with three seats
   * unable to support anything — so it is where the way to fix it belongs.
   */
  deskId?: string;
  /** Display names, so a crossing can be described by who rather than by id. */
  agentNames?: Record<string, string>;
}) {
  const { episode } = item;
  // What this desk is waiting on, when it is waiting on somebody rather than
  // talking to itself.
  const crossing = useDeskCrossing(deskId);
  const nameOf = (id: string) => agentNames?.[id] ?? id;
  const [open, setOpen] = useState(true);

  // The rows that carried the blind opening round, by message id — the block
  // splits on identity rather than on position, because an approval card or a
  // failed-turn note can sit between two turns without being one.
  const blindIds = new Set(
    episode.turns.slice(0, episode.blindCount).map((turn) => turn.messageId),
  );

  const rows = item.items.filter(
    (row) => !(row.kind === "message" && row.entry.message.id === episode.reportId),
  );
  const blindRows = rows.filter(
    (row) => row.kind === "message" && blindIds.has(row.entry.message.id),
  );
  const restRows = rows.filter((row) => !blindRows.includes(row));

  const speakers = new Set(episode.turns.map((turn) => turn.agentId));
  // No closing report yet, so the room has not finished. Read off the ending
  // rather than a live flag: the transcript is the episode, and an episode with
  // no `hive-report` row in it is one still in progress — which is also true
  // after a reload, when no live frame is coming.
  const running = episode.ending === null;

  return (
    <section
      className="my-2 rounded-lg border border-border bg-muted/20 p-3"
      aria-label="Desk deliberation"
    >
      <header className="flex items-center gap-2 pb-2">
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground hover:text-foreground"
          aria-expanded={open}
        >
          {open ? (
            <ChevronDown aria-hidden className="size-3.5" />
          ) : (
            <ChevronRight aria-hidden className="size-3.5" />
          )}
          <Users aria-hidden className="size-3.5" />
          {/* Present tense while the room is still talking. */}
          {running ? "The desk is deliberating" : "The desk deliberated"}
        </button>
        {/*
          Turns against the budget, not a bare count.
          
          A room that is still talking is the case this is for: "6 turns" tells a
          reader nothing about whether the desk is a third of the way through or
          one turn from spending its budget, so `Exhausted` arrives as a surprise.
          The budget is the only bound on how long a room can run, and an
          operator watching one work is watching that number.
          
          Once the room has closed the count is the fact and the budget is
          noise, so the denominator drops away.
        */}
        <span className="text-2xs text-muted-foreground">
          {running
            ? `turn ${episode.turns.length} of ${episode.turnBudget}`
            : `${episode.turns.length} ${episode.turns.length === 1 ? "turn" : "turns"}`}{" "}
          · {speakers.size} {speakers.size === 1 ? "seat" : "seats"}
          {running && episode.turnBudgetDerived ? " · budget derived" : ""}
        </span>
        {running ? (
          <span
            className="inline-flex items-center gap-1 rounded-full bg-status-running-soft px-1.5 py-0.5 text-3xs font-medium text-status-running-text"
            title={
              crossing
                ? "This desk is waiting on a question it put to someone else."
                : "This desk is still deliberating."
            }
          >
            <span className="size-1.5 animate-pulse rounded-full bg-status-running" />
            {/* **What the room is actually doing, when it is not its own turn.**

                A referred turn runs outside the `turn_started`/`turn_settled`
                bracket every other turn is announced by, so while a crossing
                ran the header said "turn 3 of 12 · deliberating" and nothing
                else — and with `pair_messages` raised that is several model
                turns of apparent silence. The room is not deliberating then; it
                is waiting on somebody.

                A desk crossing names the DESK, never the seat the host resolved
                it to: `@#returns` resolves to that desk's first eligible member
                while the whole room answers, so naming the seat would credit one
                member with a room's work. */}
            {crossing ? referralWorkingLabel(crossing, nameOf) : "deliberating"}
          </span>
        ) : null}
        {deskId ? (
          <a
            href={`#/company/${encodeURIComponent(deskId)}?hive`}
            className="ml-auto text-2xs text-muted-foreground underline decoration-dotted hover:text-foreground"
          >
            Move grammar
          </a>
        ) : null}
      </header>

      {open && (
        <div className="space-y-1">
          {blindRows.length > 0 && (
            <BlindRoundBand derived={episode.derived}>
              <div className="space-y-1">{blindRows.map(renderRow)}</div>
            </BlindRoundBand>
          )}
          {restRows.map(renderRow)}
        </div>
      )}

      <div className="mt-3 grid gap-3 lg:grid-cols-2">
        <StandingsRail episode={episode} onSelectTopic={onSelectTopic} />
        {/*
          Always rendered, even mid-episode: a room still talking says so, and an
          operator watching one work needs the turn count and the standings more
          than they need the block to stay quiet until it is over.
        */}
        <VerdictCard
          episode={episode}
          onSelectTopic={onSelectTopic}
          className={cn(episode.topics.length === 0 && "lg:col-span-2")}
        />
      </div>
    </section>
  );
}
