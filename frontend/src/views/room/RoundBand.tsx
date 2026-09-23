/**
 * One round of an episode: the seats that ran together, what each one is
 * doing, and the rows they produced.
 *
 * # A grouping strip, not a page
 *
 * A round is drawn *around* its rows. The rows are ordinary messages — same
 * avatar gutter, same hover actions, same thread affordances — and are rendered
 * by the timeline's own `renderRow`, so a second renderer cannot drift from
 * the first. What the band adds is what a flat list cannot show: that these
 * three replies were written **at the same time**, by seats the router picked
 * together, and that a fourth seat is still thinking.
 *
 * # The lanes
 *
 * One lane per seat, in the host's order, each with its live state: waiting,
 * working (the only pulse on the band), committed with its utterance kind, or
 * one of the three ways a turn ends without one. The lanes are what make
 * concurrency *visible* — a round of two working seats is two pulsing lanes,
 * and a relay race would be one.
 *
 * `data-round-status` and `data-seat-status` are the contract the live spec
 * reads (`test/e2e/desk-episode-live.spec.ts`).
 */

import type { ReactNode } from "react";
import { AlertTriangle, CheckCircle2, Clock, Loader2, MinusCircle } from "lucide-react";

import { ConversationChip } from "@/components/episode/ConversationChip";
import { RoutingPlanChip } from "@/components/episode/RoutingPlanChip";
import { UTTERANCE_LABEL } from "@/components/episode/UtteranceChip";
import { TeammateAvatar } from "@/components/teammate-avatar";
import type { Episode, EpisodeRound, EpisodeSeat, SeatStatus } from "@/lib/episodes";
import { cn } from "@/lib/utils";
import type { TimelineItem } from "@/views/room/timeline";

interface Props {
  episode: Episode;
  round: EpisodeRound;
  /** The rows this round produced, already in transcript order. */
  items: TimelineItem[];
  renderRow: (item: TimelineItem) => ReactNode;
  agentNames?: Readonly<Record<string, string>>;
}

/** The lane's word for each seat state — never colour alone. */
const SEAT_WORD: Record<SeatStatus, string> = {
  waiting: "waiting",
  working: "working",
  committed: "done",
  failed: "failed",
  timed_out: "timed out",
  no_utterance: "said nothing",
};

function SeatIcon({ status }: { status: SeatStatus }) {
  const className = "size-3 shrink-0";
  switch (status) {
    case "working":
      return <Loader2 className={cn(className, "animate-spin text-status-running-text")} aria-hidden />;
    case "committed":
      return <CheckCircle2 className={cn(className, "text-status-done-text")} aria-hidden />;
    case "failed":
    case "timed_out":
      return <AlertTriangle className={cn(className, "text-status-failed-text")} aria-hidden />;
    case "no_utterance":
      return <MinusCircle className={cn(className, "text-muted-foreground")} aria-hidden />;
    default:
      return <Clock className={cn(className, "text-muted-foreground")} aria-hidden />;
  }
}

function SeatLane({ seat, agentNames }: { seat: EpisodeSeat; agentNames?: Readonly<Record<string, string>> }) {
  const name = agentNames?.[seat.agentId] ?? seat.agentId;
  return (
    <li
      className={cn(
        "flex items-center gap-1.5 rounded-md border px-1.5 py-0.5 text-2xs",
        seat.status === "working" && "border-status-running/50 bg-status-running-soft",
      )}
      data-testid="round-seat"
      data-agent-id={seat.agentId}
      data-seat-status={seat.status}
      title={`${name}: ${SEAT_WORD[seat.status]}`}
    >
      <TeammateAvatar name={seat.agentId} className="size-4 shrink-0" markOnly />
      <span className="max-w-24 truncate font-medium">{name}</span>
      <SeatIcon status={seat.status} />
      <span className="text-muted-foreground">
        {seat.status === "committed" && seat.utterance
          ? UTTERANCE_LABEL[seat.utterance.kind] ?? seat.utterance.kind
          : SEAT_WORD[seat.status]}
      </span>
    </li>
  );
}

export function RoundBand({ episode, round, items, renderRow, agentNames }: Props) {
  const done = round.seats.filter((seat) => seat.status !== "waiting" && seat.status !== "working").length;
  const first = episode.rounds[0]?.revision === round.revision;
  return (
    <section
      className={cn(
        "my-2 rounded-lg border",
        round.status === "open" ? "border-status-running/40" : "border-dashed",
      )}
      data-testid="round-band"
      data-episode-id={episode.id}
      data-round-revision={round.revision}
      data-round-status={round.status}
      aria-label={`Round ${round.revision + 1}${round.status === "open" ? ", running" : ""}`}
    >
      <header className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b px-3 py-1.5 text-2xs text-muted-foreground">
        <span className="font-medium text-foreground">Round {round.revision + 1}</span>
        <span>
          {done}/{round.seats.length} seat{round.seats.length === 1 ? "" : "s"}
        </span>
        {round.status === "open" ? (
          <span className="inline-flex items-center gap-1 text-status-running-text" data-testid="round-running">
            <Loader2 className="size-3 animate-spin" aria-hidden />
            running together
          </span>
        ) : (
          <span>committed</span>
        )}
        {first && episode.plan && <RoutingPlanChip plan={episode.plan} agentNames={agentNames} />}
        {/* The private exchanges this episode opened. On the first band only:
            a conversation belongs to the episode rather than to a round, and
            repeating it under every round would read as several. */}
        {first &&
          episode.conversations.map((conversation) => (
            <ConversationChip
              key={conversation.root}
              conversation={conversation}
              agentNames={agentNames}
            />
          ))}
        {episode.referrals.filter((r) => !r.returning).map((referral) => (
          <span
            key={`${referral.toDesk}:${referral.sequence}`}
            className="rounded-full border border-dashed px-2 py-0.5"
            data-testid="round-referral"
          >
            asked {referral.direct ? `@${agentNames?.[referral.target] ?? referral.target}` : `#${referral.toDesk}`}
          </span>
        ))}
      </header>
      {round.seats.length > 0 && (
        <ul className="flex flex-wrap gap-1.5 px-3 py-1.5" data-testid="round-lanes">
          {round.seats.map((seat) => (
            <SeatLane key={seat.agentId} seat={seat} agentNames={agentNames} />
          ))}
        </ul>
      )}
      {items.length > 0 && <div className="pb-1">{items.map(renderRow)}</div>}
    </section>
  );
}
