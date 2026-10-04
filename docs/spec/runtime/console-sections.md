# The console's sections

The sidebar is the **conversation list** on top and two rows — **Company** and
**Connections** — pinned at its foot. Automations is a row on Company's rail.
This file is the record of how it got there: the four-row decision below
(Room, Company, Connections, Flows — "Flows" is now **Automations**), and the
consolidation after it, which is the next section. It is Rule 8 of
[`ledgers-console-ia.md`](ledgers-console-ia.md) written out, because that file
is at its 500-line ceiling and this is the largest IA change it has seen.

## The consolidation: conversations on top, two rows at the foot

The four-row sidebar below was cut again to two, by moving what the other two
rows were into places that already existed. Nothing was retired but the rows.

- **Room is not a row.** The conversation list *is* the front of the sidebar,
  and it is already on every route (it is portalled in by `RoomView`, which is
  mounted everywhere). A row that said "Room" over a list that is always there
  was a second way into what is already on screen. The `chat` route, `RoomView`
  and everything in `views/room/` are intact; `#/chat/<id>` still resolves and
  `DEFAULT_VIEW` is still `chat`. Only the nav row went.
- **Automations is a row on Company's rail**, a plain child after Brain and
  before the Finance caption. It must be a *direct* child of the section:
  `sectionOwning` reads only `section.children`, so a grandchild with its own
  view would not light Company. After the Finance caption it would read as one
  of Finance's pages. `#/workflows` therefore lights Company and draws its rail
  with Automations current; the address, `data-tour="nav-workflows"` and the
  canvas are unchanged.
- **Channels and Direct messages are one list.** The two captioned, collapsible
  sections are gone: one flat list, channels first and then DMs, each kind in
  the order it already had. The row's own icon (`#`, a lock, the teammate's
  avatar) is what tells them apart. A caption, **Conversations**, names the list
  — a `div`, not a heading and not a control (issue #1392 keeps headings out of
  rails; `nav-rail-headings.test.ts` is the guard for `section-rail.tsx`).
- **Two doors on the caption's row.** `+` opens a menu: *Create a new channel*
  (the channel creator) and *Create a new agent* — the real `AddMemberDialog`
  that Company > Agents "Add agent" opens, mounted by `RoomView` with its own
  handler, so it pops in place on any route. The pencil opens *Start a
  conversation in a channel* and *Start a conversation with the agent* (the
  channel and agent pickers, `NewMessageDialog`). There is deliberately no
  Search item: the title bar owns search (⌘K), and the rail once faked a keydown
  to reach it.
- **The foot.** The list takes the leftover height and scrolls inside its own
  slot (`min-h-0 flex-1`, `overflow-y-auto`); Company and Connections are a
  `shrink-0` group under it, so a list at its cap cannot push them out of reach.
  The seam is the group's own `border-t`. Its bottom padding is
  `pb-(--frame-inset)` — the same gap the content card keeps under itself
  (`mb-(--frame-inset)`, 0.75rem) — so Connections' bottom edge and the card's
  are one line (measured: both 888 at a 900px window). The strip below is the
  column's own transparent fill over the shell chrome, not a second surface.

The tour follows the anchors: "Your AI staff" spotlights
`data-tour="conversations"` (the list's slot) rather than the removed
`nav-chat`, and the Automations stop still targets `nav-workflows`, which is now
the Company rail row.

The rest of this file describes the four-row design and why. Where it says
**Room** or **Flows** as a row, read the list and **Automations** as above.

Read Rule 6 there first. It governs what a view without a nav row must be, and
eight views need that call making here.

## Why four (superseded by two — see above)

Ten flat rows is not a list an operator scans, it is a wall — the same judgement
Rule 2 made when it rejected a sidebar row per declared list, one screen earlier
in the same column. Ten equal-weight destinations force a read of every label
before the eye can settle, and they grow: every surface added since has argued
for a row, and each argument was individually reasonable.

Four is not a target number. It is what is left when the ten are sorted by the
question an operator is answering:

| The question | The row |
| --- | --- |
| I want to say something, or see what was said | **Room** |
| Something about my company: who, what, where, what it remembers, what it costs | **Company** |
| Can my teammates reach X yet? | **Connections** |
| What does this company do on repeat? | **Automations** (was "Flows") |

Everything else is chrome — Search, Notifications (with Approvals), Settings
and Discord are icon tabs in the sidebar's foot beside Company and Connections,
and Feedback is a row on the Settings rail — or is filed under one of the four.
There is no Overview: the knowledge-graph page was removed and `#/overview`
rewrites to the chat.

## The sub-navigation is in the CONTENT AREA — and this reverses a decision

This file first recorded the opposite, and the reversal is issue #2130. Both
arguments are kept, because a decision reversed without its reasons on the page
gets reversed back.

**What it said.** Finance, Settings and Connections each drew a `w-60` rail
inside the content area, and that was the wrong place once more than one section
had sub-pages, for two reasons that are not about layout fashion: it puts the
same kind of list in two different places depending on which section you are in
— the sidebar for the sections without sub-pages, a rail for the ones with — so
there is no rule to learn; and it charges the content pane 240px on every page
under it, on a screen that already has a sidebar to its left. PR #1977 built a
Connections content rail and removed it on exactly that argument.

**What outweighs it.** Putting a section's pages in the sidebar's middle region
meant spending that region on them — and what it was spending was the **channel
list**, which only appeared while you were in Room. The channel list is the one
list an operator returns to continuously, from wherever they are. Losing it on
every trip to Company, Connections or Automations is not a width, it is a round trip:
go to Room, find the channel, come back. That costs more than 240px does.

So the trade is inverted. The sidebar's middle region is the Room rail,
permanently, on every section. Company's five pages and Connections' two are the
first column of their content area, drawn by `components/section-rail.tsx` from
the same `NAV_SECTIONS` table the four rows come from.

**The "two places" half of the old argument is answered, not ignored.** There is
exactly one rule now — a section's sub-navigation is the first column of its
content — and exactly one rail on screen at a time:

- **Finance folds in.** Its three pages are nested rows on Company's rail,
  visible while Finance is the open row. A rail of its own would be the second
  240px column beside Company's, which is the 768–1023px band of issue #1383
  reproduced at *every* width. `views/finance/FinanceSection.tsx` is
  dispatch-only as a result — the shape `ConnectionsSection` already had.
- **Settings keeps its own** because it is not one of the four at all. It is a
  footer utility, and its rail *is* this pattern; the shared component copies
  its geometry (`w-60` from `lg`, chips below) rather than the other way round.
- **Room draws none, and Automations is a row on Company's.** Room's
  sub-navigation is the pinned conversation list. Automations has no sub-pages
  to move: the canvas's Automations/Runs toggle is a control on the page's title
  row whose state is client-side rather than an address, so promoting it would be
  inventing sub-pages rather than relocating any.

### Not an accordion, and no longer a swap either

The four rows are always visible, always contiguous, always in the same place.
Selecting a section does not expand a row in place and does not displace a row's
siblings — and since #2130 it does not swap the block below them either. That
block is the channel list on every route, which makes the whole column fixed
furniture: the same rows and the same list, wherever you are. (Two rows below the list
today; the diagram is the four-row layout it replaced.)

```text
┌─────────────────┐┌────────────┬──────────────────┐
│ ■ Room          ││ COMPANY    │                  │
│   Company       ││  Agents    │  the page        │
│   Connections   ││  Work      │                  │
│   Automations   ││  Workspace │                  │
│                 ││  Brain     │                  │
│ CHANNELS      + ││  Finance   │                  │
│  # engineering  ││   Overview │  the section's   │
│  # general      ││   Invoicing│  rail — nested   │
│ DIRECT MSGS   ✎ ││   Wallet   │  rows only while │
│  Neil · Alex    ││            │  Finance is open │
├─────────────────┤│            │                  │
│ ⚙ Settings      ││            │                  │
│ ⚑ Feedback      ││            │                  │
│ ✦ Discord       ││            │                  │
└─────────────────┘└────────────┴──────────────────┘
  fixed on every      one rail, never two
  route now
```

The accordion — each row expanding under itself — was the first shape this took
and was rejected twice over. The rows move, so "Automations is the fourth thing" only
holds while nothing above it is open. And the one region whose contents are
unbounded, the channel list, pushes every row after it off the bottom at an
ordinary twenty channels: the wall, recreated inside one row.

A fixed block also has no per-row open/closed state to keep. Which section is
showing is which section you are in, and the route already carries that.

**The seam between the two blocks.** It was space, not a rule, while the list
sat *under* the rows: the column is quiet, and a divider would have been its only
seam. With the rows now at the foot the seam is the group's own `border-t`, the
idiom the title row uses against the page. The old `pt-5` (24px) is long gone —
it read as the list having come loose from the rows — and there is no top
padding on either group; `sidebar-sections.test.ts` pins that.

### On the collapsed rail

The two icons stay, and so does the channel list: `ChannelRail` has a compact
variant built for exactly that width, and dropping it would make collapsing the
sidebar silently lose the channel list — the regression issue #1018 filed about
the approvals badge, in a new place. Nothing else is in this region to hide any
more. The fixed lists of child rows that *were* hidden here at 3rem are
content-rail rows now, where they keep their labels at every width.

## Room is the chat column, moved whole

`views/room/ChannelRail.tsx` is not reimplemented in the sidebar. It is
**portalled** into a slot the sidebar owns (`components/room-rail.tsx`), so
every behaviour it already had comes with it — bar the collapsible sections,
which the consolidation retired for one flat list: per-kind row icons, unread
and mention badges, the compact collapsed variant, and the two doors on the
Conversations row. The expanded DM rows carry each teammate's live-state badge
(`AgentStatusDot`, read from `useAgentPresence`), and their order follows the
latest message: the list holds still while the pointer or keyboard focus is
inside the rail (a row must not slide under a click) and slides to the new order
on release, unless reduced motion is asked for. Only a re-sort slides: rows are
measured within the list, so a channel appearing above them moves the list
without animating it. The focus a mouse click leaves
on a row does not hold it once the pointer has left. A DM's state is keyed on
the conversation, not the spelling: the bare teammate id the console posts
under and the `dm:<id>` desk the hive seat brackets its turn under are one key
(`presenceChatKey`), so a real DM turn lights its row and header. A hive seat
streams no live frames, so a DM can show working, queued or approval there, but
not thinking or typing. A DM turn another console sent is known only by its
bracket, which names no agent: the thread says whose it is, and it reads queued
until its run's `run_status_changed` says `running`. A card run or delegation in
flight (`/tasks/inflight`) reads working on agent-wide surfaces (team card,
profile, `@` picker), never on a DM row. An approval stops counting on its
`approval_resolved` frame, an expiry included. The row's accessible name is the
teammate's name first, then the state; a dot that stands alone names the agent
in its own label ("Ada Lovelace: Working").

A portal rather than a state lift, deliberately. `ChatView` stays the one owner
of the chat model, the rail renders from that state on the same pass, and the
dialogs it opens still mount inside `ChatView`'s tree — a portal moves the DOM
node, not the component tree. Lifting the model would have meant an effect
writing it up to the shell and a re-render of the whole console every time an
unread count changed.

### Pinning the rail keeps `ChatView` mounted, and that is the price

The rail is painted on every section now, so the view that renders it has to
outlive the route that used to own it. The shell mounts `ChatView` on every
route and hands it `routeOpen`; off Room it renders the rail, its two dialogs,
and nothing else.

The same two options were reweighed and the answer did not change. Lifting the
model into the shell would now be *worse* than it was: the console would
re-render on every unread tick from every section rather than only from Room.

What the portal costs, stated plainly: ~2,400 lines of chat model stay mounted
while an operator is on Company or Automations. The **data** was already resident —
the shell owns the transcripts, the mention feed and the unread map precisely
*because* `ChatView` used to unmount — so what is newly kept is the view's own
state and its desks/roster reads, not the traffic. `ChatView` takes one roster
read at shell mount that it did not take before, which
`onboarding-gate-setup-controller-mount.test.ts` accounts for by name. In
exchange the channel list is never a round trip away and a return to Room
refetches nothing.

One correctness consequence has to be said with it: **a mounted transcript is no
longer evidence of a visible one.** `chatPaneVisible` is `routeOpen && !covering`
— a mention must not be marked read because the operator happens to be on
Company, for the same reason it must not be marked read behind the phone's
covering sheet.

Two further consequences worth stating:

- **The 768–1023px two-rail band is gone by construction.** The rail was a
  second column competing with the app sidebar for the viewport, which is what
  issue #1383 was. It is a section of that sidebar now, so it has no breakpoint
  of its own: the sidebar decides once whether it is a column, a 3rem rail or a
  sheet, and the list follows.
- **`lib/chat-rail.ts` is deleted.** The sidebar's collapse *is* the channel
  list's collapse — one control, one persisted preference, no way for the two to
  disagree. The chat header's density toggle went with it, for the same reason:
  it had become a second control doing the sidebar's job, forty pixels from the
  sidebar's own.

### Room at its real cap

Validated against a company with twenty channels plus direct messages, not
against a demo company with three. The contents block is its own scroll region:
the four rows and the footer never move, and the channel list scrolls within it.

**Scroll, not truncate.** A "show all" was the alternative and is worse here: a
channel list is scanned for a name you already know, so hiding its tail behind a
control makes the one thing you came for the one thing you cannot see. Scrolling
costs a gesture only at the sizes where truncating would cost a click *and* a
gesture.

## The eight Rule 6 calls

Every view that lost, or never had, a nav row, and which of Rule 6's four
treatments it takes.

| View | Treatment | Why |
| --- | --- | --- |
| `overview` | **Removed** | The knowledge-graph page is gone; `#/overview` (and `#/company/graph`) rewrite to the chat and to Company. |
| `approvals` | **Discoverable elsewhere** | The Approvals tab of Notifications, whose bell is an icon tab in the sidebar's foot, and the count travels with it. The foot is visible from every page — the old sidebar badge was not, which is the whole reason `SidebarMenuDot` had to exist (issue #1018). It is deleted with the row. |
| `observatory` | **Discoverable elsewhere** | A row on the Settings rail. `#/settings/observatory` is *rewritten* onto `#/observatory` rather than rendered under Settings — the Observatory owns four query keys of its own and reads them off the hash's head (`views/observatory/hash.ts`), so under `#/settings/…` its analytics tab and agent/turn selection stop being addressable. The rail row is the doorway; the surface keeps its own address. |
| `tasks` | **Deep-link destination** | `#/tasks/<id>` is a card on Work's board, linked from chat, approvals, workflow rows and every card. Bare `#/tasks` is rewritten onto the board (Rule 2). |
| `team` | **Deep-link destination** | `#/team/<id>` is a seat on the org chart. Bare `#/team` is rewritten onto Agents (issue #1141). |
| `pages` | **Deep-link destination** | Agent-authored dashboards are direct-URL-only on purpose (issues #1171, #1172). What keeps `#/pages` answering is its `ROUTABLE` entry, never a commented nav row — that confusion is issue #1311. |
| `inbox` | **Parked but reachable** | Unchanged by this restructure. Issue #302 parked it; issue #1337 is the open question of what a parked surface should say. |
| `feedback` | **Discoverable elsewhere** | The sidebar footer links to it, as it always has. |

`isNavigationActive` in `lib/console-routes.ts` is where two of those deep-link
views are claimed by the section they belong to. Without it the sidebar empties
the moment an operator opens a card or a teammate: the section
goes dark and its contents block disappears with it.

## Labels and view ids are allowed to differ

"Room" was the `chat` view and is no longer a row, though `chat` still routes.
"Automations" is `workflows` (called "Flows" before, and a row on Company's rail
now). "Work" has been `ledgers`
since #1284, and "Agents" is `company`.

A view id is an **address** — every `#/chat/<channelId>` link ever minted, every
`#/workflows/<id>` a run row points at — and renaming a row is not a reason to
break them. The `data-tour` anchors follow the view id for the same reason: they
are how the guided tour and the e2e specs find a row, and they should not move
when a word does.

The one thing that *must* track a rename is prose. `chat-approval-line.spec.ts`
used to build its selector by lowercasing a label, which turned exactly this
rename into a silent break; it takes a view id now.

## The tour is a deliverable of an IA change, not a follow-up

Four of the tour's eight stops pointed at rows this change removes. That fails
**silently**: `waitForTarget` resolves `false` when an anchor never mounts and
the controller treats it as a stop to *skip* rather than an error — deliberately,
so a slow lazy chunk cannot wedge the tour. A stop pointed at a deleted row is
therefore indistinguishable from one that loaded slowly. The tour runs, teaches
less than the product has, and nothing says so.

So the tour was rewritten rather than patched: seven stops, the four sections in
the order an operator meets them, the composer opening and closing it, and
Connect-your-tools on Apps. The Overview stop is gone rather than re-anchored —
a tour that teaches a destination an operator cannot then find is worse than one
that leaves it to be discovered where it lives.

`test/unit/tour-anchors.test.ts` is the check the tour never had. It asserts
every stop's view against `VIEWS` and every anchor against the rendered nav
table, so the silent-skip case is a failing test rather than a quieter product.

**Standing rule: an IA change that moves or removes a nav row updates the tour in
the same change.** There is no test that will tell you afterwards, unless it is
that one.

## Where this leaves the older records

- Rule 2's reasoning about a row per declared list is the argument this file
  generalises. Nothing in it is superseded.
- Rule 6 is unchanged and is now exercised by eight views rather than five.
- Rule 7's Connections section survives. Its rail moved into the sidebar and
  came back to the content area as the shared one — through both moves the
  section itself stayed dispatch-only, which is the property that mattered.
- `finance-console.md` still describes Finance's sub-pages correctly. They are
  nested rows on Company's rail rather than a rail of their own; the pages,
  their addresses and their order are unchanged.
