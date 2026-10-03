import { useMemo, useState } from "react";
import {
  CircleDot,
  Hash,
  Lock,
  PanelRight,
  Plus,
  SquarePen,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { AgentFace } from "@/components/agent-face";
import { agentPresenceLabel } from "@/components/agent-status-dot";
import { TeammateAvatar } from "@/components/teammate-avatar";
import { useFlipList } from "@/hooks/use-flip-list";
import { useStableList } from "@/hooks/use-stable-list";
import { cn } from "@/lib/utils";
import { useAgentPresence } from "@/room/store";
import { NewMessageDialog } from "./NewMessageDialog";
import { channelSubtitle, dmFace, dmThreadId, type Channel, type ChannelSection } from "./model";

/**
 * What an unread badge actually claims (issue #364).
 *
 * The one thing on this rail that is still console-local: unread is derived
 * here from when this tab last looked at a channel, because the host has no
 * read-receipt surface. Transcripts, threads and reactions are all the host's
 * now — this is not, and it says so rather than letting an operator read the
 * badge as "unread by my team".
 */
const UNREAD_IS_LOCAL = "Estimated in this browser — unread is not tracked on the company.";

/** The two icon buttons above the list — same size, hit area and hover. */
const DOOR =
  "rounded-md p-1 text-muted-foreground hover:bg-accent hover:text-foreground aria-expanded:bg-accent aria-expanded:text-foreground";

interface Props {
  sections: ChannelSection[];
  /**
   * Opens the channel creator (issue #1835) — rendered as a "+" on the
   * Channels section header. Absent (the rule for a control that would be
   * refused) when the roster cannot staff a channel yet.
   */
  onAddChannel?: () => void;
  /**
   * Opens the create-agent dialog (`AddMemberDialog`), which `RoomView` mounts
   * and owns — the rail only asks. The "+" menu's "Create a new agent".
   */
  onAddAgent?: () => void;
  activeId: string | null;
  /** Channel id → unread count. Absent or 0 reads as caught up. */
  unread: Record<string, number>;
  /** Channel id → how many unread mentions name this person there. */
  mentions?: Record<string, number>;
  onSelect: (id: string) => void;
  collapsed?: boolean;
  onExpand?: () => void;
  /** Controlled section-disclosure state, shared across the desktop and
   * sub-`lg` rail instances so crossing the breakpoint keeps the operator's
   * folds (codex P2 review). Falls back to instance-local state. */
  openSections?: Record<string, boolean>;
  onToggleSection?: (id: string) => void;
  directMessages?: Channel[];
  onStartDirectMessage?: (id: string) => void;
  className?: string;
  /**
   * Whether the channel this rail marks is the page on screen.
   *
   * `true` — the default, and what a rail beside its own transcript means —
   * makes the marked row `aria-current="page"`. `false` demotes it to
   * `aria-current="true"`: still "the one of these you are on", but not a claim
   * to be the current *page*.
   *
   * It exists because this rail is pinned in the app sidebar on every section
   * since #2130. On `#/finances/wallet` the marked channel is where Room will
   * take you back to, not the page being read — and two nodes claiming `page`
   * is a page a screen reader cannot locate you on, which is the same defect
   * the section rail was reviewed for on that PR. `RoomView` passes its own
   * `routeOpen` straight through.
   */
  currentPage?: boolean;
  /**
   * Whether the Direct messages list may slide rows to their new slot when a
   * message re-sorts it. `RoomView` passes `false` until every channel's
   * history has landed, so a cold load does not play a storm of moves.
   */
  animateReorder?: boolean;
}

/**
 * The workspace's channel list.
 *
 * Sections collapse, rows carry their own icon by kind (`#` for a channel, a
 * lock when private, the teammate's avatar for a DM), and an unread channel
 * goes bold with a count on the right. This is the second sidebar on the
 * screen — the app's own nav is to its left — so it stays visually quieter
 * than that one: no group headers in caps, no badges except unread.
 */
export function ChannelRail({
  sections,
  onAddChannel,
  onAddAgent,
  activeId,
  unread,
  mentions,
  onSelect,
  collapsed = false,
  onExpand,
  directMessages = [],
  onStartDirectMessage,
  className,
  currentPage = true,
  animateReorder = false,
}: Props) {
  // Which picker the compose menu has open: `dm` is the agent picker, `channel` the channel picker. One at a time; `null` is none. (Create a new agent is not here — it asks `RoomView` for the real `AddMemberDialog`.)
  const [dialog, setDialog] = useState<"dm" | "channel" | null>(null);
  // Resolved once and threaded down, so the three row shapes cannot come to
  // disagree about what marking the open channel means.
  const activeAria: "page" | "true" = currentPage ? "page" : "true";
  // The FILL is gated on being the page; the mark is not.
  //
  // This rail is pinned in the sidebar on every section since #2130, so on
  // `#/company/work` the channel you last opened was still painted with the
  // selected pill — two filled rows on screen, one of them in a column you are
  // not looking at, both claiming to be where you are. `aria-current` already
  // drew this distinction (`page` vs `true`) and the pixels did not.
  //
  // Off-route the row keeps its weight and loses its fill: still legibly "the
  // one Room will take you back to", no longer a claim to be the open page.
  // `active` itself is untouched, so the unread badge stays suppressed on the
  // channel you have actually read.
  const onPage = currentPage;
  // The Direct messages order is `latestMessageAt` descending, so a message
  // moves its row to the top. A row sliding under the pointer can land a click
  // on the wrong DM (the same hazard as #1414), so the order is held while the
  // pointer or keyboard focus is anywhere in the rail and reconciles on
  // release. Focus a click left behind does not hold (`holdPointerFocus`): the
  // clicked row keeps focus after the pointer leaves, and holding on it froze
  // the order until focus happened to move. Only the ORDER is held, as ids:
  // row content (name, unread) still reads live.
  const dmSection = sections.find((s) => s.id === "dms");
  const liveDmIds = useMemo(() => dmSection?.channels.map((c) => c.id) ?? [], [dmSection]);
  const stable = useStableList(liveDmIds, { holdPointerFocus: false });
  const shownSections = useMemo(() => {
    if (!dmSection) return sections;
    const byId = new Map(dmSection.channels.map((c) => [c.id, c]));
    const held = stable.items.flatMap((id) => byId.get(id) ?? []);
    // A DM that appeared mid-hold goes last rather than shifting the rows the
    // pointer is aiming at; the release puts it in its real slot.
    const late = dmSection.channels.filter((c) => !stable.items.includes(c.id));
    return sections.map((s) => (s === dmSection ? { ...s, channels: [...held, ...late] } : s));
  }, [sections, dmSection, stable.items]);
  const dmRowRef = useFlipList(
    shownSections.find((s) => s.id === "dms")?.channels.map((c) => c.id) ?? [],
    { disabled: !animateReorder || collapsed },
  );

  if (collapsed) {
    return (
      <aside
        {...stable.containerProps}
        className={cn(
          "w-14 shrink-0 flex-col items-center border-r bg-sidebar/40 py-3",
          className,
        )}
      >
        <Button
          variant="ghost"
          size="icon"
          className="size-8 text-muted-foreground"
          onClick={onExpand}
          aria-label="Expand channels"
          title="Expand channels"
        >
          <PanelRight className="size-4" />
        </Button>
        <nav aria-label="Channels" className="mt-3 flex w-full flex-col items-center gap-1 px-2">
          {sections.flatMap((section) => section.channels).map((channel) => (
            <CompactChannelRow
              key={channel.id}
              channel={channel}
              active={channel.id === activeId}
              activeAria={activeAria}
              onPage={onPage}
              unread={unread[channel.id] ?? 0}
              mentions={mentions?.[channel.id] ?? 0}
              onSelect={onSelect}
            />
          ))}
        </nav>
      </aside>
    );
  }

  // One list, no captions. Channels first, then DMs, each kind in the order it
  // already had — the row's own icon (`#`, a lock, the teammate's avatar) is
  // what tells them apart now, not a heading over them.
  const rows = shownSections.flatMap((section) => section.channels);
  const channelRows = rows.filter((channel) => channel.kind !== "dm");
  const dmRows = rows.filter((channel) => channel.kind === "dm");
  const row = (channel: Channel) => (
    <ChannelRow
      channel={channel}
      active={channel.id === activeId}
      activeAria={activeAria}
      onPage={onPage}
      unread={unread[channel.id] ?? 0}
      mentions={mentions?.[channel.id] ?? 0}
      onSelect={onSelect}
    />
  );
  const channelsOnly = sections
    .filter((section) => section.id !== "dms")
    .flatMap((section) => section.channels);
  const canMessage = directMessages.length > 0 && !!onStartDirectMessage;

  return (
    <aside
      {...stable.containerProps}
      className={cn(
        "w-64 shrink-0 flex-col border-r bg-sidebar/40 pb-3",
        className,
      )}
    >
      {/* The list's two doors, on one row with no caption: "+" makes things,
          the pencil starts a conversation. They were each on a section header;
          the headers are gone, the doors are not. */}
      <div className="flex items-center gap-0.5 pt-2">
        {/* A caption, not a control and not a heading element: the list is one
            ungrouped run, this only names it. A `div` for the reason
            `section-rail.tsx`'s own caption is one (issue #1392,
            `nav-rail-headings.test.ts`); `px-2` puts it on the rows' text line. */}
        <div className="min-w-0 flex-1 truncate px-2 text-xs font-medium text-muted-foreground">
          Conversations
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger
            title="New"
            aria-label="New"
            className={DOOR}
          >
            <Plus className="size-3.5" aria-hidden />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-auto min-w-48">
            <DropdownMenuItem disabled={!onAddChannel} onClick={() => onAddChannel?.()}>
              Create a new channel
            </DropdownMenuItem>
            <DropdownMenuItem disabled={!onAddAgent} onClick={() => onAddAgent?.()}>
              Create a new agent
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <DropdownMenu>
          <DropdownMenuTrigger
            title="Start a conversation"
            aria-label="Start a conversation"
            className={DOOR}
          >
            <SquarePen className="size-3.5" aria-hidden />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-auto min-w-56">
            <DropdownMenuItem
              disabled={channelsOnly.length === 0}
              onClick={() => setDialog("channel")}
            >
              Start a conversation in a channel
            </DropdownMenuItem>
            <DropdownMenuItem disabled={!canMessage} onClick={() => setDialog("dm")}>
              Start a conversation with the agent
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {/* No horizontal padding of its own: the sidebar group already gutters the
          rail, and a second one pushed every row right of the nav rows. */}
      {/* One visual list, two lists in the DOM. The DM rows slide when a message
          re-sorts them, and `useFlipList` measures a row against its own list —
          so the DMs need a list of their own, or a channel appearing above them
          would shift every DM slot and play a slide that is not a re-sort. No
          caption, border or extra gap sits between the two: read as one run. */}
      <div className="mt-0.5 flex select-none flex-col gap-px">
        <ul className="flex flex-col gap-px">
          {channelRows.map((channel) => (
            <li key={channel.id}>{row(channel)}</li>
          ))}
        </ul>
        <ul
          // A re-sort moves DM rows in the DOM. Left as scroll-anchor candidates,
          // a visible row that jumped to the top dragged the scrolled sidebar
          // with it, so the sliding list opts out and the offset stays put.
          className="flex flex-col gap-px [overflow-anchor:none]"
        >
          {dmRows.map((channel) => (
            <li key={channel.id} ref={dmRowRef(channel.id)}>
              {row(channel)}
            </li>
          ))}
        </ul>
        {rows.length === 0 && (
          <p className="px-2 py-1 text-xs text-muted-foreground">Nothing here yet.</p>
        )}
      </div>

      {/* Controlled: the menu items open these, there is no trigger of their own. */}
      <NewMessageDialog
        open={dialog === "dm"}
        onOpenChange={(next) => setDialog(next ? "dm" : null)}
        directMessages={directMessages}
        onSelect={(id) => onStartDirectMessage?.(id)}
      />
      <NewMessageDialog
        open={dialog === "channel"}
        onOpenChange={(next) => setDialog(next ? "channel" : null)}
        directMessages={channelsOnly}
        onSelect={onSelect}
        title="Start a conversation in a channel"
        description="Choose a channel to talk in."
      />
    </aside>
  );
}

function CompactChannelRow({
  channel,
  active,
  activeAria,
  onPage,
  unread,
  mentions,
  onSelect,
}: {
  channel: Channel;
  active: boolean;
  activeAria: "page" | "true";
  /** Whether this rail's channel is the page on screen — see `onPage`. */
  onPage: boolean;
  unread: number;
  mentions: number;
  onSelect: (id: string) => void;
}) {
  const hasUnread = unread > 0 && !active;
  const hasMentions = mentions > 0;

  return (
    <button
      type="button"
      onClick={() => onSelect(channel.id)}
      aria-current={active ? activeAria : undefined}
      // The compact row renders unread as a bare dot, so the count has to live
      // in the accessible name — the expanded row says it in text, and
      // collapsing the rail must not strip the same fact from the screen-reader
      // tree. The dot itself stays a sighted-hover-only cue.
      aria-label={
        [
          channel.name,
          hasMentions && `${mentions > 99 ? "99+" : mentions} mention${mentions === 1 ? "" : "s"}`,
          hasUnread && `${unread > 99 ? "99+" : unread} unread`,
        ]
          .filter(Boolean)
          .join(", ")
      }
      title={channel.name}
      className={cn(
        "relative flex size-9 shrink-0 items-center justify-center rounded-md transition-colors",
        active
          ? onPage
            ? "bg-sidebar-accent text-sidebar-accent-foreground"
            : "text-foreground"
          : "text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground",
      )}
    >
      <ChannelIcon channel={channel} />
      {hasMentions && (
        <span
          data-testid="channel-mentions"
          title={`${mentions} ${mentions === 1 ? "mention" : "mentions"} of you here`}
          className="absolute -right-0.5 -top-0.5 size-2 rounded-full bg-destructive"
        />
      )}
      {hasUnread && (
        <span
          title={UNREAD_IS_LOCAL}
          className={cn(
            "absolute -right-0.5 size-2 rounded-full bg-primary",
            hasMentions ? "-bottom-0.5" : "-top-0.5",
          )}
        />
      )}
    </button>
  );
}

function ChannelRow({
  channel,
  active,
  activeAria,
  onPage,
  unread,
  mentions,
  onSelect,
}: {
  channel: Channel;
  active: boolean;
  activeAria: "page" | "true";
  /** Whether this rail's channel is the page on screen — see `onPage`. */
  onPage: boolean;
  unread: number;
  mentions: number;
  onSelect: (id: string) => void;
}) {
  const hasUnread = unread > 0 && !active;
  const hasMentions = mentions > 0;
  // The dot on the avatar is decorative here (`AgentFace decorative`): its
  // words go AFTER the name, so the row is announced "Ada Lovelace, Thinking"
  // and a screen-reader user hears who before what. Same lookup the dot makes.
  const statusAgent = channel.kind === "dm" && dmFace(channel) ? channel.member?.id : undefined;
  const status = useAgentPresence(
    statusAgent,
    channel.member ? dmThreadId(channel.member) : undefined,
  );

  return (
    <button
      type="button"
      onClick={() => onSelect(channel.id)}
      aria-current={active ? activeAria : undefined}
      // The row's own label is `channel.name`, so a tooltip that resolves to
      // the same string is the header's issue-#1180 duplicate in a slower
      // form: you hover for a second fact and get the one already under the
      // cursor. No tooltip at all is the better answer, and `undefined` — not
      // `""` — is what suppresses the native bubble.
      title={channelSubtitle(channel) ?? undefined}
      className={cn(
        "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-md transition-colors",
        active
          ? onPage
            ? "bg-sidebar-accent font-medium text-sidebar-accent-foreground"
            : "font-medium text-foreground"
          : "text-muted-foreground hover:bg-sidebar-accent/50 hover:text-foreground",
        hasUnread && "font-semibold text-foreground",
      )}
    >
      <ChannelIcon channel={channel} withStatus />
      <span className="min-w-0 flex-1 truncate">{channel.name}</span>
      {status !== "inactive" && (
        <span className="sr-only">, {agentPresenceLabel(status)}</span>
      )}
      {hasMentions && (
        <span
          data-testid="channel-mentions"
          title={mentions === 1 ? "1 mention of you here" : `${mentions} mentions of you here`}
          className="shrink-0 rounded-full bg-destructive px-1.5 text-3xs font-semibold leading-4 text-destructive-foreground"
        >
          @{mentions > 99 ? "99+" : mentions}
        </span>
      )}
      {hasUnread && (
        <span
          data-testid="channel-unread"
          // Issue #364: unread is derived in this browser from what this tab has
          // seen — the host keeps no read receipts. Two consoles will disagree,
          // and a badge that quietly means something narrower than it looks is
          // worse than one that says so.
          title={UNREAD_IS_LOCAL}
          className="shrink-0 rounded-full bg-primary px-1.5 text-3xs font-semibold leading-4 text-primary-foreground"
        >
          {unread > 99 ? "99+" : unread}
        </span>
      )}
    </button>
  );
}

function ChannelIcon({ channel, withStatus = false }: { channel: Channel; withStatus?: boolean }) {
  if (channel.kind === "dm") {
    const face = dmFace(channel);
    return face ? (
      // The live state badge rides the expanded row only: the compact rail's
      // 36px tiles are measured to fit its 48px width and stay as they were.
      // Scoped to this DM's own thread, so a teammate busy in a channel does
      // not light every row that names them.
      <AgentFace
        agentId={withStatus ? channel.member?.id : undefined}
        chatId={channel.member ? dmThreadId(channel.member) : undefined}
        surface="chrome"
        decorative
      >
        <TeammateAvatar {...face} className="size-6 text-2xs" />
      </AgentFace>
    ) : (
      <CircleDot className="size-4 shrink-0" aria-hidden />
    );
  }
  const Icon = channel.private ? Lock : Hash;
  return <Icon className="size-4 shrink-0 opacity-70" aria-hidden />;
}
