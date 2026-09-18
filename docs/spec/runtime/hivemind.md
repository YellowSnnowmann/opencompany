# Hive-mind desks

A desk with somebody to deliberate **with** answers an operator message as a
room rather than through one responder.

Before this, a message addressed to a `[[group_chat]]` selected exactly one
agent off a deterministic ladder — a `@mention`, else an `auto` channel's
per-message pick, else the desk lead — that agent took one turn, and the
interaction ended. That is still the whole story for a desk of one, a DM, the
General line, and a workflow copilot thread. For a desk of two or more it is
now the fallback rather than the rule: the message opens an **episode**, and
the episode runs a bounded sequence of **rounds** until the room converges on
an option, deadlocks between two, spends its turn budget, or finds it has
nothing to say.

The mechanics come from [`tinyhivemind`](https://github.com/tinyhumansai/tinyhivemind),
vendored at `vendor/tinyhivemind` and linked unconditionally: it is pure Rust,
executor-neutral, and adds no port. `tinyhivemind_hive::step` is a fold over a
transcript this host already holds; it never appends, never waits, and never
calls back.

## One message is still a bounded number of turns

An episode is not a fan-out. `step` authorizes a **round** — at most
`round_width` speakers while the room is blind, at most `revealed_width` once it
can see itself — so the number of turns an operator message can start is bounded
by the desk's turn budget and by nothing else, the same bound a single-responder
desk has at 1. A wider round spends that budget in fewer rounds; it does not
raise it.

A width bounds what the episode *authorizes*, not how this host runs it. The
driver takes a round's turns in series and commits the round's `next_state` only
once every one of them is durably appended — committing part-way through would
charge a threshold nobody spent. Turns in one round cannot read each other:
each is projected against the single transcript `step` decided on, and the
pinboard is read under the same round bound so a round-mate's `!pin` cannot
reach a peer's prompt either.

What a room buys over one responder is not parallelism. It is:

- **Independence.** The opening round is *blind*: a member forms its own view
  before it can read its peers'. A shared transcript destroys independence —
  the third speaker has already read the first two — and this is the cheapest
  available repair, costing a projection flag rather than any concurrency.

  **What a blind member is asked for is a deposit, not a position**, and that
  distinction is load-bearing rather than stylistic. The prompt used to say
  "form your own first". On any question where members hold correlated priors
  and one member holds the decisive fact — which is what a desk of specialists
  *is*, since the fleet technician holds machine facts nobody else does — that
  instruction is fatal: every member opens by advocating what its own reading
  favours, a proposal counts as its own author's support, and the option the
  shared prior favours reaches quorum **inside the blind round**, before the
  informed member has said anything. The room is not converging; it is
  amplifying a shared error and calling the result agreement.

  Measured on the deliberation benchmark over 2000 seeded rooms
  (`vendor/tinyhivemind/crates/tinyhivemind-hive/examples/bench`): asking for a
  deposit instead takes a hidden-profile room from **16.2% to 66.6%** correct,
  and costs 3.7 points (78.8% → 75.1%) where every member's reading is equally
  good. Raising `quorum` was tried first and does not work — at unanimity the
  room simply stops deciding (31% of episodes reach one, accuracy 10.1%). The
  bar is not the problem; what the bar counts is.
- **A reason to stop.** The episode ends on a quorum it can name, not when one
  agent decides it is finished.

It is deliberately *not* a quality claim. Conformity in a group of language
models rises with interaction time, so a long episode buys correlated error
rather than better judgement. That is why the default budget is small and why
raising it should be a decision rather than a default.

## When a room opens

`crate::hivemind::desk_episode` is the single gate, and every rung is a reason
**not** to open one:

| Condition | Behaviour |
| --- | --- |
| The message names a teammate (`@mention`) | Single turn — naming somebody addresses them, not the room |
| No addressed chat (`chat: None`) | Single turn — the company's own line, answered by the orchestrator |
| A General spelling (`""`, `main`, `general`, `General`) | Single turn |
| A `dm:<teammate>` key, or a bare roster id | Single turn |
| A key that resolves to no desk | Single turn |
| A workflow copilot thread | Single, **confined** turn — it returns before this gate |
| A desk with fewer than two effective roster members | Single turn |
| `hive = { enabled = false }` on the desk | Single turn |
| The effective roster can no longer reach the desk's `quorum` | Single turn |
| Anything else | **Episode** |

Membership is read through `CompanyRecord::effective_desk_members` — the same
source `desk_lead`, `desk_default_responder` and the console's desk list read —
so who is in the room and who the console says is in it cannot drift. Overlay
(operator-created) desks are included, and take the default config.

A one-teammate-per-desk company therefore behaves byte-for-byte as it did.

The quorum rung is the runtime half of a check the manifest also makes.
`manifest.rs` refuses a **declared** desk whose `moves` table lets fewer seats
put a distinct supporter on a topic than `hive.quorum` needs, but it reads
`[[group_chat]].members` and runs once, at load. The effective roster moves
underneath it: retire the only seat holding `support` through the Team API and
a manifest that is still valid now describes a room that can never carry
anything. So `desk_episode` re-checks eligibility against the roster it is
actually about to seat, and declines rather than erroring — an operator who
retires somebody mid-flight gets a working desk with one responder, not a
failed message. Eligibility counts seats holding `propose` as well as
`support`, because `tinyhivemind_hive::quorum::standings` gates on
`TraceKind::Support` only and a `!propose` is therefore an ungated supporter.

## The loop

```text
fold the desk transcript  →  step()  →  Speak?  →  run one ordinary turn
        ▲                                   │              │
        └───────── commit next_state ◀──────┴── append the line to the journal
```

The ordering is the contract. `step` hands back a `next_state` that is only
valid once the turn it authorized is **durably** in the journal; committing it
first would let a failed write leave the room believing in a turn nothing can
read back.

The transcript is re-read out of the journal on every iteration rather than
accumulated in memory. That is the same discipline seen from the other side:
what the room folds is exactly what a human reading the desk would see, so the
standings can never disagree with the transcript they came from.

## Each turn is an ordinary turn

The episode supplies the prompt and decides who is asked. Everything else is
unchanged: the answering teammate runs on its own harness, with its own tools,
its own `memory_store` / `memory_recall` / `memory_forget` belt, the same
retrieve→inject→store memory loop every turn runs, and the same approval gate.
The seam is one function wide (`HiveTurnRunner::speak` — an agent id and a
prompt in, one reply out), which is what lets a test drive a whole episode with
no model at all.

Model tiers are unchanged too: a member's `[[agent]].tier` still selects its
model through `[inference].models`, so a room is seated with specialists by
declaring them in the manifest, not by anything here.

## The prompt

Every block is a port of the reference host's own builder, and each exists
because a live room failed without it. A driver can predict exactly what an
agent sees from the transcript plus the turn's visibility:

```text
You are @<id>, the <role> on the <desk> desk. <sight sentence>

This desk is for: <desk description>            (when the manifest says)
In the room with you: @a (Role A), @b (Role B). Address them by the ids above.
The operator asked the desk:
<the operator's message, verbatim>

The desk remembers:                                     (when memory returned any)
(From earlier episodes on this desk. This is memory, not a line in this
conversation — it has no message number and cannot be cited with ^.)
- Decide the rollout — #stage / Carried: #stage / Supporters: planner, critic

Pinned on this desk, whatever else has scrolled away:   (when the board holds any)
[12] #label opening words of the pinned message

<this seat's move list, then the rules those moves are read under>

<the options on the floor, each with its standings>     (or "no option yet")
Members who have not spoken yet: @scout, @critic. …     (see speaker diversity)
You already said this, so do not repeat it — …          (when it has spoken)
Shared attributed transcript:
[7] operator: Decide the rollout.
[9] planner: !propose #stage …
--- Above: earlier conversation on this desk, from before this      (only when a
question was asked. Still readable and citable with ^N — none of     prior episode
it is on this episode's floor. ---                                   is in the window)
[15] operator: Decide the next rollout.
[17] planner: !propose #canary …

Your one line:
```

- **The move list is phase-gated.** `!commit` is absent while the room is
  deliberating: the library authorizes a commit turn by setting the phase, and
  a `!commit` deposited before that adds no supporter to anything — a room that
  reaches for it early spends its whole budget recording a decision it never
  reached.
- **The floor carries standings**, folded with the same `standings` the episode
  decides on. Two live failures come from a member not seeing them: models coin
  a fresh id for an idea the room already named (`#rollout` and
  `#rollout-strategy` in one episode) and support split across two names never
  adds up; and a member that cannot see how far an option is from carrying has
  no way to know one more supporter would settle it.
- **A member is shown its own last line.** Live models restate their previous
  line verbatim when they have nothing new; `repetition_cap` damps a restated
  *support* and cannot see this at all.
- **The pinboard is rendered.** The window is thirty messages, so what the desk
  settled long ago is otherwise gone. A `!pin` is how it stays unavoidable, and
  the board is folded fresh each turn — a pin laid down *during* the episode is
  on the board for the next speaker.
- **Blind turns see less.** Under `Visibility::Blind`, peers' episode messages
  are withheld; the operator's task, system rows, the member's own lines, and
  everything at or below the watermark remain.
- **The watermark divides the transcript.** A live six-seat run spent an
  eighteen-turn episode arguing the *previous* Project Euler problem, because
  the transcript rendered a prior episode's `!propose` and `!support` rows
  exactly like this episode's own. Rows are never hidden — `EpisodeDriver::run`
  promises the room may still read and cite them — but a divider is drawn
  between the rows at or below `trigger` and the rows above it, once, only
  when both sides are non-empty, so a live-looking line from before this
  question was asked no longer reads as something on this episode's floor.
- **The watermark alone cannot bound a *second, concurrent* episode.** It is a
  lower bound only, and two episodes opened in the same thread — a follow-up
  accepted before the first one's turns finish — share one `(desk_id,
  thread_root)`. Without more, episode B's fold would read episode A's turns,
  appended above B's own trigger, as its own live traces and could converge on
  A's question instead of its own. `EpisodeScope` (`src/hivemind/scope.rs`) is
  the upper bound that closes this: it narrows every row `EventLogSessionLog`
  and `read_pinboard` return to what is at or below this instance's own
  trigger, or what this instance itself appended above it — a set recorded at
  the point of every append this driver or a referral it started makes, never
  re-derived from a second journal read. A concurrent episode's own turns are
  recorded against its own, separate `EpisodeScope` and so can never appear in
  this one's set.

## The grammar

A marker is recognised at the start of a line, outside fenced code blocks.

| Move | Means |
| --- | --- |
| `!propose #topic …` | puts a new option on the floor |
| `!support #topic ^N …` | backs an option, citing message N as grounds |
| `!object >N ^M …` | objects to message N, citing message M |
| `!refute #topic ^N …` | caps an option out of contention, naming the topic and citing the fact that refutes it |
| `!evidence #topic ^N …` | adds grounds without taking a side |
| `!question …` | asks for what nobody has established |
| `!defer #topic …` | stands aside; costs the turn, adds no support |
| `!commit #topic ^N …` | records the decision — **commit phase only**, and available to every seat in it |
| `!pin` / `!unpin ^N` | folds the desk's pinboard |

The `#` and the `^` are part of the grammar: `!propose canary …` names nothing
and is discarded, and a support with no citation does not count.

## The per-member move grammar

A room whose members may all make every move is a room that votes: on a live
`companies/hive_math_lab` run, every episode was three independent `!propose`s
of the same number followed by `!commit`, because in `tinyhivemind` a proposal
already counts as its own author's support. `hive.moves` assigns each seat the
deliberation markers it may open a line with, the prompt renders only those, and
the driver enforces them — one correction, then the line is journaled with its
`!` stripped so it folds to no trace at all.

`commit`, `question` and `defer` are never gated. The fold picks the speaker in
the Commit phase, so a desk that could bar a seat from recording a decision
reaches quorum and then hands the floor to somebody with nothing legal to say —
which is what a six-member desk did for eight turns before reporting itself
exhausted on an answer it had already carried.

The table, the prompt it produces, the enforcement rules, the canonical topic id
each task gets, the citation correction `require_evidential` adds, what the desk
remembers between episodes, the speaker-diversity line, and what happens when a
member's turn fails are all in
[`hivemind-deliberation.md`](hivemind-deliberation.md).

## What lands in the journal

Nothing new. There is no episode record and no second store — the transcript
*is* the episode, which is what makes the standings impossible to disagree with
the conversation they were folded from.

| Row | `chat_id` | `agent_id` | `text` |
| --- | --- | --- | --- |
| each turn | the desk id | the teammate that spoke | its one marker line, or that line with its `!` stripped when the move was barred |
| a failed turn | the desk id | `hive-report` | `@<id>'s turn did not finish: …` |
| the close | the desk id | `hive-report` | how the episode ended, plus any failed turns, demotions and questions asked elsewhere |

A desk that opted in to [private asides](hivemind-asides.md) writes the same
row with a narrower **audience** — the turn's own row, carrying the peers it
was addressed to, so a non-member's projection elides its content and keeps
everything else. That is one additive field on `AgentReply`, not a row shape
and not a second store; a desk that never enables asides writes exactly what
it wrote before. Off by default.

A desk that opted in to [cross-desk referral](hivemind-referral.md) adds two
more row shapes — the far desk's own turn, on the far desk, and its answer
carried home under `hive-referral`. Both are off by default.

Every row carries the same `parent` a single-responder reply would: the
operator message's own parent, so an answer joins the thread its question was
asked in rather than opening one underneath it. In the desk channel that is
`None`, and it is load-bearing there — the library's channel projection
promotes each root and each root's *first* reply, so turns parented to the
operator's message would be invisible to the rest of their own episode.

Only the **marker line** is journaled, not the whole reply. A harness turn can
return a paragraph however firmly it was asked for one line, and the whole
paragraph would then be rendered back into every subsequent prompt — thirty of
those is the transcript window spent on one answer. The marker is what the room
counts, so the marker line is what the transcript keeps; prose with the marker
buried in it is unwrapped, and a turn that deposits no trace falls through to
the first thing it actually said.

`hive-report` is hyphenated on purpose, exactly as `workflow-report` is: both
id minters reject a hyphen, so no roster teammate can ever hold the id and the
room's own summary can never be misattributed to one. The log adapter reads
those authors back as **system** rows, which is load-bearing in both
directions — a system row is never counted as a supporter, and is never hidden
by a blind round.

The episode journals its own turns, and the brain deliberately raises **no**
chat bubble for them: the REST chat route journals every response it is handed,
so a bubble would write a second row for a line that is already durable. The
console still sees each turn arrive live — the operator SSE feed projects
journal rows — and a reload reads the same transcript the room folded.

## The manifest knob

```toml
[[group_chat]]
id = "creative"
name = "Creative studio"
members = ["copywriter", "editor", "strategist"]
hive = { enabled = true, turn_budget = 9, quorum = 2, blind_round = true,
         require_evidential = true, refutation_cap = 2,
         dominance_cap = 4, repetition_cap = 2,
         moves = { copywriter = ["propose", "support", "commit"],
                   editor    = ["object", "refute", "evidence", "question"],
                   strategist = ["evidence", "pin", "support", "question"] } }
```

| Key | Default | Meaning |
| --- | --- | --- |
| `enabled` | on at two members | `false` keeps the desk on a single responder. `true` cannot conjure a room out of one member |
| `turn_budget` | `3 × members` | hard cap on turns; the episode reports itself exhausted at it |
| `quorum` | `(n / 2 + 1).min(n - 1)` | distinct grounded supporters a topic needs. Clamped into `1..=members` on read |
| `blind_round` | `true` | whether the opening round hides peers' positions |
| `moves` | every member, every move | member id → the *deliberation* markers that seat may open a line with. `commit`, `question` and `defer` are ungated: every seat keeps them, and an entry naming one is accepted and ignored. See [`hivemind-deliberation.md`](hivemind-deliberation.md) |
| `require_evidential` | `false` | support counts only when its citation chain reaches an `!evidence`, and an objection silences nobody unless its author deposited evidence in the window. Implies `require_grounded` |
| `refutation_cap` | unset (off) | distinct grounded refuters that cap a topic out of contention. Left off by default because tinyhivemind's own benchmark measured it costing accuracy — a refutation is global where an objection is local |
| `dominance_cap` | `50` | turns one member may take before the attention market damps its bids |
| `repetition_cap` | `3` | distinct supporters after which restating a topic scores nothing |

Every zero is rejected at validation rather than clamped — `quorum = 0` would
settle every topic the moment it was proposed, `turn_budget = 0` opens a room
that is exhausted before anybody speaks, and a cap of zero fires before anybody
has done anything — because an operator who wrote a number meant it, and
silently substituting a different one is how a desk ends up behaving in a way
its manifest does not describe.

`require_evidential` is the second half of the repair `moves` begins: assigning
somebody the evidence seat is worth little if support can still be grounded in a
peer's say-so. A support citing another support is a citation of an *opinion*,
which is the information-cascade condition with a citation on it.

The defaults are functions of the membership because the membership is the only
thing the runtime reliably knows. A quorum that is a simple majority *and still
leaves somebody outside it* means a decision is never contingent on the whole
room agreeing; for a pair that is exactly one supporter, which is the only
number available.

## Installing a grammar without editing the manifest

`company.toml` is not the only way in. `PUT {scope}/desks/{id}/hive` installs or
replaces a desk's whole `hive` block at runtime, `DELETE` on the same path drops
the override and restores whatever the manifest declares, and `GET` reports the
block in force alongside the numbers it derives.

The override is a **sibling collection** (`CompanyRecord::overlay_desk_hive`),
not a field on `OverlayDesk`, and that is the design rather than an accident:
`OverlayDesk` covers only console-created desks, so hanging the grammar there
would leave the interesting case — installing a table on a desk the manifest
declares — needing a second mechanism. It mirrors `AgentOverride`, which is the
layer that already exists for "the operator edited a manifest-declared thing".
Reset is therefore a `retain` and nothing else, because
`CompanyRecord::effective_desk_hive` falls through to `manifest.group_chats`.

Replacement is **wholesale**, unlike `AgentOverride`'s field-wise merge. A
`moves` table is a single artefact — merging one seat into a stored table is how
a desk ends up running a grammar nobody authored — and every knob is an `Option`
whose `None` already means something, so "clear `quorum` back to the derived
default" is only expressible if the block is replaced entire.

The route runs `manifest::hive_problems`, the **same** function the manifest
loader calls, so the runtime cannot accept a block a `company.toml` carrying it
would be refused for. It judges the table against the desk's **effective**
roster rather than its declared one, so overlay additions and Team-API
retirements count — which means it can legitimately refuse a table the manifest
accepted, because a table valid when `company.toml` was written stops being
valid once a seat retires.

An episode already running is unaffected: `EpisodeDriver` holds its `HiveDesk`
as a snapshot for the episode's life, so a room cannot have its quorum moved
underneath it mid-argument. The change takes effect on the next message that
opens one.

Deleting a desk drops its installed grammar with it — without that, an overlay
desk re-created under the same id silently inherits a table nobody installed.

Each write journals a `DeskHiveConfigured` row carrying the desk, whether it was
a reset, and who asked — but **not the table**, the same no-body rule
`WorkflowUpdated` follows. See [`events.md`](events.md).

## How an episode ends

| Ending | Means | The `hive-report` line |
| --- | --- | --- |
| Converged | one topic carried and the room recorded it | "The desk settled on #topic after N turns (backed by a, b)." |
| Deadlocked | two or more carried and nobody broke the tie | "The desk deadlocked after N turns: #a and #b carried together…" |
| Exhausted | the turn budget ran out first | "The desk spent its N-turn budget without reaching a decision." |
| Idle | nobody's urge cleared their threshold | "Nobody on the desk had anything to add, so the room did not open." |

Two postscripts may follow any of them, in this order: how many turns did not
finish, and which lines were demoted for a move their author may not make.

The summary says what happened rather than restating the decision's content:
the argument is in the transcript directly above it, and a paraphrase would be a
second, unattributed account of a conversation that already has one.

## Failure

A **journal** append that fails ends the episode and propagates: the room reads
its own turns back out of the journal to fold the next step, so a turn nothing
can read is a room that cannot continue. The turns already appended stay in the
transcript — they are real — and no closing report claims a decision the room
did not reach.

A **member's turn** that fails does not; see [`hivemind-deliberation.md`](hivemind-deliberation.md).

A closing report that cannot be appended is logged and swallowed: the decision
is already durable in the turns above it. So is a memory note.

## Where the code is

| Path | Holds |
| --- | --- |
| `src/hivemind/types.rs` | `HiveConfig`, `HiveDesk`, `HivePolicy`, `EpisodeOutcome`, `desk_episode` |
| `src/hivemind/moves.rs` | `MOVE_KINDS`, the marker reader, the correction, the demotion, `MoveViolation` |
| `src/hivemind/memory.rs` | `HiveMemory` / `HiveMemoryHit` / `HiveMemoryNote`, `NullHiveMemory`, the `hive/<desk>/` labels |
| `src/hivemind/log.rs` | `EventLogSessionLog` — the journal as a `SessionLog` |
| `src/hivemind/prompt.rs` | `EpisodePrompt`, `marker_line` |
| `src/hivemind/episode.rs` | `EpisodeDriver`, `HiveTurnRunner` |
| `src/hivemind/referral.rs` | `ReferralConfig`, `HiveFederation`, `HiveReferralRunner`, `EpisodeReferrals` — see [`hivemind-referral.md`](hivemind-referral.md) |
| `src/hivemind/aside.rs` | `AsideConfig`, the `!aside` / `!surface` grammar, the per-pair budget fold — see [`hivemind-asides.md`](hivemind-asides.md) |
| `src/harness/built_in/brain.rs` | the routing hook, `HiveDeskRunner`, and `HiveDeskMemory` (the `ContextStore` implementation) |

## Testing

`src/hivemind/test.rs` pins the fold with a `HiveTurnRunner` that returns
strings — the manifest knob, the log adapter, the driver's authors, the blind
projection, and `marker_line`. It deliberately runs no model, so it can say
nothing about whether a *company* deliberates.

`tests/hivemind_e2e.rs` (gated `openhuman`, run by the `rust-gated` lane's
`cargo test --features openhuman --tests`) is that half. It boots a real
company — `RuntimeBuilder`, the embedded harness, the filesystem store, the
HTTP surface, loopback sign-in — and drives it through `POST
/api/v1/company/chat`, the route the console posts to. Only the model is
scripted, and the scripted endpoint is **content-aware**: it reads the prompt
each turn was handed, works out who is speaking and what that member can see,
and answers from that. A member citing `^N` has to find `N` in the transcript
it was given, which is the property a fixed reply queue cannot prove.

| Test | What it proves |
| --- | --- |
| `a_desk_deliberates_and_converges_through_the_fold` | one operator message, three members, one `AgentReply` per turn under the teammate that took it, one closing `hive-report` naming the topic and its supporters, no second responder, and exactly one model call per journaled turn in the same order |
| `the_opening_round_is_blind_and_every_later_line_is_attributed` | the opening round is one blind turn per member and no peer line reaches it — through the transcript, the memory preamble, or the assistant history; later turns render peers as `[seq] <id>: …`, never as the viewer's own words, and each member is shown its own last line |
| `an_objection_silences_an_advocate_and_a_second_topic_carries` | cross-inhibition end to end: a backed option is objected to by message, and the option the room records is the other one — with the silenced advocate absent from its supporters |
| `a_desk_reasons_with_what_it_stored_in_an_earlier_episode` | two episodes: `memory_store` in the first, `memory_recall` in the second, and the journaled line written from what came back |
| `a_desk_reasons_with_memory_held_in_a_remote_engine` | the same claim with the memory ports bound to a CortexDB mock through `StorageSettings`: the write lands on `/v1/experience`, the read is served from `/v1/recall` |
| `a_room_that_settles_on_nothing_reports_itself_exhausted` | the budget is the only bound, and the report says the budget was spent rather than inventing a decision |
| `two_carrying_topics_and_no_objection_deadlock` | two options carry, nobody is left to break the tie, and the close says `Deadlocked` |
| `a_single_member_desk_answers_with_one_ordinary_turn` | the same company's desk of one is never handed an episode prompt: one reply, no `hive-report` |
| `a_desk_asks_another_desk_and_only_the_information_crosses` | one question crosses to the desk of one, runs a real turn there under its own member, and comes home under `hive-referral` — so the far teammate informs the room without ever authoring a row in it |

Two things the file asserts around rather than through, said here rather than
left for a reader to assume:

- **The memory loop's automatic injection is not asserted to contain the
  stored fact.** Its query is the whole incoming message — a multi-kilobyte
  episode prompt — and `store::lexical` ranks by term rarity against it, so
  which memories surface is the ranker's business. The deliberate
  `memory_recall` call is the load-bearing assertion, because it is the path an
  agent controls.
- **The deadlock script keeps supporting during the commit phase.** Two options
  can only carry at once if support keeps arriving after the first one has, and
  a live model that ignores the commit protocol is exactly how a real room gets
  there.

See also [`hivemind-deliberation.md`](hivemind-deliberation.md),
[`hivemind-referral.md`](hivemind-referral.md),
[`hivemind-asides.md`](hivemind-asides.md) and
[`docs/modules/hivemind/README.md`](../../modules/hivemind/README.md).
