# What a turn is allowed to spend

The limits that bound one teammate's turn — what each one stops, and why a run
that hits one must never be reported as having hit another. Two of them are set
here (the tool-iteration cap and the in-turn spend brake); the other two are the
account running out of credits and the harness's per-turn wall-clock ceiling,
which is declared in the vendored runtime and described under
[Wall-clock](#wall-clock--the-vendored-harnesss-own-ceiling) below.

Split out of [`agents.md`](agents.md) on this repo's 500-line cap for a Markdown
file. That page owns how a teammate is declared and what reaches its prompt;
this one owns what a turn built from it may spend.

Four limits can stop one turn short, and they stop different things. Every one
of them settles as a **pause** that keeps the partial work — never as an error —
and each has its own operator lever:

| Limit | `TurnOutcome` field | Operator's lever |
|---|---|---|
| Tool-iteration cap | `hit_iteration_cap` | reply `"continue"` — there is a checkpoint |
| In-turn spend brake | `halted_for_spend` | raise the company's cap, or narrow the ask |
| Account out of credits | `budget_paused` | add credits |
| Wall-clock ceiling | `ceiling_paused` | narrow the ask, or raise the ceiling |

The first two are this crate's own policy and are described below. The third is
issue #1846's. The fourth was the last one to stop hard rather than pause; see
the section at the end.

## Tool iterations — 25

A turn may run **25** tool-calling rounds before the runtime pauses it and hands
the operator a resumable checkpoint instead of an answer.

The number is stated by this crate, not inherited. It used to be inherited: the
agent builder was never told a cap, so every teammate silently ran on the
vendored default of ten. Ten is a summariser's budget. A product manager asked
for a feature spec reads the standards, reads the release checklist, reads the
nearest prior spec, drafts, and publishes — and spends the ten before delivering
anything, which is the incident the raise comes from.

Twenty-five is ~2.5x headroom over that shape without the 5x of the runtime's
"extended" 50. **Cost grows faster than the multiplier**: each iteration re-sends
a transcript longer than the last one's, so 2.5x the rounds is more than 2.5x the
spend. That is why the ceiling is the smallest number that covers the observed
work rather than the largest one that would be safe.

It is a **global** default, deliberately: it reaches every shipped template
without editing each one. A teammate cannot raise or lower it from its manifest
today.

**Reaching it is a pause, not a failure.** The runtime stops the tool loop, asks
the model once more (with tools withheld) for a resumable "Done so far / Next
steps" checkpoint, and returns that as an ordinary successful reply. There is no
error to catch and no error to match on, which is precisely why a capped turn
used to be invisible — the operator read a tidy plan with no deliverable behind
it and no way to tell the agent had been cut off mid-task. So the harness reads
the runtime's cap flag while the turn's agent lock is still held and carries it
out on the turn's outcome, OR'd across every seat turn of the round behind one
operator message. When any of them paused, the operator gets a **second,
unauthored bubble** after the reply saying the turn stopped at its step limit,
that nothing errored, and that replying "continue" asks the agent to pick up
from there. It is a separate bubble rather than an addition to the reply because
the reply — and only the reply — is written back to the context store as memory;
appending would file the platform's notice as something the agent said and
recall it into later turns. See `src/harness/built_in/mod.rs`
(`TurnOutcome::hit_iteration_cap`), `src/hive/round.rs` for the fold across a
round, and `src/harness/built_in/brain.rs` for the notice.

## In-turn spend — armed only for a teammate with a declared daily budget

The company's other two spend controls — the plan-level token ceiling and a
teammate's `budget_usd_daily` — are both **pre-dispatch**. They decide whether a
turn may *start*; neither can see inside one. So a turn that begins one cent
under a cap can finish over it, and raising the iteration ceiling widens that
window in proportion. How far over depends on whether anything is watching from
inside: for the plan-level token ceiling, and for a teammate who declares no
daily budget, nothing is, and the overshoot is unbounded. A teammate who does
declare one gets the in-turn brake below, which bounds it to a single call.

A running turn is therefore additionally metered by an in-turn brake — openhuman's
`BudgetStopHook` — an after-call threshold check installed between iterations.
It records each completed model call, then compares cumulative spend
(`TurnCost::total_usd()`) against the cap and pauses the turn before the next
provider call once spend is at or beyond it. The brake is installed **only** for
a teammate who declares a `budget_usd_daily` cap. Because the check runs after a
call has already been charged, a crossing call lands on the ledger before the
next one is prevented — the turn can finish at or slightly above the cap, so the
worst-case overshoot is bounded by a single model call rather than an entire
turn ("one call" rather than "one turn, of unknown size").

This mirrors the vendored runtime's own posture rather than inventing one.
OpenHuman constructs `BudgetStopHook` nowhere — it is an available primitive, not
an applied policy — and the only hook it installs is `GoalBudgetStopHook`, opt-in
and tied to a user-declared goal. Its own docs are explicit: *"we never
hard-stop a user-present turn that isn't actively burning a live budget."* So a
teammate with no declared budget gets no in-turn brake, and there is deliberately
no blanket per-turn dollar figure that no operator can see or change (it would
not be in `company.toml` and not in the console). Four shipped templates do set
`budget_usd_daily` — three agents in `signals_opportunity_studio` and one in
`e2e_harness` — so the opt-in path is genuinely exercised, not dead code.

Since a budget halt and an iteration-cap pause are different outcomes, the
runtime reports them separately: `TurnOutcome::hit_iteration_cap` is read off
the turn's progress stream (`progress_pump::hit_iteration_cap`), which stays
`false` for a hook-driven stop — the run paused below the 25-round ceiling, so
the cap predicate never held. A cap pause means the teammate ran out of rounds
with work still to do and can be resumed via the "continue" bubble above; a
budget halt means it ran out of money, returns whatever reply the model produced
before the hook fired, and gets no such bubble today. Anything that renders one
to an operator must not label it with the other.

## Wall-clock — the vendored harness's own ceiling

The fourth limit, and the only one not set by this crate: the vendored harness
policy's `max_wall_clock_ms` — `DEFAULT_AGENT_TURN_TIMEOUT_SECS`, **3600** at
the current pin and 600 when #1680 was filed, with no override set anywhere in
this repo — overridden with
`OPENHUMAN_AGENT_TURN_TIMEOUT_SECS`. It bounds the whole turn from the moment
the harness run starts — model time, tool time, sub-agent time and retry backoff
all count against it. See
[`harnesses.md`](harnesses.md#how-long-a-turn-may-take) for the nesting diagram
and for why the harness's own error message misleads.

It was also the last of the four to **fail** rather than pause, until issue
#1680's second half. That was the expensive half of the defect, rather than the
misleading message #1761 fixed.

The ceiling's lever is distinct from the three above, which is why it is a fourth
field rather than a reading of an existing one: credits buy nothing, there is no
company-declared cap to raise, and — unlike a step pause — **there is no
checkpoint to continue from.** `ceiling_pause_notice`
(`src/harness/built_in/brain.rs`) therefore never uses the word "continue";
doing so would invite the operator to spend another full ceiling arriving at the
same wall.

**What survives, and what does not.** Three different answers:

- **The reply text does not.** The vendored harness returns an `Err` with no
  partial `String` in it, so the draft the turn was composing is unrecoverable
  from here. A salvage would have to happen upstream, inside the harness.
- **The spend already did**, via issue B-120 — `turn_costs` is returned outside
  the `Result` precisely because a ceiling hit "fires precisely *because* the
  agent worked for the full ceiling" and was reporting the most expensive runs a
  founder owns as free.
- **The folded step timeline does now.** `pump.finish()` runs unconditionally
  and `fold_steps` builds the full `Vec<TurnStep>` whether the reply is `Ok` or
  `Err` — but `reply.map(..)` then dropped it on every `Err`, so a `Hard` ceiling
  arm discarded a complete timeline one line after computing it. On a ceiling hit
  that timeline is by definition substantial, and for the workflow node #1680 was
  filed against it is the fetched material the summary was going to be written
  from.

**At a workflow node**, a ceiling pause settles the attempt row `Failed` with
the notice as its error, pushes the node id onto `RunCappedNodes` so the row and
the attempt agree (the #1865 reconciliation, as for the other three), and
reports `StopReason::LimitStop { limit: "wall_clock_ceiling" }` rather than
`Finished` — so the pause copy is never bound downstream as if it were the
node's deliverable. The run then continues to the next node, which is what #1680
asked for: on its own workflow, the **Send update** step is reached.

**On a delegation chain**, the pause folds first-wins exactly as
`budget_paused` does, so a ceiling hit two desks down names the teammate that
actually ran out of time. The CEO-relay is deliberately **not** skipped for it,
unlike a budget pause: the provider has not run dry, so the relay call will
work, and the synthesis it produces over the branches that *did* finish is what
the delegates' own text would otherwise never reach the operator as.

What the relay buys is the **fold onto the operator's timeline**, not its reply
text. Both chat callers replace the primary reply with
`CEILING_PAUSED_PLACEHOLDER_REPLY` whenever `ceiling_paused` is set, exactly as
#1906 established for a budget pause — so the relay's own words are discarded
the same way, and anything appended to `OperatorTurn::reply` upstream is
unreachable from there. A delegate's words reaching the operator through a pause
would need a channel of their own, which is the constraint #1906 already
records.
