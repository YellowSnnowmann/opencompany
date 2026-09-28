import type { ReactNode } from "react";
import { Plus } from "lucide-react";

import { AgentAvatarButton } from "@/components/agent-profile-sheet";
import { TeammateAvatar } from "@/components/teammate-avatar";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import type { PresenceStatus } from "@/lib/awareness";
import { roleSubtitle, type TeamMember } from "@/lib/team";
import { cn } from "@/lib/utils";
import { PresenceDot } from "@/views/room/PresenceDot";

interface Props {
  /**
   * Who is in the channel on screen, lead first — or `null` when the channel
   * names no membership of its own (a fallback desk on a host without
   * `.../desks`). `null` renders one plain roster list, which is what this pane
   * did for every channel before issue #369.
   */
  channelMembers: TeamMember[] | null;
  /**
   * Everyone else on the roster. The roster-wide surface stays reachable — you
   * still need it to open somebody's DM or add a teammate — it just stops being
   * presented as this channel's membership. When `channelMembers` is `null`
   * this is the whole roster.
   */
  others: TeamMember[];
  /**
   * The desk's lead (`DeskDto.members[0]`) — the routing target for this
   * channel, badged rather than left implicit. Matched by id, so a lead who is
   * no longer on the roster simply goes unbadged instead of promoting whoever
   * happens to be first.
   */
  leadId?: string;
  /**
   * The company's people.
   *
   * A section of their own, and **not** part of `channelMembers` or `others`:
   * desk membership is a teammate concept, so a person is never "in" or
   * "outside" a channel — every signed-in human can see every desk. Folding
   * them into either list would state a membership that does not exist.
   *
   * Absent on a host without the mentionables route, which simply renders no
   * People section.
   */
  people?: Array<{ id: string; label: string }>;
  /**
   * Who is present, keyed by user id. A person missing from this map has **no
   * live signal** — which is not the same as offline, because presence is
   * replica-local. {@link PresenceDot} renders that distinction.
   */
  presence?: ReadonlyMap<string, { status: PresenceStatus }>;
  loading: boolean;
  /** True when the roster came from the host rather than the starter set. */
  fromHost: boolean;
  /**
   * Put an agent already on the roster onto this channel's desk (issue #2224).
   * Only ever offered on an "Everyone else" row. Absent has the same meaning
   * `onManageDesk` gives it: no real desk behind this channel, nothing to add
   * to — the caller gates this on `activeIsDesk`, not on anything this pane
   * can see for itself, because a DM has real (non-null) channel membership
   * too and is not a desk.
   */
  onAddExisting?: (id: string) => void;
  onMessage: (member: TeamMember) => void;
  /**
   * Open this channel's desk on the org chart (issue #485). Absent for a DM and
   * for a fallback desk — neither is a desk the chart draws.
   *
   * A link, deliberately, and not an editor bolted on here. This pane *drops* a
   * member id that resolves to no roster teammate (see `channelMembers`), which
   * is right for a chat — you cannot message nobody. But that ghost seat is
   * precisely the one an operator most needs to remove, and an editor on this
   * pane could not offer it without breaking the drop rule the pane is built
   * on. Membership editing therefore stays on the surface that keeps ghost
   * seats visible and badged: the chart.
   */
  onManageDesk?: () => void;
  /**
   * Whether to offer the daily-budget controls at all (issue #360, ported
   * from the retired Team page): admin-only, and only for a host-backed
   * teammate — a starter-roster row is a local placeholder with no budget
   * record to edit.
   */
  /** Who set a teammate's cap override, resolved to a display label. */
}

/**
 * The right-hand member pane — who is in this channel, and the rest of the
 * company under it.
 *
 * This replaces the standalone Team page for channel-scoped work: put an
 * agent already on the roster onto this desk directly from "Everyone else"
 * (issue #2224), and a row now also opens that teammate's DM, which the page
 * could not do at all. Hiring a brand-new teammate, and dropping one from the
 * roster entirely, are different actions and live elsewhere (the empty-desk
 * "Add an agent" prompt and the Team page, respectively) — this pane only
 * ever offers an agent already on the roster, and never removes one.
 *
 * The two sections exist because those are two different questions. "Who is in
 * this room" is what a channel header is for, and answering it with the whole
 * company answered nothing (issue #369). But the roster-wide actions still have
 * to live somewhere, so the rest of the company keeps its rows — visually
 * subordinate, and never labelled as this channel's membership.
 */
export function MembersPane({
  channelMembers,
  others,
  leadId,
  people,
  presence,
  loading,
  fromHost,
  onAddExisting,
  onMessage,
  onManageDesk,
}: Props) {
  const total = (channelMembers?.length ?? 0) + others.length;
  // Both scopes on one line, so the pane never leaves you guessing which of the
  // two numbers the header's count refers to.
  const subtitle = channelMembers
    ? `${channelMembers.length} in this channel · ${total} in the company`
    : `${total} ${total === 1 ? "agent" : "agents"} · ${
        fromHost ? "defined by this company" : "starter roster"
      }`;

  return (
    <aside className="flex w-72 shrink-0 flex-col border-l bg-background">
      <header className="flex h-13 shrink-0 items-center gap-2 border-b px-3">
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold tracking-tight">Team</h2>
          <p className="truncate text-xs text-muted-foreground">{loading ? "Loading…" : subtitle}</p>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto p-2">
        {loading ? (
          <div className="space-y-2 p-1">
            {Array.from({ length: 6 }).map((_, i) => (
              <Skeleton key={i} className="h-12 rounded-lg" />
            ))}
          </div>
        ) : (
          (() => {
            // `withAdd` is only ever true for the "Everyone else" list on a
            // real desk (see below) — never for `channelMembers` (already
            // here) and never for the plain-roster fallback (no desk id to
            // add into).
            const rows = (list: TeamMember[], withAdd?: boolean) => (
              <ul className="flex flex-col gap-px">
                {list.map((m) => (
                  <li key={m.id}>
                    <MemberRow
                      member={m}
                      lead={m.id === leadId}
                      onMessage={() => onMessage(m)}
                      onAddToChannel={
                        withAdd && onAddExisting ? () => onAddExisting(m.id) : undefined
                      }
                    />
                  </li>
                ))}
              </ul>
            );

            // No membership to scope to — one plain roster, as before.
            if (!channelMembers) return rows(others);

            return (
              <>
                <div className="flex items-baseline justify-between gap-2">
                  <SectionLabel className="text-foreground">In this channel</SectionLabel>
                  {onManageDesk && (
                    <ManageDeskLink onClick={onManageDesk}>Manage on the org chart</ManageDeskLink>
                  )}
                </div>
                {channelMembers.length > 0 ? (
                  rows(channelMembers)
                ) : (
                  // An empty desk is the strongest case for reaching the
                  // editor, so the copy carries the way there rather than
                  // leaving the operator to find the header link.
                  <p className="px-2 py-1.5 text-xs text-muted-foreground">
                    Nobody is on this desk yet.
                    {onManageDesk && (
                      <>
                        {" "}
                        <ManageDeskLink onClick={onManageDesk} inline>
                          Staff it on the org chart
                        </ManageDeskLink>
                      </>
                    )}
                  </p>
                )}

                {others.length > 0 && (
                  <div className="mt-2 border-t pt-2">
                    <SectionLabel className="text-muted-foreground">Everyone else</SectionLabel>
                    {rows(others, true)}
                  </div>
                )}

                {people && people.length > 0 && (
                  <div className="mt-2 border-t pt-2">
                    <SectionLabel className="text-muted-foreground">People</SectionLabel>
                    {people.map((person) => (
                      <div
                        key={person.id}
                        data-testid="person-row"
                        className="flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm"
                      >
                        <PresenceDot status={presence?.get(person.id)?.status} />
                        <span className="min-w-0 flex-1 truncate">{person.label}</span>
                      </div>
                    ))}
                  </div>
                )}
              </>
            );
          })()
        )}
      </div>
    </aside>
  );
}

/**
 * The way out to the org chart (issue #485).
 *
 * Quiet by design — a link, not a button. Editing membership is a different
 * surface's job, and dressing the way there as an action here would suggest
 * this pane could do it.
 */
function ManageDeskLink({
  onClick,
  inline,
  children,
}: {
  onClick: () => void;
  /** Sits inside a sentence rather than beside a heading. */
  inline?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "text-xs text-muted-foreground hover:text-foreground",
        inline ? "underline" : "shrink-0 px-2 pb-1",
      )}
    >
      {children}
    </button>
  );
}

/** A section heading inside the pane's scroll body. */
function SectionLabel({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <h3 className={cn("px-2 pb-1 text-2xs font-medium uppercase tracking-wide", className)}>
      {children}
    </h3>
  );
}

function MemberRow({
  member,
  lead,
  onAddToChannel,
  onMessage,
}: {
  member: TeamMember;
  /** The desk's lead — badged, since this channel routes to them. */
  lead?: boolean;
  /**
   * Present only on an "Everyone else" row when there is a real desk to add
   * to (issue #2224) — `undefined` renders no button at all, not a disabled
   * one, the same rule every other optional action on this row follows.
   */
  onAddToChannel?: () => void;
  onMessage: () => void;
}) {
  // Issue #1208: only when the role is not the name over again. The roster's
  // name falls back to the role (`fromDto`), so every manifest-declared
  // teammate said it twice here too.
  const roleLine = roleSubtitle(member.name, member.role);

  return (
    <div className="group/member flex items-center gap-2.5 rounded-lg px-2 py-1.5 transition-colors hover:bg-accent/60">
      {/* Outside the row button, not inside it: a button inside a button is
          invalid HTML, and the two want different things anyway — the face
          opens who this agent is (issue #1653), the row opens a line to
          them. */}
      <AgentAvatarButton agentId={member.id} name={member.name}>
        <TeammateAvatar
          name={member.name}
          tone={member.tone}
          avatar={member.avatar}
          mascotCostume={member.mascotCostume}
          mascotSkinColor={member.mascotSkinColor}
          mascotHandColor={member.mascotHandColor}
          mascotMode={member.mascotMode}
          className="size-8"
        />
      </AgentAvatarButton>
      <button
        type="button"
        onClick={onMessage}
        className="flex min-w-0 flex-1 items-center gap-2.5 text-left"
        title={member.description || member.role}
      >
        <span className="min-w-0">
          <span className="flex items-center gap-1">
            <span className="truncate text-sm font-medium">{member.name}</span>
            {lead && (
              <span className="shrink-0 rounded border px-1 text-3xs font-medium uppercase tracking-wide text-muted-foreground">
                Lead
              </span>
            )}
          </span>
          {roleLine && (
            <span className="block truncate text-xs text-muted-foreground">{roleLine}</span>
          )}
        </span>
      </button>

      {onAddToChannel && (
        <Button
          variant="ghost"
          size="icon"
          className="size-7 shrink-0 opacity-0 transition-opacity focus-visible:opacity-100 group-hover/member:opacity-100"
          aria-label={`Add ${member.name} to this channel`}
          title={`Add ${member.name} to this channel`}
          onClick={onAddToChannel}
        >
          <Plus className="size-4" />
        </Button>
      )}
    </div>
  );
}

