# Ledger status vocabularies

How many states a ledger may declare, and what happens to the words a narrowing
retires. Split out of [`ledgers.md`](./ledgers.md), which owns everything else
about how a ledger is declared, folded and rendered.

## The board: why three, and not six

Six lifecycle states is the right number for the runtime and the wrong number
for a reader. The board asked every agent and every operator to hold a six-word
vocabulary in which four of the words mean some shade of *a teammate has started
this*, told apart by which machine is currently owed something — a distinction
the runtime needs and nobody else does. What that bought was agents filing cards
`in_review` when they meant paused, filing work as `planning` because a plan
existed, and reading a rendered board they could not summarise.

So the four middle stages are one column now:

| phase | stages it covers | closed |
| --- | --- | --- |
| `pending` | `todo` | no |
| `working` | `planning`, `in_progress`, `paused`, `in_review` | no |
| `done` | `done` | yes |

Nothing is lost. The stage rides on the row — a `stage` field in
`derived/tasks.md`, `Task.stage` on the wire, a badge beside the status on the
card — so *waiting on your verdict* is still visible where it matters, as a
property of a working card rather than as a fourth pile to file it into. The
console reads the stage for everything genuinely stage-specific: Resume on a
paused card, the review link on one waiting for a verdict.

**Writes take a phase.** A drop sends `working`, and the write boundary resolves
it to that phase's `entry_stage` — `in_progress`, which dispatches. Stage words
are still accepted (the runtime's own paths speak them, and so does every stored
card) but the refusal names only the three, because a caller who guessed wrong
should be learning the small vocabulary rather than the large one.

**Planning is an act, not a column.** Dragging a card into Planning was the one
console route to a planning pass, and three columns has no drop target for it.
It is a *Plan first* control on the card instead, which is where an act belonged
rather than a state. It writes the `planning` stage and spends exactly what the
drag spent.

## Three statuses, everywhere

Every built-in ledger declares exactly three statuses, and
`no_built_in_ledger_declares_more_than_three_statuses` is what keeps it that
way. The board is the loudest case, but it was not the only one: `goals`
declared six and `decisions` four, and both were narrowed for the same reason.

| ledger | statuses | what went, and why |
| --- | --- | --- |
| `tasks` | `pending`, `working`, `done` | four stages that all meant "started, not finished" |
| `goals` | `active`, `met`, `dropped` | `proposed` (a goal nobody committed to is a chat message), `at_risk` (that is what `progress` says, and nothing ever moved a goal back out of it), and the `met`/`missed` split (both are "this is over"; which one is the first clause of the required reason) |
| `decisions` | `proposed`, `accepted`, `retired` | `superseded` and `reversed` — one status wearing two words, told apart only by the reason both already require |

The rule behind all three: a status answers *where does this row stand*. Every
fourth status was answering a second question in the same place — how is it
going, who is owed something, which flavour of over — and asking a writer to
encode two answers in one word buys a coin-flip, not information. The second
answer goes in a field: `progress` on a goal, `reason` on a closed row, `stage`
on a working card.

**Retired words heal on read.** A row stored as `at_risk` or `superseded` is a
row somebody wrote, and a spec that simply stopped declaring the word would
report it as a fault *and* leave it out of the rendered file, since sections
select by status. So each surviving status names the words it adopted
(`StatusSpec::aliases`), `Entry::status` resolves through them, and the row
renders and counts under the survivor. A **write** of a retired word is still
refused — the same reads-heal/writes-fail asymmetry `LEGACY_COLUMN_BACKLOG`
already makes for a stored card, and for the same reason: a client should learn
the surviving vocabulary once rather than be kept quietly on the old one.

### Five for authored ledgers

The shipped templates get one notch looser, and a test holds them there
(`no_shipped_template_ledger_declares_more_than_five_statuses`, over both
`globals/ledgers/` and `companies/*/ledgers/`). A template ledger is a
*pipeline* far more often than a built-in is — a candidate, a deal, a filing
genuinely moves through stages — and three would have forced each one to throw
away either its pipeline or its outcomes.

Five leaves room for both. What it forbids is the sprawl these started at: seven
statuses, four of which an agent had to choose between on every write with
nothing to tell them apart but a blurb. Past five, the extra status is reliably
answering a second question — how is it going, which flavour of over — and that
answer belongs in a field, where it does not have to be guessed:

| what was merged | where the distinction went |
| --- | --- |
| `at_risk` into the active status (commitments, engagements) | the row's own progress line — a status saying how something is *going* is one nothing ever moves back out of |
| `stalled`, `on_hold`, `paused` into the working status | same |
| a third closed outcome into a second (`extended`+`not_required` → `not_filed`, `passed`+`lost` → `not_bought`) | `reason`, which is required on close and is what a closed row is read for |
| adjacent pipeline stages (`hit_finding`+`lead_optimization` → `discovery`) | nothing — they were one stage under two names |

Where a file argued for a distinction it had, that argument won: `deals` in
enterprise sales keeps `no_decision` separate from `lost`, because its own blurb
says collapsing them hides that the competitor was inertia, so its **pipeline**
compressed instead.

Every retired word is an alias on the status that adopted it, so no stored row
moves or disappears.

A ledger a company declares at run time is not capped — the wizard's presets are
three, and the argument above is an argument, not a mechanism. What is
guaranteed is that nothing OpenCompany ships asks a reader to hold more.
