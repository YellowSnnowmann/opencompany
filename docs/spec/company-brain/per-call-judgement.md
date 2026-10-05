# Per-call judgement

How the approval gate decides which calls warrant a human *on their own merits*,
in the gap the static configuration leaves (issue #338, spine epic #183 §4).

Read [approvals.md](approvals.md) first: this is **step 7** of the
[precedence chain at the tool gate](grants.md#precedence-at-the-tool-gate), and
the placement is most of the argument.

Steps 1–6 are all decided before the run starts, by an operator writing a
manifest. Nothing in them looks at what the run is about to do. That is a bad
trade in both directions: a low bar stops the run constantly, and `full` lets an
agent-picked call send, pay, delete or otherwise cross an unbounded boundary
without asking anyone. Publishing is the deliberate #658 exception below.

Step 7 closes that gap. `src/policy/judgement.rs` asks, per candidate call,
whether it warrants a human on its own merits — and it can **only ever add a
stop**. It runs only where step 6 already allowed the call, so:

- the reserved `never_do` slot, the `readonly` brake, both grant arms,
  `always_approve`, the daily cap and `auto_approve_under_usd` have each
  already had their say and returned before it is reached;
- it cannot turn a deny into a park, skip `always_approve`, or spend a grant.

The invariant, phrased so it survives the next tier: **this arm only ever speaks
where the mode allowed.** A tier that already parks or denies a call keeps its
own answer, whatever it is called and however many tiers there are — so a tier
an operator has already reasoned about does not shift under them.

Which tiers that leaves the arm speaking on is deliberately **not enumerated
here, in `judgement.rs`, or in `policy.rs`**. A prose list of tiers is a copy to
miss the next time one lands, and `auto` (#560) landing mid-review is the proof
that they land. The question is answered in the one copy that cannot go stale:
`the_arm_adds_nothing_under_auto` walks **every declared tool** and asserts this
arm is silent on each one a tier still allows.

Which matters most where the answer holds by a property of the declaration
table rather than by construction. The day a `Grantable` + `Consequence` tool
with a named consequence class is declared, a tier an operator has already
reasoned about would shift under them silently — and that test fails that day,
rather than the claim resting on a paragraph nobody re-reads.

`full`'s contract is "act without asking, *except the few things on the
always-ask list*", and this is what makes that exception mean something without
an operator having to anticipate each one by name.

## What stops

**In order:** Three rules, in order, all derived from declarations that
already exist rather than a new taxonomy:

1. a **declared consequence class** (`Spend`, `Send`, `Sign`, `Publish`,
   `Hire`, `Identity`) **paired with `Reach::Consequence`**. Both halves are
   required: `web_search` is declared `Spend` because the backend bills per
   request, but its reach is `Money` — the spend buys the call, nothing changes
   and nothing leaves. Stopping on the group alone parks every search in the
   company, and a parked search is a search that never happens. Media
   generation is `Spend` + `Consequence` and does stop, because it moves real
   money on submit;
2. **money named in the arguments** — a positive `amount_usd`, whatever the
   tool's own class says;
3. **unbounded reach** — a named set (`shell`, `curl`, `http_request`,
   `git_operations`, `run_workflow`) that is `Consequence`. This is how a
   *delete* stops: destruction has no `EffectGroup` of its own, and `shell` is
   how a run deletes. `web_fetch` is not in this set, resolvable URL or not:
   its `EffectGroup` is `Other`, so rule 1 never applies to it, and rule 3 does
   not name it — so whether it parks is decided entirely at step 4 by its
   `Reach`: free when the URL names a concrete host, `Consequence` (so it
   parks under `supervised`) when it does not. `http_request` reaches the same
   step-4 `Reach` check when its method reads GET/HEAD/OPTIONS, but "free"
   there is earned by the **whole call**, not the method in isolation: a `body`
   or a header outside an explicit safe allowlist keeps it `Consequence` even
   though the method alone would have read as a fetch — the method names the
   request's intent, but `to_tool_args` forwards `body`/`headers` unconditionally
   and the wired tool sends both regardless of method, so either is a channel a
   method-only check would have missed.

Rule 3 is a **list, and was briefly a derivation**. `Consequence` +
`Standing::PerCall` looks like it names exactly this set, since #444 refuses a
standing grant to all of it — but `PerCall` is refused for two reasons the flag
cannot tell apart: *unbounded* (`shell`) versus *bounded but overwriting
operator-authored state* (`workspace_write`). Deriving from it swept up the
workspace tools, which is how a company publishes to its own note tree, and
broke publishing outright. What keeps it narrow at the other end is
`Grantable`: the agent's own scratch writes (`file_write`, `edit`,
`apply_patch`, OpenHuman's `memory` tool) are `Consequence` too and do not stop, so "a run
that only reads, searches, or drafts does not stop" stays true.

## Which path the call arrived on

The three rules above do not apply to every call the same way. **Issue #674 split
them by path**, and the split is not a convention — it is a difference in what an
operator actually consented to.

A workflow `tool_call` node is **refused at author time** unless its namespace is
one the workflow invoker wires *and* the company's `[tools].allow` grants it. So
a saved `shell` node has passed **two operator gates** — the manifest grant, then
authoring — and the operator saw the command they saved. An agent turn has passed
**neither**: the model picks the tool and the arguments at run time. That is why
`full` meaning "do not ask me about what I authored" is coherent, while "do not
ask me about anything the model decides to run" is not.

- **Agent path** (`CallPath::Agent`) — #338's rules govern. The default at every
  construction site, because it is the strict one.
- **Authored workflow node** (`CallPath::AuthoredWorkflowNode`) — #614's position
  governs: this step is silent. The workflow gate pass opts in explicitly, on the
  private policy instance it builds for that pass. The operator's controls there
  are the two gates above plus `always_approve`, which still gates a node the
  tier would allow.

**The boundary condition, without which the split is a hole.** A node whose
arguments are **templated from an upstream node's output is not pre-declared**,
and follows the *agent* rule. The operator declared the *shape*; the *content*
arrives at run time from data they never saw. Without it, one `shell` node whose
`command` is `=previous.output` defeats the split outright — the two-gate
argument stops describing what actually runs while still being cited as the
reason not to ask. `an_authored_node_templated_from_upstream_output_follows_the_agent_rule`
pins the rule and
`a_shell_node_templated_from_upstream_output_gates_under_full` pins it at the
gate pass itself.

### The acceptance verbs, per path

#338's acceptance says "a run that would **send, publish, pay, or delete** stops
for approval regardless of mode". Restated so it is true of the code:

| Verb | Agent path | Authored workflow node |
|---|---|---|
| **send** | **Stops** — rule 1 (`Send` + `Consequence`), or fail-closed if the tool is undeclared | Not stopped here. `always_approve` and `[tools].allow` are the operator's controls — **unless templated**, then the agent rule |
| **publish** | **Not stopped** — `publish_artifact` is carved out; see #658 below | Not stopped, on the same carve-out. The carve-out is not part of the split, and holds on both paths even when templated |
| **pay** | **Stops** — rule 1 (`Spend` + `Consequence`) and rule 2 (a positive `amount_usd`) | Not stopped here — **unless templated**, then the agent rule |
| **delete** | **Stops** — rule 3, via `shell`; destruction has no `EffectGroup` of its own | Not stopped here — **unless templated**, then the agent rule |

"Regardless of mode" is exact and unchanged: on the agent path no tier exempts a
call from this step. "Regardless of *path*" was never claimed and is now
explicitly not true.

**What it deliberately does not stop, on either path.** `publish_artifact`, named
in `DEFERRED` in `src/policy/judgement.rs`. It qualifies under rule 1
(`EffectGroup::Publish` + `Reach::Consequence`) and is silent anyway.

**#658 ruled this correct behaviour rather than a stopgap.** Under `full` a
company publishes without asking, and `always_approve` is the operator's
override. The argument it settled: this step's only escape is a single-use,
argument-exact per-call grant (#243), and there is no manifest knob — so stopping
`publish_artifact` here would end unattended publishing for every `full` company
permanently, with no way to opt out short of editing the declaration table. An
operator who wants publishing gated names it in `always_approve`, which is read
at step 4 and is a thing they can see, change and revoke.

`http_request`, `curl` and `web_fetch` **were** deferred pending #674 and are
not any more. They are **scoped by path**, not excluded from the rule: governed
by #614 on the authored-node path and by #338 on the agent path — same as
`shell`, `git_operations` and `run_workflow`. On the agent path, `http_request`
and `curl` can still pick up a stop from rule 3 above; `web_fetch` cannot — see
the note there — so the path split still applies to it, but rule 3 never fires.

`every_deferred_tool_would_otherwise_be_stopped` asserts every remaining
carve-out *qualifies* on the declaration and is silent anyway — and fails if the
list is emptied — so deleting one fails loudly rather than silently re-opening
the question.

**It does not learn.** If an operator approves the same shape of call five
times, the sixth still stops. The module holds no state across calls at all.
Consent is an operator writing a rule — that is issue #563 — and a rule can be
read, revoked and shown. A classifier noticing a habit is none of those, and
turning a security boundary into a frequency count is how it stops being one.
The two mechanisms must not converge.

This is also why **novelty is not a signal**, though #338 lists it among the
four. Every use of "we have seen this before" makes the gate looser, and the
looser second call is already governed by the single-use grant: the operator
approves one call, redeeming it consumes it, the next identical call parks
again. A novelty rule that skipped the second stop would quietly repeal that.

**It is not a model call.** A classifier LLM on the approval path costs money
and latency per candidate, is itself an external effect deciding whether
external effects are allowed, and returns different verdicts on different days —
and "why did this stop?" has to be answerable from the trace months later.

**Failure is a stop.** There is no unclassified-so-allow path: an undeclared
tool reaches `consequence_of`'s cautious fallback and a Composio slug nobody
has classified is treated as a send, so both stop. Fail-closed is not a branch
to remember — it falls out of reusing a classifier that is already cautious.

**Coverage.** This step is reached from two places, and both are deliberate.
`ApprovalPolicy::check` on the agent path, and `apply_policy_gates` on the
workflow path — where #612 (closing #460) and #627 (closing #614) made
`tool_call` and `http_request` nodes consult the same policy. The earlier
statement that "the workflow path never consults `ApprovalPolicy`" was true when
this was written and is not any more; the path split above is what governs there
now, rather than the step being unreachable.

`sub_workflow` children use the same policy-controlled gate on the parent path.
The child gate is stored under a namespaced gate id, and resuming the parent
resumes the child from that gate before the parent continues. This keeps child
approval decisions auditable without requiring a separate top-level resume.
