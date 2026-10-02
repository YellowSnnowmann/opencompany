import { useEffect, useRef, useState } from "react";

import type { RosterAgent } from "@/api/types";
import { AgentAvatarButton } from "@/components/agent-profile-sheet";
import { TeammateAvatar } from "@/components/teammate-avatar";
import { avatarFor } from "@/lib/team";

/**
 * Who can reach a server, as faces rather than a count.
 *
 * The mark is the shipped tiny mascot hashed from the teammate's id, never an
 * uploaded avatar — it must resolve synchronously from a static file, with no
 * fetch per face.
 */
/** One face's own width, and how far the next one is offset into it. */
const FACE = 24;
const PITCH = 18;
/** Room the overflow control needs beside the stack. */
const OVERFLOW_WIDTH = 74;

export function McpReachCell({
  agents,
  serverName,
  onOverflow,
}: {
  agents: RosterAgent[];
  serverName: string;
  /** Opens this server's row, where every teammate is named. */
  onOverflow: () => void;
}) {
  const wrap = useRef<HTMLDivElement | null>(null);
  /**
   * The measured width, or `null` before anything has measured it. `null` shows
   * every face.
   */
  const [width, setWidth] = useState<number | null>(null);

  useEffect(() => {
    const node = wrap.current;
    if (!node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (entry) setWidth(entry.contentRect.width);
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  const fit = fitCount(agents.length, width);
  const shown = agents.slice(0, fit);
  const hidden = agents.length - shown.length;

  return (
    <div ref={wrap} className="flex min-w-0 items-center gap-1.5">
      {agents.length === 0 ? (
        <span className="text-xs text-muted-foreground">no teammate</span>
      ) : (
        <span className="flex shrink-0 items-center">
          {/* The overlap and the ring live on the tile, not on the button:
              `AgentAvatarButton` renders its children bare where no profile
              panel is mounted, and a stack that fell apart there would be the
              component's documented fallback taking the layout with it. */}
          {shown.map((agent) => (
            <AgentAvatarButton
              key={agent.id}
              agentId={agent.id}
              name={agent.name}
              className="shrink-0"
            >
              <TeammateAvatar
                name={agent.name}
                avatar={avatarFor(agent.id)}
                className="-ml-1.5 size-6 ring-2 ring-card"
              />
            </AgentAvatarButton>
          ))}
        </span>
      )}
      {hidden > 0 && (
        <button
          type="button"
          onClick={onOverflow}
          className="shrink-0 rounded-sm text-xs text-muted-foreground underline focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
          aria-label={`Show all ${agents.length} teammates that reach ${serverName}`}
          data-testid="mcp-reach-overflow"
        >
          +{hidden} more
        </button>
      )}
    </div>
  );
}

/**
 * How many faces the cell has room for. An unmeasured cell shows all of them.
 */
export function fitCount(total: number, width: number | null): number {
  if (width === null || width <= 0) return total;
  const all = FACE + PITCH * Math.max(0, total - 1);
  if (all <= width) return total;
  const room = width - OVERFLOW_WIDTH - FACE;
  return Math.max(1, 1 + Math.floor(room / PITCH));
}

/** Every teammate that reaches a server, named. */
export function McpReachChips({ agents }: { agents: RosterAgent[] }) {
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {agents.map((agent) => (
        <AgentAvatarButton
          key={agent.id}
          agentId={agent.id}
          name={agent.name}
          className="rounded-full"
        >
          <span className="flex items-center gap-1.5 rounded-full border border-border py-0.5 pr-2.5 pl-0.5 text-xs text-muted-foreground">
            <TeammateAvatar
              name={agent.name}
              avatar={avatarFor(agent.id)}
              className="size-5"
            />
            {agent.name}
          </span>
        </AgentAvatarButton>
      ))}
    </div>
  );
}
