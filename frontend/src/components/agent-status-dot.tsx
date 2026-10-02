import type { CSSProperties } from "react";

import type { AgentPresenceState } from "@/lib/agent-presence";
import { cn } from "@/lib/utils";

/**
 * The words each state announces and shows on hover. `inactive` has none: it
 * draws nothing at all.
 */
const LABEL: Record<Exclude<AgentPresenceState, "inactive">, string> = {
  approval: "Waiting for your approval",
  working: "Working",
  typing: "Typing",
  thinking: "Thinking",
  queued: "Queued",
};

/** The words a state is announced by, or `null` for `inactive` (which draws nothing). */
export function agentPresenceLabel(state: AgentPresenceState): string | null {
  return state === "inactive" ? null : LABEL[state];
}

/** The surface the dot sits on, so its cut-out ring matches what is behind it. */
export type AgentStatusSurface = "chrome" | "card" | "popover" | "background";

const SURFACE: Record<AgentStatusSurface, string> = {
  chrome: "bg-chrome ring-chrome",
  card: "bg-card ring-card",
  popover: "bg-popover ring-popover",
  background: "bg-background ring-background",
};

const SIZE = { sm: "size-2", md: "size-2.5" } as const;

/**
 * An agent's live state, drawn as a small badge on its avatar.
 *
 * Presentational only: it takes the state and draws it, and draws **nothing**
 * for `inactive` (no open turn, no timer: an absence, not an "offline" claim).
 * `PresenceDot` stays a person's (online/away) and is never rendered for a
 * teammate; this one is never rendered for a person. The two answer different
 * questions and must not share a testid (`chat-presence.spec.ts` counts
 * `presence-dot` against the person rows).
 *
 * Colour is never the only cue (`color.md`), and working / typing / thinking
 * share `status-running`, so each has its own shape that survives reduced
 * motion, where the global rule stills the animation:
 *
 * | state    | colour           | shape                          |
 * | -------- | ---------------- | ------------------------------ |
 * | approval | status-blocked   | filled, with an exclamation bar |
 * | working  | status-running   | arc with a solid centre dot    |
 * | typing   | status-running   | wide pill of three dots        |
 * | thinking | status-running   | closed ring, pulsing           |
 * | queued   | status-idle      | plain filled dot               |
 *
 * Working and thinking were first both a ring, told apart by the arc's gap
 * alone, which at 8px under reduced motion is a pixel or two. Working now
 * carries a solid centre dot, so the still shapes read as a hollow ring
 * (thinking) against a target (working), not a ring against a nearly-ring.
 *
 * `decorative` hides it from assistive tech (no role, no label) for a surface
 * that announces the state in its own words after the agent's name.
 *
 * `name` is for a dot that stands alone beside a face (a header, a card, the
 * members pane): "Working" on its own does not say who, so the label becomes
 * "Ada Lovelace: Working". The hover title stays the bare state, since the
 * pointer is already on that agent's face.
 *
 * Positioned at the bottom-right of a `relative` parent, which is what
 * `AgentFace` provides. It is a sibling of the avatar tile, never inside it:
 * `TeammateAvatar` clips with `overflow-hidden`.
 */
export function AgentStatusDot({
  state,
  size = "sm",
  surface = "chrome",
  decorative = false,
  name,
  className,
}: {
  state: AgentPresenceState;
  size?: keyof typeof SIZE;
  /**
   * What the dot's cut-out ring blends into: `chrome` on the app sidebar's
   * rows, `card` on cards, `popover` in a floating menu, `background` on the
   * page or a sheet.
   */
  surface?: AgentStatusSurface;
  /** The surrounding control says the state itself; hide this from assistive tech. */
  decorative?: boolean;
  /** Who the dot is about, for its accessible label where nothing else says it. */
  name?: string;
  className?: string;
}) {
  if (state === "inactive") return null;
  const label = LABEL[state];
  return (
    <span
      role={decorative ? undefined : "img"}
      aria-label={decorative ? undefined : name ? `${name}: ${label}` : label}
      aria-hidden={decorative ? true : undefined}
      title={label}
      data-testid="agent-status-dot"
      data-state={state}
      className={cn(
        "absolute -bottom-0.5 -right-0.5 flex items-center justify-center rounded-full ring-2",
        state === "typing" ? "h-2 w-3.5" : SIZE[size],
        SURFACE[surface],
        className,
      )}
    >
      {state === "approval" && (
        <span className="flex size-full items-center justify-center rounded-full bg-status-blocked">
          <svg viewBox="0 0 10 10" className="size-full text-background" aria-hidden>
            <path d="M5 2.2v3.4M5 7.4v.4" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" fill="none" />
          </svg>
        </span>
      )}
      {state === "working" && (
        <span className="relative size-full">
          <span className="absolute inset-0 animate-spin rounded-full border-2 border-status-running border-t-transparent" />
          <span className="absolute inset-0 m-auto size-[35%] rounded-full bg-status-running" />
        </span>
      )}
      {state === "thinking" && (
        <span className="size-full animate-pulse rounded-full border-2 border-status-running" />
      )}
      {state === "queued" && <span className="size-full rounded-full bg-status-idle" />}
      {state === "typing" && (
        <span className="flex size-full items-center justify-center gap-px rounded-full bg-status-running">
          {[0, 1, 2].map((i) => (
            <span
              key={i}
              className="size-0.5 animate-bounce rounded-full bg-background"
              style={{ animationDelay: `${i * 120}ms` } satisfies CSSProperties}
            />
          ))}
        </span>
      )}
    </span>
  );
}
