import { useMemo, useState, type ReactNode } from "react";
import {
  ChevronRight,
  CircleDot,
  Hash,
  Lock,
  type LucideIcon,
  PanelRight,
  Plus,
  SquarePen,
} from "lucide-react";

import { Button } from "@/components/ui/button";
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

interface Props {
  sections: ChannelSection[];
  /**
   * Opens the channel creator (issue #1835) — rendered as a "+" on the
   * Channels section header. Absent (the rule for a control that would be
   * refused) when the roster cannot staff a channel yet.
   */
  onAddChannel?: () => void;
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
  activeId,
  unread,
  mentions,
  onSelect,
  collapsed = false,
  onExpand,
  openSections,
  onToggleSection,
  directMessages = [],
  onStartDirectMessage,
  className,
  currentPage = true,
  animateReorder = false,
}: Props) {
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
  // Section disclosure lives here rather than inside `Section`, because the
  // collapsed branch below unmounts every `Section`. Held inside them, folding
  // a section and then collapsing the rail would reopen it on expand — the
  // density toggle must not discard the operator's organization. Absent means
  // "open": the default is a fully expanded list. `RoomView` passes the state
  // in so both rail instances share one fold set across the `lg` breakpoint;
  // a standalone rail (tests, other hosts) keeps it local to the instance.
  const [internalOpenSections, setInternalOpenSections] = useState<Record<string, boolean>>({});
  const resolvedOpenSections = openSections ?? internalOpenSections;
  const toggleSection = (id: string) => {
    if (onToggleSection) {
      onToggleSection(id);
    } else {
      setInternalOpenSections((prev) => ({ ...prev, [id]: !(prev[id] ?? true) }));
    }
  };

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

  return (
    <aside
      {...stable.containerProps}
      className={cn(
        "w-64 shrink-0 flex-col border-r bg-sidebar/40 pb-3",
        className,
      )}
    >
      {shownSections.map((section) => (
        <Section
          key={section.id}
          section={section}
          rowRef={section.id === "dms" ? dmRowRef : undefined}
          // Each section header carries its own door, and only its own.
          // Channels gets "+" (create a channel); Direct messages gets the
          // compose pencil, because a DM is what it starts. It used to float
          // alone above the whole list, attached to nothing and reading as
          // chrome for the rail rather than an action on a section.
          action={
            section.id === "channels" ? (
              onAddChannel && <SectionAction onClick={onAddChannel} label="New channel" icon={Plus} />
            ) : section.id === "dms" && onStartDirectMessage ? (
              <NewMessageDialog
                directMessages={directMessages}
                onSelect={onStartDirectMessage}
                trigger={
                  <SectionAction
                    label="New message"
                    icon={SquarePen}
                    disabled={directMessages.length === 0}
                  />
                }
              />
            ) : undefined
          }
          activeId={activeId}
          activeAria={activeAria}
          onPage={onPage}
          unread={unread}
          mentions={mentions}
          onSelect={onSelect}
          open={resolvedOpenSections[section.id] ?? true}
          onToggle={() => toggleSection(section.id)}
        />
      ))}
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

/**
 * One section header's door, on the right of its caption.
 *
 * One component for both, so "+" on Channels and the compose pencil on Direct
 * messages read as the same kind of affordance — same size, same hit area, same
 * hover — rather than two controls that happen to sit in the same place.
 */
function SectionAction({
  label,
  icon: Icon,
  onClick,
  disabled,
  ...rest
}: {
  label: string;
  icon: LucideIcon;
  onClick?: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={label}
      // The whole of what a screen reader gets for an icon-only control.
      aria-label={label}
      className="rounded-md p-1 text-muted-foreground hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
      {...rest}
    >
      <Icon className="size-3.5" aria-hidden />
    </button>
  );
}

function Section({
  section,
  activeId,
  activeAria,
  onPage,
  unread,
  mentions,
  onSelect,
  open,
  onToggle,
  action,
  rowRef,
}: {
  section: ChannelSection;
  activeId: string | null;
  activeAria: "page" | "true";
  /** Whether this rail's channel is the page on screen — see `onPage`. */
  onPage: boolean;
  unread: Record<string, number>;
  mentions?: Record<string, number>;
  onSelect: (id: string) => void;
  open: boolean;
  onToggle: () => void;
  /** This section's own door, rendered at the right of its caption. */
  action?: ReactNode;
  /** Per-row ref from `useFlipList`, for a section whose rows slide when it re-sorts. */
  rowRef?: (channelId: string) => (node: HTMLElement | null) => void;
}) {
  const hiddenUnread = !open
    ? section.channels.reduce((n, c) => n + (unread[c.id] ?? 0), 0)
    : 0;
  const hiddenMentions = !open
    ? section.channels.reduce((n, c) => n + (mentions?.[c.id] ?? 0), 0)
    : 0;

  return (
    // No horizontal padding of its own. This rail was written as a standalone
    // column with its own gutter; inside the sidebar that gutter doubles up
    // against `SidebarGroup`'s `px-3` and pushes every channel row 8px right of
    // the four nav rows above — which is what made the list read as a panel
    // pasted into the column rather than part of it. Measured, not guessed: the
    // nav row's box starts at x=12 and its icon at x=20, and with this removed
    // a channel row lands on exactly the same two numbers.
    <section className="group/section select-none pt-2">
      <div className="flex items-center gap-0.5">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        // `px-2`, matching the nav rows above: the caption's chevron then
        // stands on the same vertical line as their icons.
        className="flex w-full min-w-0 flex-1 items-center gap-1 rounded-md px-2 py-1 text-left text-xs font-medium text-muted-foreground transition-colors hover:text-foreground"
      >
        <ChevronRight
          className={cn("size-3 shrink-0 transition-transform", open && "rotate-90")}
          aria-hidden
        />
        <span className="truncate">{section.label}</span>
        {(hiddenMentions > 0 || hiddenUnread > 0) && (
          <span className="ml-auto flex items-center gap-1">
            {hiddenMentions > 0 && (
              <span
                data-testid="section-mentions"
                title={`${hiddenMentions} ${hiddenMentions === 1 ? "mention" : "mentions"} of you in this section`}
                className="rounded-full bg-destructive px-1.5 text-3xs font-semibold leading-4 text-destructive-foreground"
              >
                @{hiddenMentions > 99 ? "99+" : hiddenMentions}
              </span>
            )}
            {hiddenUnread > 0 && (
              <span
                title={UNREAD_IS_LOCAL}
                className="rounded-full bg-primary px-1.5 text-3xs font-semibold leading-4 text-primary-foreground"
              >
                {hiddenUnread > 99 ? "99+" : hiddenUnread}
              </span>
            )}
          </span>
        )}
      </button>
      {action}
      </div>

      {open && (
        <ul
          // A re-sort moves rows in the DOM. Left as scroll-anchor candidates,
          // a visible row that jumped to the top dragged the scrolled sidebar
          // with it (to 0, or to wherever the row landed), because anchoring
          // keeps the anchor node still on screen. Opting the sliding list out
          // leaves the offset where the operator put it.
          className={cn("mt-0.5 flex flex-col gap-px", rowRef && "[overflow-anchor:none]")}
        >
          {section.channels.map((channel) => (
            <li key={channel.id} ref={rowRef?.(channel.id)}>
              <ChannelRow
                channel={channel}
                active={channel.id === activeId}
                activeAria={activeAria}
                onPage={onPage}
                unread={unread[channel.id] ?? 0}
                mentions={mentions?.[channel.id] ?? 0}
                onSelect={onSelect}
              />
            </li>
          ))}
          {section.channels.length === 0 && (
            <li className="px-2 py-1 text-xs text-muted-foreground">Nothing here yet.</li>
          )}
        </ul>
      )}
    </section>
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
