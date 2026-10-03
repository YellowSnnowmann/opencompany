// Who a conversation is with, without a header bar.
//
// The channel bar across the top of the transcript was removed: the sidebar
// row already names the open conversation, and a full-width bar spent a row of
// height restating it. What it carried — the name, who is in it, and a way to
// more — lives in two quieter pieces instead, the shape Grok's desktop client
// uses: a small pill floating over the top of the transcript (the faces and the
// name), and a panel that slides in on the right when the pill is pressed
// (details and members).

import { ExternalLink, FileCode2, X } from "lucide-react";

import { TeammateAvatar } from "@/components/teammate-avatar";
import type { TeamMember } from "@/lib/team";
import { cn } from "@/lib/utils";
import { ChannelFace, memberFace } from "./ChannelRail";
import { channelSubtitle, channelTitle, dmChannelId, type Channel } from "./model";

/**
 * The pill over the top of the transcript: the conversation's face and name.
 *
 * Centred and floating, translucent over the scrolling messages, and quiet at
 * rest — it is a label first and a control second. Pressing it toggles the info
 * panel; `aria-expanded` says which way.
 */
export function ChannelPill({
  channel,
  members,
  open,
  onToggle,
}: {
  channel: Channel;
  members: TeamMember[];
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={open}
      aria-controls="channel-info-panel"
      title={open ? "Hide details" : "Show details"}
      data-testid="channel-pill"
      className={cn(
        "absolute top-2 left-1/2 z-20 flex max-w-[60%] -translate-x-1/2 items-center gap-2 rounded-full border border-foreground/10 bg-page/80 py-1 pr-3 pl-1 text-sm font-medium shadow-sm backdrop-blur-md transition-colors",
        "hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none",
        open && "bg-muted",
      )}
    >
      <ChannelFace channel={channel} members={members} size="pill" />
      <span className="min-w-0 truncate">{channelTitle(channel)}</span>
    </button>
  );
}

/**
 * The info panel on the right: the conversation's face, name and purpose, and
 * who is in it. A member row opens a direct message with them; a desk also
 * links to its place on the org chart, where membership is managed now.
 */
export function ChannelInfoPanel({
  channel,
  members,
  channelMembers,
  onClose,
  onMessage,
  rawHref,
}: {
  channel: Channel;
  /** The whole roster, for the head's face. */
  members: TeamMember[];
  /** Who is in this conversation, lead first; `null` when it has no membership of its own. */
  channelMembers: TeamMember[] | null;
  onClose: () => void;
  /** Open a DM with a teammate. */
  onMessage: (channelId: string) => void;
  /** The raw-turns view of this DM (`?raw`), or `undefined` where there is none. */
  rawHref?: string;
}) {
  const purpose = channelSubtitle(channel);
  const list = channel.kind === "dm" ? (channel.member ? [channel.member] : []) : (channelMembers ?? members);
  return (
    <aside
      id="channel-info-panel"
      aria-label={`${channelTitle(channel)} details`}
      data-testid="channel-info-panel"
      className="flex w-80 shrink-0 flex-col border-l border-foreground/10 bg-page"
    >
      <div className="flex justify-end p-2">
        <button
          type="button"
          onClick={onClose}
          aria-label="Close details"
          title="Close details"
          className="inline-flex size-8 items-center justify-center rounded-lg text-muted-foreground transition hover:bg-rail-hover hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        >
          <X className="size-4" aria-hidden />
        </button>
      </div>
      <div className="flex flex-col items-center gap-3 px-6 pb-6 text-center">
        <ChannelFace channel={channel} members={members} size="hero" />
        <div className="min-w-0">
          <h2 className="truncate text-lg font-semibold tracking-tight">{channelTitle(channel)}</h2>
          {purpose && <p className="mt-1 text-sm text-muted-foreground">{purpose}</p>}
        </div>
        <div className="flex flex-wrap justify-center gap-2">
          {channel.kind === "channel" && channel.memberIds && (
            <a
              href={`#/company/${encodeURIComponent(channel.id)}`}
              className="inline-flex items-center gap-1.5 rounded-full border px-3 py-1 text-xs text-muted-foreground transition hover:bg-rail-hover hover:text-foreground"
            >
              <ExternalLink className="size-3.5" aria-hidden /> Manage desk
            </a>
          )}
          {rawHref && (
            <a
              href={rawHref}
              className="inline-flex items-center gap-1.5 rounded-full border px-3 py-1 text-xs text-muted-foreground transition hover:bg-rail-hover hover:text-foreground"
            >
              <FileCode2 className="size-3.5" aria-hidden /> Raw turns
            </a>
          )}
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto border-t border-foreground/10 px-3 py-3">
        <div className="px-2 pb-2 text-xs font-medium text-muted-foreground">
          {channel.kind === "dm" ? "With" : `Members · ${list.length}`}
        </div>
        <ul className="flex flex-col gap-px">
          {list.map((m, i) => (
            <li key={m.id}>
              <button
                type="button"
                onClick={() => onMessage(dmChannelId(m))}
                title={`Message ${m.name}`}
                className="flex w-full items-center gap-3 rounded-lg px-2 py-2 text-left transition-colors hover:bg-rail-hover"
              >
                <TeammateAvatar {...memberFace(m)} className="size-8 bg-avatar-disc text-2xs" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium">{m.name}</span>
                  {m.role && (
                    <span className="block truncate text-xs text-muted-foreground">{m.role}</span>
                  )}
                </span>
                {channel.kind === "channel" && i === 0 && !channel.leadless && channel.memberIds && (
                  <span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-3xs font-medium text-muted-foreground">
                    Lead
                  </span>
                )}
              </button>
            </li>
          ))}
        </ul>
      </div>
    </aside>
  );
}
