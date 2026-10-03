// Who a conversation is with, without a header bar.
//
// The channel bar across the top of the transcript was removed: the sidebar
// row already names the open conversation, and a full-width bar spent a row of
// height restating it. What it carried — the name, who is in it, and a way to
// more — lives in two quieter pieces instead, the shape Grok's desktop client
// uses: a small pill floating over the top of the transcript (the faces and the
// name), and a panel that slides in on the right when the pill is pressed
// (details and members).

import { useState, type ReactNode } from "react";
import { Check, ChevronRight, Copy, ExternalLink, FileCode2, Plus, X } from "lucide-react";

import { AgentFace } from "@/components/agent-face";
import { AgentAvatarButton } from "@/components/agent-profile-sheet";
import { TeammateAvatar } from "@/components/teammate-avatar";
import { Skeleton } from "@/components/ui/skeleton";
import type { PresenceStatus } from "@/lib/awareness";
import { roleSubtitle, type TeamMember } from "@/lib/team";
import { cn } from "@/lib/utils";
import { ChannelFace, memberFace } from "./ChannelRail";
import { channelSubtitle, channelTitle, dmChannelId, type Channel } from "./model";
import { PresenceDot } from "./PresenceDot";

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
        "group/pill absolute top-2 left-1/2 z-20 flex max-w-[60%] -translate-x-1/2 items-center gap-2 rounded-full border border-foreground/10 bg-page/80 py-1 pr-3 pl-1 text-sm font-medium shadow-sm backdrop-blur-md transition-colors",
        "hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none",
        open && "bg-muted",
      )}
    >
      <ChannelFace channel={channel} members={members} size="pill" />
      <span className="min-w-0 truncate">{channelTitle(channel)}</span>
      {/* On hover (or keyboard focus), a right arrow slides in: the panel
          this opens is on the right. Zero width at rest, so the label stays
          centred and quiet until the pointer asks. */}
      <ChevronRight
        aria-hidden
        className="-ml-2 size-3.5 w-0 shrink-0 text-muted-foreground opacity-0 transition-all group-hover/pill:ml-0 group-hover/pill:w-3.5 group-hover/pill:opacity-100 group-focus-visible/pill:ml-0 group-focus-visible/pill:w-3.5 group-focus-visible/pill:opacity-100"
      />
    </button>
  );
}

/**
 * The info panel on the right — everything the channel bar and its members
 * pane used to carry, in one place.
 *
 * The head: the conversation's face, its name (with a copy control), its
 * purpose and a count of who is in it, then the actions — "Manage desk" on the
 * org chart for a desk, the raw-turns toggle for a DM. Below: the members (live
 * status on each face, the face opens their profile, the row opens a DM, the
 * lead marked), everyone else in the company with a `+` to add them to this
 * desk, and the people signed in to it with their online dots.
 */
export function ChannelInfoPanel({
  channel,
  members,
  channelMembers,
  others = [],
  people,
  presence,
  loading = false,
  leadId,
  onAddExisting,
  onClose,
  onMessage,
  raw,
}: {
  channel: Channel;
  /** The whole roster, for the head's face and as the list where a channel has no membership of its own. */
  members: TeamMember[];
  /** Who is in this conversation, lead first; `null` when it has no membership of its own. */
  channelMembers: TeamMember[] | null;
  /** Roster teammates not in it — "Everyone else". */
  others?: TeamMember[];
  /** The company's signed-in people. */
  people?: Array<{ id: string; label: string }>;
  /** Who is online, keyed by user id, for the People list's dots. */
  presence?: ReadonlyMap<string, { status: PresenceStatus }>;
  /** The roster is still loading. */
  loading?: boolean;
  /** The desk's lead, badged — absent for a leaderless (`auto`) desk and a DM. */
  leadId?: string;
  /** Add a roster teammate to this desk; absent where membership cannot change. */
  onAddExisting?: (agentId: string) => void;
  onClose: () => void;
  /** Open a DM with a teammate. */
  onMessage: (channelId: string) => void;
  /**
   * The raw-turns view of this DM (`?raw`) as a toggle — whether it is on, and
   * how to flip it — or `undefined` where there is none (a channel has several
   * agents, so "the raw turns" would have to pick one).
   */
  raw?: { on: boolean; onToggle: () => void };
}) {
  const purpose = channelSubtitle(channel);
  const isDm = channel.kind === "dm";
  const inside = isDm ? (channel.member ? [channel.member] : []) : (channelMembers ?? members);
  const total = inside.length + (channelMembers ? others.length : 0);
  const summary = isDm
    ? "Direct message"
    : channelMembers
      ? `${inside.length} in this channel · ${total} in the company`
      : `${inside.length} ${inside.length === 1 ? "agent" : "agents"}`;
  const manageHref =
    channel.kind === "channel" && channel.memberIds
      ? `#/company/${encodeURIComponent(channel.id)}`
      : undefined;

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
          className={ICON_BUTTON}
        >
          <X className="size-4" aria-hidden />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex flex-col items-center gap-3 px-6 pb-5 text-center">
          <ChannelFace channel={channel} members={members} size="hero" />
          <div className="min-w-0">
            <div className="flex items-center justify-center gap-1">
              <h2 className="min-w-0 truncate text-lg font-semibold tracking-tight">
                {channelTitle(channel)}
              </h2>
              <CopyName title={channelTitle(channel)} />
            </div>
            {purpose && <p className="mt-1 text-sm text-muted-foreground">{purpose}</p>}
            <p className="mt-1 text-xs text-muted-foreground" data-testid="channel-info-summary">
              {loading ? "Loading…" : summary}
            </p>
          </div>
          {(manageHref || raw) && (
            <div className="flex flex-wrap justify-center gap-2">
              {manageHref && (
                <a href={manageHref} className={CHIP}>
                  <ExternalLink className="size-3.5" aria-hidden /> Manage desk
                </a>
              )}
              {raw && (
                // A toggle, not a link: pressed while the transcript shows the
                // raw turns, and pressing it again goes back to the conversation.
                <button
                  type="button"
                  onClick={raw.onToggle}
                  aria-pressed={raw.on}
                  data-testid="channel-info-raw-toggle"
                  title={raw.on ? "Back to the conversation" : "Show the raw turns"}
                  className={cn(
                    CHIP,
                    raw.on &&
                      "border-primary bg-primary text-primary-foreground hover:bg-primary/90 hover:text-primary-foreground",
                  )}
                >
                  <FileCode2 className="size-3.5" aria-hidden /> Raw turns
                </button>
              )}
            </div>
          )}
        </div>

        <Section label={isDm ? "With" : channelMembers ? "In this channel" : "Members"}>
          {loading ? (
            <div className="space-y-2 px-2">
              {Array.from({ length: 4 }).map((_, i) => (
                <Skeleton key={i} className="h-11 rounded-lg" />
              ))}
            </div>
          ) : inside.length > 0 ? (
            <MemberList list={inside} leadId={leadId} onMessage={onMessage} />
          ) : (
            <p className="px-2 py-1.5 text-xs text-muted-foreground">
              Nobody is on this desk yet.
              {manageHref && (
                <>
                  {" "}
                  <a href={manageHref} className="underline hover:text-foreground">
                    Staff it on the org chart
                  </a>
                </>
              )}
            </p>
          )}
        </Section>

        {!isDm && channelMembers && others.length > 0 && !loading && (
          <Section label="Everyone else">
            <MemberList list={others} onMessage={onMessage} onAdd={onAddExisting} />
          </Section>
        )}

        {!isDm && people && people.length > 0 && (
          <Section label="People">
            <ul className="flex flex-col gap-px">
              {people.map((person) => (
                <li
                  key={person.id}
                  data-testid="person-row"
                  className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm"
                >
                  <PresenceDot status={presence?.get(person.id)?.status} />
                  <span className="min-w-0 flex-1 truncate">{person.label}</span>
                </li>
              ))}
            </ul>
          </Section>
        )}
      </div>
    </aside>
  );
}

const ICON_BUTTON =
  "inline-flex size-8 items-center justify-center rounded-lg text-muted-foreground transition hover:bg-rail-hover hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none";

const CHIP =
  "inline-flex items-center gap-1.5 rounded-full border px-3 py-1 text-xs text-muted-foreground transition hover:bg-rail-hover hover:text-foreground";

function Section({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section className="border-t border-foreground/10 px-3 py-3">
      <h3 className="px-2 pb-2 text-xs font-medium text-muted-foreground">{label}</h3>
      {children}
    </section>
  );
}

/** Copies the conversation's title, and says so for a moment. */
function CopyName({ title }: { title: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      onClick={() => {
        void navigator.clipboard?.writeText(title).then(() => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1500);
        });
      }}
      aria-label={`Copy channel name: ${title}`}
      title={copied ? "Copied" : "Copy name"}
      className={cn(ICON_BUTTON, "size-7 shrink-0")}
    >
      {copied ? <Check className="size-3.5" aria-hidden /> : <Copy className="size-3.5" aria-hidden />}
    </button>
  );
}

/**
 * Teammate rows. The face opens who they are (their profile sheet) and wears
 * their live status; the rest of the row opens a DM. With `onAdd`, a `+` on
 * hover adds them to this desk.
 */
function MemberList({
  list,
  leadId,
  onMessage,
  onAdd,
}: {
  list: TeamMember[];
  leadId?: string;
  onMessage: (channelId: string) => void;
  onAdd?: (agentId: string) => void;
}) {
  return (
    <ul className="flex flex-col gap-px">
      {list.map((m) => {
        // The role only where it says something the name does not — a
        // teammate called "QA Engineer" with that role is one fact.
        const roleLine = roleSubtitle(m.name, m.role);
        return (
          <li key={m.id}>
            <div
              data-avatar-hover-scope
              className="group/member flex items-center gap-3 rounded-lg px-2 py-1.5 transition-colors hover:bg-rail-hover"
            >
              {/* Outside the row button: a button in a button is invalid, and
                  the face opens the profile while the row opens a DM. */}
              <AgentFace agentId={m.id} surface="background" name={m.name}>
                <AgentAvatarButton agentId={m.id} name={m.name}>
                  <TeammateAvatar {...memberFace(m)} className="size-8 bg-avatar-disc text-2xs" />
                </AgentAvatarButton>
              </AgentFace>
              <button
                type="button"
                onClick={() => onMessage(dmChannelId(m))}
                title={`Message ${m.name}`}
                className="flex min-w-0 flex-1 items-center gap-2 text-left"
              >
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium">{m.name}</span>
                  {roleLine && (
                    <span className="block truncate text-xs text-muted-foreground">{roleLine}</span>
                  )}
                </span>
                {m.id === leadId && (
                  <span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-3xs font-medium text-muted-foreground">
                    Lead
                  </span>
                )}
              </button>
              {onAdd && (
                <button
                  type="button"
                  onClick={() => onAdd(m.id)}
                  aria-label={`Add ${m.name} to this channel`}
                  title={`Add ${m.name} to this channel`}
                  className={cn(
                    ICON_BUTTON,
                    "size-7 shrink-0 opacity-0 group-hover/member:opacity-100 focus-visible:opacity-100",
                  )}
                >
                  <Plus className="size-4" aria-hidden />
                </button>
              )}
            </div>
          </li>
        );
      })}
    </ul>
  );
}
