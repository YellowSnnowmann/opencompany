/**
 * The rows one company-hive episode produced on a desk, drawn as one block.
 *
 * A grouping strip, not a page: the rows are ordinary messages rendered by the
 * timeline's own `renderRow`, so a second renderer cannot drift from the
 * first. All the block adds is that these replies answered **one** question
 * (they share `hive.episodeId`), and — while the episode is open — a faint
 * running edge. `data-episode-status` is what the live spec reads.
 */

import type { ReactNode } from "react";

import type { DeskEpisode } from "@/lib/hive";
import { cn } from "@/lib/utils";
import type { TimelineItem } from "./timeline";

interface Props {
  episode: DeskEpisode;
  items: TimelineItem[];
  renderRow: (item: TimelineItem) => ReactNode;
}

export function EpisodeGroup({ episode, items, renderRow }: Props) {
  return (
    <section
      className={cn(
        "my-1 border-l-2 pl-1",
        episode.status === "open" ? "border-status-running/40" : "border-border",
      )}
      data-testid="episode-group"
      data-episode-id={episode.id}
      data-episode-status={episode.status}
    >
      {items.map(renderRow)}
    </section>
  );
}
