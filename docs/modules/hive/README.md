# Hivemind Module

`src/hivemind/` turns a desk with two or more members into a room: an operator
message addressed to it runs a bounded deliberation episode instead of one
teammate's turn. The normative account — when a room opens, what the prompt
looks like, what lands in the journal, the manifest keys — is
[`docs/spec/runtime/hivemind.md`](../../spec/runtime/hivemind.md). This page is
the module's own shape and the reasoning behind its boundaries.

## Layout

| File | Holds |
| --- | --- |
| `types.rs` | `HiveConfig` (the `[[group_chat]].hive` block), `HiveMember` / `HiveDesk` (the snapshot an episode runs over), `HivePolicy` (config + size → `EpisodePolicy`), `EpisodeEnding` / `EpisodeOutcome`, and `desk_episode` — the gate |
| `log.rs` | `EventLogSessionLog`: the company journal read as a `tinyhivemind::SessionLog`, narrowed to one desk |
| `moves.rs` | the per-member move grammar: `MOVE_KINDS`, the marker reader, the one-line correction, the demotion, `MoveViolation` |
| `memory.rs` | `HiveMemory` (recall / remember), `HiveMemoryHit` / `HiveMemoryNote`, `NullHiveMemory`, and the `hive/<desk id>/<slug>` labels |
| `prompt.rs` | `EpisodePrompt` (what one authorized turn is shown) and `marker_line` (what its answer contributes) |
| `episode.rs` | `EpisodeDriver` (the host loop) and `HiveTurnRunner` (the one-function turn seam) |
| `referral.rs` | cross-desk referral: `ReferralConfig`, `HiveFederation` (the peer snapshot), `HiveReferralRunner` (the far-turn seam), `EpisodeReferrals` (the `ReferralQueue` impl) and `consider` |
| `aside.rs` | private asides: `AsideConfig` (the `hive.aside` block), the `!aside` / `!surface` grammar, and the per-pair budget-and-settlement fold — see [`hivemind-asides.md`](../../spec/runtime/hivemind-asides.md) |
| `test.rs` | the module's unit tests |
| `moves_test.rs` | the move grammar, the quorum knobs, desk memory, speaker diversity, and turn failure |
| `referral_test.rs` | what crosses, what does not, and what validation refuses |

## Why it is ungated

The module is compiled in every build, unlike `src/harness/`. Two reasons:

1. **The dependency is pure.** `tinyhivemind-hive` is serde + thiserror,
   executor-neutral, no ports, no storage, no network. There is nothing in it
   the default build would want to shed.
2. **The decision is a routing decision.** "Does this desk answer as a room?"
   is the same class of question as `desk_lead` and `chat_responder`, both of
   which live outside the harness precisely so a non-harness build can answer
   them. Putting the gate behind `#[cfg]` would put a routing rule in one build
   and not another.

Only the *hook* is gated, because only the harness has turns to run.

## The seams, and why they are where they are

**`HiveTurnRunner` is one function wide.** An agent id and a prompt in, one
reply out. The production implementation (`HiveDeskRunner`, in the brain) is a
plain `RunTurn::run`; a test scripts replies. Anything wider would have started
to encode what a turn *is* into this module, and the entire point is that a
deliberating turn and a single-responder turn are the same turn.

**The driver takes a `HiveDesk` snapshot, not a `&CompanyRecord`.** An episode
is several turns long. A room whose membership changed underneath it would hand
the floor to somebody the earlier turns never saw, and the roster the library
validated against would stop matching the desk it was validated for.

**The log adapter holds no roster.** It is opened for one desk and reads rows
written by teammates who may since have left it, so an id is its own label
there. A seated member's display name is applied by the prompt, which does hold
the roster.

**`HiveReferralRunner` is a second trait rather than a method on
`HiveTurnRunner`.** A referred turn runs on a *different* desk from the
episode's, so it needs the desk id. Folding that into the episode seam would
have made every existing implementation carry a parameter it must ignore, and
would have quietly invited an implementation that ignores it — which is a far
desk's answer journaled in the asking room under the far teammate's name, the
one thing the whole design exists to prevent.

**The federation is opt-in at the driver, not read off the record.**
`EpisodeDriver::with_federation` is absent by default, so a driver built without
it never considers a line for referral and is byte-identical to the one that
ran before referral existed. `desk_federation` returns `None` for a desk that
did not opt in and for a company with no peer desk, so the common case costs the
loop one `if let` per turn and nothing else.

**`HiveMemory` is a trait here and a `ContextStore` in the brain.**
`src/hivemind/` compiles in every build and holds no ports; starting to hold one
for desk memory would put a storage dependency in a module whose whole claim is
that it is a fold over a journal and a turn seam. The trait is two functions
wide, `NullHiveMemory` satisfies it, and `HiveDeskMemory` (in `brain.rs`) is the
real one — deliberately beside the memory loop whose `ContextStore` calls it
reuses, so a desk's notes travel through the same overlay a teammate's do.

**The move grammar is enforced in the driver, not in the prompt.** Rendering
only the moves a seat holds is necessary and not sufficient: a model will reach
for a marker it was not shown. The prompt and the enforcement therefore read the
same table (`config.moves_for`), and a barred line that survives its correction
is journaled with its `!` removed — which is the only thing that actually stops
it, since `resolve` reads a marker at the start of a line and nowhere else.

## Two contracts that are easy to break

**Append before commit.** `step` returns a `next_state` that is only valid once
the turn it authorized is durably journaled. `EpisodeDriver::run` appends, then
assigns. Reversing those two lines would let a failed write leave the room
believing in a turn nothing can read back.

**A member's failed turn is not the room's.** `HiveTurnRunner::speak`
returning `Err` journals a system row, commits `next_state` and continues; the
`?` that used to be there threw away three good turns and answered the operator
with a 500 the first time one member hit the harness's per-turn wall-clock
ceiling. Only a failed *journal* append, or `members × 2` consecutive failures,
ends the episode.

**The page contract.** `SessionLog::read_before` must return rows newest-first,
strictly descending, no larger than asked, with an exclusive cursor no newer
than the oldest row — and an *empty page must carry no cursor*, because an
empty page means the log is finished. A company journal carries approvals, task
cards and workflow runs between a desk's rows, so a single raw chunk can easily
hold no chat at all; the adapter therefore loops over raw chunks until it has at
least one qualifying row or the journal runs out. Returning the empty chunk
would truncate the transcript at whatever the last busy stretch of the journal
happened to be.

## Running the tests

```bash
cargo test --features openhuman hivemind
```

The module's own tests need no feature (`cargo test hivemind` covers them); the
brain's routing tests need `openhuman`. Nothing in the suite makes a model call:
the log adapter is exercised against an in-memory journal, and every episode is
driven by a scripted runner, so a three-agent room converging is a deterministic
assertion rather than a live-run hope.
