import { cn } from "@/lib/utils";
import type { PresenceStatus } from "@/lib/awareness";

/**
 * A person's live status.
 *
 * Three states and an absence, and the absence is the interesting one: no dot
 * at all means *no live signal*, not "offline". Presence is replica-local, so
 * somebody connected to another host of the same tenant is simply unknown here
 * — and drawing them as offline would be a confident claim the console cannot
 * support. A hollow ring says "not seen" honestly.
 *
 * Never rendered for a teammate. An agent is not "online": it has no session
 * and no machine to be at. Its live state (typing, thinking, working, waiting
 * for approval, queued) is `AgentStatusDot`'s, under a different testid, so
 * `chat-presence.spec.ts` can keep counting this one against the person rows.
 */
export function PresenceDot({
  status,
  className,
}: {
  status: PresenceStatus | undefined;
  className?: string;
}) {
  const label =
    status === "online" ? "Online" : status === "away" ? "Away" : "No recent activity";
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      data-testid="presence-dot"
      data-status={status ?? "unknown"}
      className={cn(
        "inline-block size-2 shrink-0 rounded-full",
        status === "online" && "bg-status-running",
        status === "away" && "bg-status-idle",
        // Not seen: a ring rather than a fill, so it reads as "unknown" instead
        // of as a third status.
        !status && "border border-muted-foreground/40",
        className,
      )}
    />
  );
}
