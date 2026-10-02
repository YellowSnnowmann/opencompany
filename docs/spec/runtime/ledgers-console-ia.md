# Ledgers console redesign: naming and information architecture

Filed as issue #1284, from a design discussion between the operator and their
assistant. This is a **surface** redesign only. The engine
(`docs/spec/runtime/ledgers.md`: declared `LedgerSpec`, the append-only fold,
bounds-are-code, one derived Markdown file per list) does not change — nothing
here alters `POST …/ledgers`'s request or response shape, the fold, or the
board's drag-and-drop. What changes is what the operator sees: the word they
read, where each list sits, and how a new one is declared.

## Why: three surface problems, one engine

1. **"Ledger" reads as a financial record** to almost anyone who did not build
   it. What it actually means — any tracked list of rows with a status
   (goals, decisions, risks, a hiring pipeline, customer promises) — is a far
   more useful concept than the name suggests, and the mismatch makes a
   first-time operator skip past the feature that would have answered "where
   do we track X?"
2. **`tasks` gets a nav row of its own; every other list does not.** `tasks`
   is a ledger under the hood (`LedgerSource::Native`,
   `docs/spec/runtime/ledgers.md#the-task-board-is-the-tasks-ledger`) but the
   console still treats it as special-cased chrome around the board, while
   `goals`, `decisions`, and anything a company declares are buried one level
   down inside a single "Ledgers" nav item, picked from an in-page list.
   Nothing about the engine draws that line — it is a console artifact left
   over from before issue #1140 merged the Tasks page into `LedgersView`.
3. **Declaring a list means authoring `fields[]`/`statuses[]` as JSON.** An
   operator who wants to track "customer promises" thinks in terms of "make a
   list," not "declare a schema."

## Rule 1: "Ledger" is an internal word only

Every user-facing surface names a list by its own title — **Tasks**,
**Goals**, **Decisions**, or whatever a company called something it declared.
"Ledger" does not appear in a nav label, a page heading, a button, a dialog
title, an empty state, a toast, or help copy anywhere in the console.

It stays exactly where it already is useful: module and file names
(`src/ledger/`, `LedgerSpec`, `LedgerStore`), Rust and TypeScript type and
function names (`LedgerSummary`, `defineLedger`, `listLedgers`), route and API
shapes (`…/ledgers`, `#/ledgers/<slug>`), code comments, and this design
corpus. None of that is operator-facing, so none of it needs to change, and
renaming it would cost a mechanical diff across the engine for zero surface
benefit — precisely the "None of that needs to change" instruction issue
#1284 gives.

A concrete before/after for the strings that do move:

| today | becomes |
| --- | --- |
| nav item "Ledgers" | nav item **"Work"** — the hero surface below, see Rule 2 |
| "New ledger" button | "New list" (moved — see Rule 3) |
| "Declare a ledger" dialog title | wizard, titled by its first step (see Rule 4) |
| "This ledger leaves this screen…" (retire confirm) | "This list leaves the sidebar…" |
| "This company has no ledgers yet." | "This company has no lists yet." |
| "Rows here are opened elsewhere" banner | unchanged in substance, reworded off "ledger" |

## Rule 2: one nav row, a Tasks-hero page, a switcher on its own title

This rule went through four drafts before landing; the first three are kept
below because the reasoning that ruled them out is the part worth not
re-litigating.

### Draft 1 (rejected): every list its own sidebar row

The first cut took "no list should be a click further from the sidebar than
any other" literally: `NAV` stopped being a fixed array and grew one row per
list the company held, spliced in where the single "Ledgers" row used to sit.

It did not survive contact with the cap. A company may declare up to
`MAX_DECLARED` = 12 lists (`src/ledger/spec.rs`) on top of the 3 built-ins
(`tasks`, `goals`, `decisions`) — 15 list rows, sharing the sidebar with the 9
other fixed `NAV` entries (Overview, Company, Chat, Work, Workspace,
Approvals, Finance, Workflows, Settings). Twenty-four rows in one sidebar is not a
list of destinations an operator scans, it is a wall. The draft read fine
during review only because it was checked against a demo company holding
exactly three lists — the cap itself was never rendered and never looked at.

### Draft 2 (rejected): a collapsible "Lists" section

The second cut tried to keep flat, always-visible rows while bounding their
count: a collapsible sidebar section, one header row, with every list but
Tasks as an indented, persistently-collapsible child. Tasks stayed a
top-level row outside it.

This was abandoned before it shipped, for a reason upstream of scaling: a
sidebar section — collapsible or not — is still built on the premise that an
operator's daily relationship to a declared list is *finding it in the nav*.
That premise does not hold. `goals`, `decisions`, and anything a company
declares are `LedgerSource::Events`, written almost entirely by **agents**
through `record_entry`/`close_entry` (`docs/spec/runtime/ledgers.md`); a
person's own relationship to them is closer to "check what we decided" than
"work out of this every day." `tasks` is the one genuine exception —
`LedgerSource::Native`, its rows live in the task store, its columns fire
dispatch, and an operator works out of it constantly. Reserving permanent nav
real estate for surfaces that are consulted rather than worked was solving
the wrong half of the original complaint. The actual defect Draft 1 correctly
identified — that reaching Goals or Decisions meant an in-page picker of bare
cards that said nothing about what was in them — was never "not enough nav
rows"; it was "the one existing entry point told you nothing useful before
you clicked."

### Draft 3 (rejected): a tab strip

The third cut kept one nav row, relabeled "Work", opening a single screen with
Tasks first and selected by default and every other list as a tab beside it —
`[ Tasks 12 ] [ Goals 3 ] [ Decisions 1 ] [More▾]` across the top, tabs that
did not fit the measured width collapsing behind a "More" menu.

This was built, tested, and verified at the 12-declared-list cap (the strip
degraded correctly, in both themes, at both a normal and a narrow viewport) —
and still replaced before it shipped. It solved scaling honestly, at the cost
of a permanent new band across the top of a screen an operator opens
constantly: every visit to Tasks now paid for the existence of eleven other
lists it does not care about that day, in vertical space, even when nothing
overflowed. A control that pays a fixed cost on the operator's *most-visited*
screen so that *occasionally-visited* lists are one click away is optimizing
for the wrong side of that trade.

### What ships: the switcher lives on the page's own title

Instead of adding a band above the page, the page's own `<h1>` becomes the
control: `Tasks ▾`, with the chevron the only visual signal that it is
interactive rather than decoration. Clicking it opens a menu listing every
list the company holds — Tasks included, marked as the current one — each
with its own open count:

```
┌────────────────────────────────────────────────┐
│  Tasks ▾                          [+ Add task]  │
│  The company's work board…                      │
│  [ search ]                                      │
│  [ board ]                                       │
└────────────────────────────────────────────────┘
        ┌──────────────────┐
        │ ✓ Tasks       12 │
        │   Goals        3 │
        │   Decisions    1 │
        │ ─────────────────│
        │ + New list        │
        │   Manage lists    │
        └──────────────────┘
```

This adds **no new element** to the page — the title was always going to be
there — and it costs the operator **one click** to reach any other list
(open the menu, pick one) against the tab strip's two-to-three (find the tab,
possibly open "More", pick one), while scaling identically past any width a
strip could run out of: the menu is a list, and a list scrolls. It also
naturally answers what a tab strip and a flat "go elsewhere" button both get
slightly wrong — a switcher's menu includes *where you already are*, marked,
rather than only offering somewhere else to go.

**Open counts stay in the menu**, for the same reason they were on the tabs:
without them, opening the switcher is a bare list of names, and a menu with
nothing to weigh before clicking is barely better than the picker-of-cards
Draft 1 and 2 were both trying to get away from.

**`+ New list` and `Manage lists` sit at the foot of the menu**, after a
separator — the same two actions Rule 3 already gives their own settings
surface, reachable now from wherever the operator happens to be looking at a
list rather than only from the Company page.

**Why "Tasks" and not "Work" now.** The tab-strip draft needed a
Ledger-free, Tasks-agnostic nav label because the nav row and the hero
content were two different things wearing one name. A switcher removes that
tension: the page's title is always exactly what is on screen, so it is
allowed to say "Tasks" — the default and by far the most common state — and
change to "Goals" the moment that is what is showing. `NAV`'s own row keeps
whatever label gets an operator here in the first place (kept as "Work", not
reverted, so the sidebar and the address bar do not have to agree on a
running title); the page's `<h1>` is a different, more honest thing.

**Routing is exactly what Draft 3 already established, unchanged again.**
`#/ledgers/<slug>` names any list; a bare `#/ledgers` resolves to Tasks;
`#/ledgers/<slug>` opens directly on that list, deep link or not; back/forward
and reload all work because the switcher's only side effect is the same
`navigate()` every other address in this console already uses. `#/tasks/<id>`
— the card detail page kept alive since issue #1140 — is a different route
entirely and this redesign has never touched it, draft after draft.

**Furniture trimmed while rebuilding the header.** The pre-switcher page
stacked title → purpose → `Renders into derived/<NAME>.md` → a lock notice
naming the tool calls that write a native list (`spawn_task`, `assign_task`)
→ search → the board — six bands before the thing an operator opened the
page for. The derived-file path and the tool-name notice are accurate but
developer/agent-facing detail, not something an operator reads on a screen
they open constantly; both moved behind a small disclosure next to the
title rather than being deleted, so the information is still one click away
for whoever is debugging a derived file or wiring a manifest.

**What happened to the measured-overflow work.** Draft 3's `lib/overflow-
tabs.ts` (the pure "how many tabs fit" decision) and `hooks/use-overflow-
tabs.ts` (its `ResizeObserver`-based measurement) existed to solve a problem
a dropdown does not have — fitting items into a bounded strip width — and
neither is consumed by what shipped. Removed with it, rather than kept on a
"might be useful later" basis: unused measurement code with no caller is a
maintenance cost with no offsetting benefit, and the reasoning for *why* it
does not apply here is preserved in this paragraph, not in a comment on dead
code nobody will read next to the code that replaced it.

## Rule 3: declaring and retiring live in Work, not Company

Today "New ledger" is a button in `LedgersView`'s own toolbar, reachable
the moment an operator opens what used to be the single Ledgers page. A
list's own screen (Rule 2) is for *working its rows* — putting a "New list"
control there would mean an operator looking at Goals sees a button for
creating an unrelated new list, which is a settings action wearing a
data-page's chrome.

**First cut, corrected.** The original version of this rule put Manage Lists
under the Company page — "the same precedent `CompanyView` set for desks:
`#/company/desks` for desk creation, so `#/company/lists` for list
creation." That analogy does not hold. A desk **is** company structure, so
managing desks belongs on the page that already shows the org chart. A list
is a work record, and — because the switcher (Rule 2) is the only real entry
point to any of them — an operator declaring or retiring one is reached
almost entirely *from Work*, not from Company. Routing that flow through
Company meant every visit crossed a section boundary and came back
(Work → Company → Work), which read as arbitrary because it was: the
placement modeled the wrong analogy.

**What ships:** Manage Lists lives inside Work, at `#/ledgers/manage` — a
segment `LedgersView` reserves (`MANAGE_SEGMENT`) the same way
`CompanyView.DESKS_SEGMENT` reserves `desks`, checked in `app-shell.tsx`
*before* `LedgersView` itself ever mounts, since its own hooks read and write
real list rows keyed on `sub` and would misfire against a slug that names no
list. The switcher's menu is the only entry point — no Company-page button
duplicates it — and its own "Back" returns wherever the operator actually
came from (`history.back()`, not a fixed destination), since that screen no
longer has exactly one canonical parent.

- Manage Lists shows every list the company holds — built-in and declared,
  including `tasks` — each with its title, purpose, row counts, and whether
  it is retireable. `tasks`, `goals`, and `decisions` cannot be retired (they
  are not `LedgerSummary.builtin === false`), so the row shows why rather
  than hiding the control inconsistently with how Manage Desks always shows
  every desk.
- **Declaring**, specifically, has two paths now, both producing the same
  `LedgerSpec` (Rule 4): Manage Lists' own "New list" button, for an operator
  already browsing the full roster; and the switcher's own "New list" menu
  item, which opens the wizard **in place** — layered over whatever list was
  already on screen, no navigation at all. The in-place path is the one that
  matters day to day: it is reachable from any list's title in one click,
  and it is what makes "declare a new axis" cost the same whether an
  operator is already on Manage Lists or three tabs away from it.
- A list's own page keeps the two other things `docs/spec/runtime/ledgers.md`'s
  console section already documents as deliberately visible rather than
  hidden: row-level delete (person-only, "Close" offered first), and a native
  list's `writtenBy` sentence in place of a compose box.

**The in-place wizard needs a URL, or Back breaks it.** Local component state
with no history entry meant the browser Back button, pressed while the
wizard was open, skipped past both the wizard *and* Manage Lists in one jump
— the exact "moves so randomly" failure this correction exists to fix.
`hooks/use-hash-flag.ts` is the fix: a boolean riding the current hash's
*query* suffix (`#/ledgers/goals?new`), which `useHashView`'s own segment
parsing already ignores (it strips everything from `?` onward), so it
coexists with the ordinary `view`/`sub` routing rather than replacing it.
Setting it is a real `window.location.hash` assignment — a genuine history
entry — so popping it via Back is an ordinary `hashchange` the hook already
listens for. The wizard's five internal steps do not each get their own
history entry; only "is the wizard open at all" needs to survive Back.

## Rule 4: the declare dialog becomes a plain-language wizard

The current `DeclareDialog` (`frontend/src/views/LedgersView.tsx`) is a
`Textarea` seeded with a worked JSON example (`TEMPLATE`) and a "Declare"
button that calls `JSON.parse` on whatever the operator typed. Its own doc
comment says why it was built that way — "the declaration is small, the field
roles matter, and a wizard that produced a subset of what a teammate's
`define_ledger` can produce would leave the console unable to express a
ledger it can display." That tradeoff is real: `LedgerSpec` supports roles
the wizard below does not surface (`refs`, `number`, custom `sections`,
`checks` beyond the built-in three). The wizard is deliberately a **curated
subset** of what the wire format allows, not a full re-expression of it — an
operator who needs `refs` or a custom section order still has the same POST
body reachable by whoever builds agent tooling against `define_ledger`
directly; the console's wizard optimizes for the common case a human names in
one sentence.

The wizard replaces the JSON editor with four steps, each producing a piece of
the same `LedgerSpec` the host already accepts:

1. **What do you want to track?** — free text, becomes `purpose`. ("What we
   promised a customer, and whether we did it.")
2. **Name it** — free text, becomes `title`; the `slug` is derived from it
   (lowercase, `-`-joined, checked against the existing registry the way
   `OrgChartView` slugifies a desk name) with the slug editable if the
   derived one collides or reads badly.
3. **What stages does a row go through?** — becomes `statuses[]`. Two
   presets front and center:
   - **To do / In progress / Done** (`todo`, `in_progress` → `done`, closed,
     `needs_reason` off — a task-shaped list)
   - **Open / Closed** (`open` → `closed`, closed, `needs_reason` on — an
     event-shaped list, the shape `TEMPLATE`'s `customer-promises` example
     already uses for kept/broken)
   plus a **Custom** path: add named stages, mark which end the row
   (`closed: true`) and which of those need a reason
   (`needs_reason: true`) — the same two flags the JSON template already
   sets by hand, asked as two checkboxes per stage instead.
4. **What details does each row need?** — becomes `fields[]`. A title field
   is implicit and always present (`role: "title", required: true`, plus the
   `id` and `status` roles the engine always needs — the wizard fills those
   without asking). Presets offered as toggles: **Owner** (`role: "owner"`),
   **Notes** (`role: "prose"`), **Due date** (`role: "date"`) — the three
   `TEMPLATE`'s worked example already reaches for beyond title/status/reason
   — plus **Add a custom field** for anything else, asking for a name and
   picking from the same role list `FieldRole` already exposes
   (`frontend/src/api/ledgers.ts`). A closing reason field
   (`role: "prose"`, conventionally named `reason`) is added automatically
   whenever step 3 marks any status `needs_reason` — the wizard does not ask
   for it separately, since a status that needs a reason and a spec with
   nowhere to write one is exactly the mistake `TEMPLATE`'s own comment
   flags as "the commonest mistake."

A final review step shows the assembled plain-language summary ("Customer
promises, tracked open → kept/broken, each row has a customer and a due
date") rather than the JSON — the wizard's job is to make the *shape* legible
without asking the operator to read `LedgerSpec` to check its own work — and
submits the same object `defineLedger()` already POSTs today. `sections` is
assembled by the wizard, not asked about: one section per non-closed status
group (open stages) plus one "Settled"-style section for the closed ones,
mirroring the shape `TEMPLATE`'s own `Outstanding`/`Settled` split already
uses, so the operator never has to think about section headings to get a
working list. `checks` is fixed to
`["required-field", "known-status", "closed-needs-reason"]` — the same three
`TEMPLATE` ships — since nothing in the wizard's four steps produces a
`LedgerSpec` those three would ever reject.

## Rule 5: the board and drag-and-drop are untouched

`LedgerBoard` (`frontend/src/views/LedgerBoard.tsx`), the board/list toggle,
`patchTask` for the native board vs. `record_entry` for every other list, the
empty-column rail collapse (issue #1101), and the drag mechanics (issue #334)
are unchanged by this redesign. A list's own screen still renders through
that one shared component; only how the operator arrives at that screen
(Rule 2) and how the list came to exist (Rules 3–4) move.

## Rule 6: navigation and routing are separate promises

A sidebar row answers “where should an operator start?”; a route answers
“what address can still be opened?” They are deliberately different. A view
without a `NAV` row in `frontend/src/components/app-shell.tsx` must fit exactly
one of these treatments:

- **Discoverable elsewhere.** Feedback has no main-nav row because the sidebar
  footer links to it.
- **A deep-link destination.** Bare `#/tasks` and `#/team` redirect to their
  successors, while their detail addresses stay routable because cards and
  teammate links name them. Agent-authored Pages can likewise be intentionally
  direct-URL-only.
- **Parked but reachable.** A complete operator surface can keep its route,
  host API, and tests while it is absent from navigation. It must render a
  persistent, plain-language notice explaining that it is not in console
  navigation, that its data remains live, and how it can be reached. Inbox is
  the current example.
- **Retired.** Remove the render surface and route it through
  `REWRITE_RETIRED` to the real successor. Do not leave a complete page
  pretending to be a live navigation destination when it has neither a
  discoverable entry point nor a parked notice.

`VIEWS` in `frontend/src/lib/console-routes.ts` is the routing allow-list, not
the navigation model: no surface gains or loses an address merely by adding or
removing a sidebar row. A future change that parks a complete surface must add
its visible notice in the same change; a future change that retires one must
replace the route deliberately.

## Rule 7: Connections is a section, not two settings tabs

Added after the fact, by the change that moved OAuth and MCP Servers out of
Settings. Recorded here because this file says at the bottom that it is the one
to update first when the console IA changes.

**What moved.** Two settings sub-pages became a top-level nav row with two
sub-pages of its own:

| address | before | after |
| --- | --- | --- |
| `#/connections` | rewritten → `#/settings/oauth` | **real** — the section, defaulting to Apps |
| `#/connections/apps` | — | the accounts page, renamed from OAuth |
| `#/connections/mcp` | — | the MCP page |
| `#/oauth` | → `#/settings/oauth` | → `#/connections/apps` |
| `#/mcp` | → `#/settings/mcp` | → `#/connections/mcp` |
| `#/settings/oauth` | the page | → `#/connections/apps` |
| `#/settings/mcp` | the page | → `#/connections/mcp` |
| `#/settings/connections` | **dead** — silently landed on General | → `#/connections` |

**Why a section and not a settings tab.** The same argument
`docs/spec/runtime/finance-console.md` makes about Billing, and the one that
took Brain out of the rail. Settings is where an operator changes how the
company is configured — a place they visit once, on the way to something else.
Which apps the company can act through, and which tool servers its teammates can
call, is not that: it is read repeatedly, it changes as the work changes, and an
operator arrives at it asking "can my teammates do X yet?". Two clicks down a
settings rail is the wrong depth for a question asked that often.

**Why only two of the five.** Inference, Hosting and Search stayed in Settings'
Integrations group, which survives with three rows. Each is a credential form
that belongs beside the one thing it unlocks — the model, the deploy target, the
search provider — which is the argument `views/settings-pages.ts` makes twice.
Filing them under a section named for the act of connecting would undo it.

**This is not a revert of the Connections split.** A single "Connections" *page*
once carried five subjects — accounts, MCP, inference, channels, repositories —
and was deliberately broken apart because each was something an operator
scrolled past on the way to another. That decision was about one question per
page, and it stands: Apps and MCP Servers are still two pages. What is new is
that they have a parent. Composio stays on the Apps page for the same rule read
the other way: `ProvidersSection` reads Composio's credential state to decide
what every provider tile renders, so splitting them would separate a credential
from the thing it unlocks.

**Why "Apps".** "OAuth" names the protocol a connection happens to run on rather
than what an operator is looking for, which is Gmail, Slack, Notion — and under
a parent already called Connections it said the same word twice. Only the
surface strings change; the file, the component and every internal name stay
`OAuth*`, exactly the split Rule 1 makes for "ledger".

**Rule 6 is satisfied by promotion, not by an exemption.** Both pages gained a
nav row rather than losing one, so neither needs one of Rule 6's four
treatments. What Rule 6 governs here is the addresses they left behind: all four
are **retired** in its sense — routed through `REWRITE_RETIRED` to the real
successor, with no page left pretending to be a live destination.
`#/settings/connections` is the case that shows why that clause is not
bookkeeping. It named no `SETTINGS_PAGES` id, so the unknown-sub repair dropped
the operator on **General** — a real page that was not the one the link
promised, which is worse than an address that plainly fails, because it looks
like it worked. It was live in four places in the setup flow the whole time.

**The nav row sits between Finance and Workflows.** Finance and Connections are
the two rows about the world outside this company — the money it moves through
Chargebee and PayPal, and the apps and tool servers its teammates act through.
Workflows and Observatory are the "what the agents did" pair and read as one
thing when adjacent. Draft 1 above counted 9 fixed `NAV` entries against the
15-list cap; this makes 10, which does not change that draft's arithmetic enough
to reopen it.

## Rule 8: the sidebar is four sections, and sub-navigation lives in it

Ten flat rows became four — Room, Company, Connections, Automations (once
"Flows") — and then two: the conversation list now fronts the sidebar, with
**Company** and **Connections** pinned at its foot and Automations a row on
Company's rail. A section's pages are the first column of its content area.

The full record is [`console-sections.md`](console-sections.md): why four, why
why the block is fixed rather than an accordion, how the channel list is moved
whole by portal and became one flat list, what happens on the collapsed rail, what Room
does at twenty channels, and the **nine Rule 6 calls** for the views that have
no nav row. This file names the rule and points at it, because it is at its
500-line ceiling and that is the split the repo's own guideline asks for.

Three things belong here rather than there, because they change what the rules
above mean:

- **Rule 2's argument generalises.** A row per declared list was rejected as "a
  wall"; ten fixed rows are the same wall arrived at one row at a time. Rule 8
  is that judgement applied to the whole column.
- **Rule 6 now governs nine views, not five.** Overview, Approvals and
  Observatory join it. Each takes a named treatment; none is left implicit.
- **Rule 7's Connections section survives; its rail does not.** Sub-navigation
  moved into the sidebar for all sections at once, so the console never
  carried two patterns. Finance's rail is the last one and is the obvious next
  conversion.

**An IA change that moves or removes a nav row updates the guided tour in the
same change.** A missing tour anchor is *skipped*, not reported, so a stale tour
degrades to teaching half the product with nothing failing.
`test/unit/tour-anchors.test.ts` is what makes that loud.

## What stays out of scope

- `LedgerSpec`, the fold, `LedgerStore`, the derived-Markdown guard, and every
  Rust type under `src/ledger/` — unchanged.
- The five agent tools (`list_ledgers`, `read_ledger`, `record_entry`,
  `close_entry`, `define_ledger`) and their schemas — unchanged. The wizard is
  a console-only path to the same `define_ledger`/`POST …/ledgers` call a
  teammate's tool call already reaches.
- `[[agent]].ledgers` manifest semantics — unchanged.
- Row-level behavior on a list's own screen (search, status filter, compose,
  delete) — unchanged, only relabeled per Rule 1.

## Where the engine doc still applies

`docs/spec/runtime/ledgers.md`'s "## The console" section describes how a
list's *own* screen renders from its `fields`/`statuses`/`sections` (the
board/list duality, the task-card slot, the empty-rail collapse, the two
board specs) — all of that remains accurate after this redesign and is not
duplicated here. What that section's opening paragraphs describe as reached
through "The Ledgers section" is superseded by Rules 1–3 above: there is no
longer one section named Ledgers, and this document is the one to update
first if the IA changes again.
