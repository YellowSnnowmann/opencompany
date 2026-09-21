/**
 * The one speech act a seat ended its turn with, on the row it produced.
 *
 * Every turn inside an episode ends with exactly one of four calls on the
 * `opencompany` MCP server — `post`, `broadcast`, `dm`, `complete_episode` —
 * and the row is the utterance. Rendered verbatim it is prose like any other
 * reply, so a reader cannot tell a line the whole desk heard from one that
 * went to a single teammate, or the line that ended the episode from the one
 * before it. The chip carries that difference, and the prose carries the
 * argument.
 *
 * Told apart by icon and word, never by colour alone.
 */

import { CheckCircle2, MessageSquare, Radio, Send } from "lucide-react";

import type { MessageEpisodeDto, UtteranceKind } from "@/api/types";
import { RoutingPlanChip } from "@/components/episode/RoutingPlanChip";
import { cn } from "@/lib/utils";

interface Props {
  episode: MessageEpisodeDto;
  /** Who may read the row, when the host narrowed it — drawn as `→ @x`. */
  audience?: string[];
  agentNames?: Readonly<Record<string, string>>;
  className?: string;
}

/** The chip's word for each kind. */
export const UTTERANCE_LABEL: Record<UtteranceKind, string> = {
  post: "post",
  broadcast: "broadcast",
  dm: "dm",
  complete_episode: "complete",
};

const ICON: Record<UtteranceKind, typeof MessageSquare> = {
  post: MessageSquare,
  broadcast: Radio,
  dm: Send,
  complete_episode: CheckCircle2,
};

export function UtteranceChip({ episode, audience, agentNames, className }: Props) {
  const Icon = ICON[episode.kind] ?? MessageSquare;
  const name = (id: string) => agentNames?.[id] ?? id;
  // A dm names its recipients; the audience is the same list when the host
  // narrowed the row, so the recipients are shown once, from whichever the
  // host filled in.
  const to = episode.to?.length ? episode.to : episode.kind === "dm" ? audience : undefined;
  return (
    <span
      className={cn("mt-1 flex flex-wrap items-center gap-1.5", className)}
      data-testid="utterance-chip"
      data-kind={episode.kind}
      data-episode-id={episode.id}
      data-round-revision={episode.revision}
    >
      <span
        className={cn(
          "inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-2xs font-medium",
          episode.kind === "complete_episode"
            ? "border-status-done/50 text-foreground"
            : "text-muted-foreground",
        )}
        title={`round ${episode.revision + 1} of episode ${episode.id}`}
      >
        <Icon className="size-3 shrink-0" aria-hidden />
        {UTTERANCE_LABEL[episode.kind] ?? episode.kind}
        {to?.length ? (
          <span className="font-normal" data-testid="utterance-audience">
            → {to.map((id) => `@${name(id)}`).join(", ")}
          </span>
        ) : null}
      </span>
      {episode.routedBy && (
        <RoutingPlanChip
          plan={episode.routedBy.plan}
          router={episode.routedBy.router}
          agentNames={agentNames}
        />
      )}
    </span>
  );
}
