/**
 * The line that says a company-hive episode is over, and how.
 *
 * A centred pill like the dispatch marker (issue #377), for the same reason:
 * the end of an episode is a structural fact the prose cannot carry. The
 * closing reply reads like any other — this is what tells a reader the desk
 * has stopped, and whether it finished or failed. Driven by
 * `hive_episode_settled` (`lib/hive.ts`); a failure carries the host's reason.
 */

import { CheckCircle2, AlertTriangle } from "lucide-react";

import type { DeskEpisode } from "@/lib/hive";
import { cn } from "@/lib/utils";

interface Props {
  episode: DeskEpisode;
}

/** The pill's words for a settled or failed episode. */
export function describeSettle(episode: Pick<DeskEpisode, "status" | "failure">): string {
  if (episode.status === "failed") {
    return episode.failure ? `Episode failed · ${episode.failure}` : "Episode failed";
  }
  return "Episode complete";
}

export function EpisodeCompleteMarker({ episode }: Props) {
  const failed = episode.status === "failed";
  return (
    <div
      className="my-2 flex justify-center"
      data-testid="episode-complete"
      data-episode-id={episode.id}
      data-episode-status={episode.status}
    >
      <span
        className={cn(
          "inline-flex items-center gap-1.5 rounded-full border px-3 py-1 text-2xs text-muted-foreground",
          failed ? "border-dashed" : "border-status-done/50",
        )}
      >
        {failed ? (
          <AlertTriangle className="size-3 text-status-failed-text" aria-hidden />
        ) : (
          <CheckCircle2 className="size-3 text-status-done-text" aria-hidden />
        )}
        <span>{describeSettle(episode)}</span>
      </span>
    </div>
  );
}
