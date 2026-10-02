import type { ReactNode } from "react";

import { AgentStatusDot, type AgentStatusSurface } from "@/components/agent-status-dot";
import { cn } from "@/lib/utils";
import { useAgentPresence } from "@/room/store";

/**
 * An agent's avatar with its live state badge, where a surface opts in.
 *
 * The dot is opt-in per surface rather than baked into the avatar: a teammate's
 * face is also drawn on every historical message in a transcript, and a pulsing
 * dot on each of those would be noise about a moment that is long over. A
 * surface that is about the agent *now* (the DM row, the header, the members
 * pane, the profile) wraps its avatar in this and names the agent.
 *
 * `chatId` scopes the state to one conversation, for a surface that is about
 * that conversation (a DM row): an agent busy elsewhere does not light it.
 * Omit it for an agent-wide surface.
 *
 * `decorative` hides the dot from assistive tech, for a surface that says the
 * state in its own words after the agent's name (the DM row), so the name is
 * not announced as "Thinking Ada Lovelace". Where the dot stands alone (a
 * header, a profile) it keeps its `role="img"` label, and `name` puts the agent
 * in it ("Ada Lovelace: Working"), since the state alone does not say who.
 *
 * The wrapper is `relative` and shrink-proof, so it drops into a flex row where
 * the bare avatar was, without changing the layout.
 */
export function AgentFace({
  agentId,
  chatId,
  size = "sm",
  surface = "chrome",
  decorative = false,
  name,
  className,
  children,
}: {
  agentId?: string | null;
  chatId?: string | null;
  size?: "sm" | "md";
  surface?: AgentStatusSurface;
  /** The surface announces the state itself; the dot is hidden from assistive tech. */
  decorative?: boolean;
  /** The agent's display name, for a standalone dot's accessible label. */
  name?: string;
  className?: string;
  /** The avatar to draw. */
  children: ReactNode;
}) {
  const state = useAgentPresence(agentId, chatId);
  return (
    <span className={cn("relative inline-flex shrink-0", className)}>
      {children}
      {agentId && (
        <AgentStatusDot
          state={state}
          size={size}
          surface={surface}
          decorative={decorative}
          name={name}
        />
      )}
    </span>
  );
}
