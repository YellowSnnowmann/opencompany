# The ledger console: rendering a list's screen

Split out of `ledgers.md` when that file outgrew the 500-line cap enforced by
`scripts/ci/assert-md-line-cap.sh`. The engine spec, declared `LedgerSpec`,
the append-only fold, bounds-are-code and one derived Markdown file per list,
stays in [ledgers.md](ledgers.md). The console-facing IA, naming and sidebar
placement is the separate `ledgers-console-ia.md` (issue #1284).

## The console

The console-facing IA — naming ("ledger" stays an internal word only), how a
company's lists reach the sidebar, where declaring and retiring one lives, and
the plain-language wizard that replaced the JSON declare dialog — is
`docs/spec/runtime/ledgers-console-ia.md` (issue #1284). What follows here is
how a list's own screen renders, which that redesign left unchanged.

Each list's screen renders from its own `fields`, `statuses` and `sections`,
never from anything hard-coded. A list a teammate declared this morning
renders correctly this afternoon with no console release; a screen that
hard-coded the goals columns would have made "declare your own axis" a promise
the UI quietly broke.

Two things it shows rather than hides. The delete control exists here and nowhere
an agent can reach, with **Close** offered first as the ordinary way to be
finished with a row. And a native ledger renders its `writtenBy` sentence in
place of a compose box, rather than offering a form whose save the host refuses.

The compose form reads `needsReason` off the declaration and asks for the reason
*before* the save — the same rule the host enforces, met earlier.

### One board component, one screen

Any ledger with statuses renders as columns — one per status, in declaration
order, labelled by the host. That falls out rather than being designed: a status
list *is* a lifecycle, and the only thing that differs between the board and a
hiring pipeline is which call a drop makes. A native ledger's drop goes through
`patchTask`, because entering a column fires work; every other ledger's is an
ordinary `record_entry` merge. A drop into a status that demands a reason opens
the compose form instead of writing, since the host refuses a silent close.

`views/LedgerBoard.tsx` is that board, and it is the **only** one. It owns the
columns, the counts and the drag mechanics; the card is a `renderCard` slot.

There was a standalone Tasks page beside Ledgers until issue #1140, rendering
the same records through this same component, and an operator who met their work
twice had to learn which of the two was the real one. Now one screen uses the
board, and it chooses its card by what is behind the row:

| rows | card |
| --- | --- |
| the `tasks` ledger's rows, joined to `Task` records from `…/tasks` | `views/TaskCard.tsx` — priority, assignee, cost, plan badge, output link, and a paused card's blocker and Resume |
| any other ledger's `LedgerEntry` rows | title, owner, id |

**The card is a slot, not a role-driven renderer.** That was tried and is wrong:
a task card carries a priority, a plan badge, an output link and a Resume
button, none of which is a ledger field and all of which come off the `Task`
record rather than the ledger's projection of it. A generic renderer would have
had to drop them or grow a special case per ledger, and the second is the slot
with more steps.

Which column a row is in is a `statusOf` accessor rather than a required
property, because the two sides spell it differently — a `Task` says `column`,
a `LedgerEntry` says `status` — and a board renaming its callers' fields is a
board deciding what their data is called.

The history here is worth keeping, because it is the argument for the shared
component. The board screen was first deleted and re-implemented inline in the
Ledgers section, and that re-implementation silently lost all three of issue
#334's fixes: the edge auto-scroll, the miss message, and the trailing gutter.
Nothing failed — there was no test on the new path — until the ported drag spec
was actually run. The board is one component now so that cannot happen again,
and #1140's deletion is the proof: retiring the screen a second time moved a
`renderCard` and a route, and touched none of the gesture.

#### An empty column collapses to a rail

Six columns are wider than an ordinary window, and the three that fit are the
three a working company empties first. So an operator opened the board, read
three confident zeros, and never learned that the hundred cards they came for
were two columns off the right edge (issue #1101): the board that had moved the
most work looked the most finished.

A column holding nothing therefore renders as a ~40px rail — label set
vertically, count beneath it — and the populated columns take the room back.
Three rules keep that from breaking the gesture the board exists for, since the
empty columns are precisely the ones a card is dragged *into*:

* **A rail is a whole drop target.** The drop handlers sit on the column
  wrapper, which is the element that shrinks.
* **A rail opens under a drag.** The `over` state that draws the drop highlight
  also suppresses the collapse, and the empty placeholder becomes a dashed
  "Drop it here" while a card is in hand.
* **Nothing collapses unless another column is populated.** Six rails and no
  board would answer "show me the work" worse than the zeros this fixes.

Clicking a rail pins it open — a real button, with the column and its count in
its accessible name — and a pinned column stays open until the operator folds it
back. A column that re-collapsed itself while somebody was reading it would be
this bug's mirror image. A column whose `columnHeader` slot holds a control
never collapses either: a rail has nowhere to put it, so folding one would hide
an affordance rather than some whitespace.

### What the ledger drives, and what it does not

**Drives**: the columns, their order, their labels, which one closes a card, and
therefore what "outstanding" counts. The console declares no column.

**Does not drive**: what is *on* a card. A task is a `Task` — a priority, an
assignee, a plan brief, a published output, a deliverable kind — and the
ledger's native projection deliberately does not carry any of it
(`src/ledger/native.rs`). So `LedgersView` reads `…/tasks` alongside the ledger
and joins the two by id: the ledger supplies the shape of the board and which
column a card is in, and the task store supplies what is on it. A row whose
record has not arrived yet renders as an ordinary ledger row rather than
waiting, so a card never flickers in and out of its column.

Creation keeps its own dialog (`views/CreateTaskDialog.tsx`) rather than becoming
a ledger compose form: the board's write path is `POST …/tasks`, and
`record_entry` is refused for this ledger precisely because entering a column
fires work. It is offered on the board and nowhere else, and it lands a card in
Pending — the one column new work may enter.

`#/tasks/<id>` remains the card detail — a timeline, a plan brief, a discussion,
its attempts, a workflow proposal, the steer controls — none of which has
anywhere to live on a column, and none of which Ledgers tries to reproduce. It
is the one address that outlived its page: `views/TaskDetailRoute.tsx` mounts it,
`tasks` stays in the shell's `HIDDEN_VIEWS` so the router still knows the head,
and `#/tasks` with no card is rewritten to `#/ledgers/tasks` rather than
resolving to Overview.

The board is covered end to end by two specs at that one address:
`board-columns.spec.ts` drives its shape (columns, order, intake, and the two
addresses #1140 had to keep), and `board-drag.spec.ts` drives the gesture (the
three #334 fixes).
