//! Per-call judgement: which calls stop for a human, beyond what the static
//! configuration says (issue #338, spine epic #183 §4).
//!
//! Specified in `docs/spec/company-brain/per-call-judgement.md`.
//!
//! # The gap this fills
//!
//! Whether a call stops is answered entirely by configuration decided before
//! the run starts: reserved `never_do`, the `readonly` brake, a live grant,
//! `always_approve`, the daily cap, `auto_approve_under_usd`, then the mode.
//! Nothing looks at what the run is about to do.
//!
//! That is a bad trade in both directions. Set the bar low and every run stops
//! constantly, which is the batching pressure #183 decision 3 exists to avoid.
//! Set it high — `full` — and an agent-picked call can send, pay, delete or
//! otherwise reach beyond a boundary this layer can see without asking anyone.
//! Publishing is the deliberate #658 exception described below.
//!
//! `full`'s stated contract is "the agents act without asking, **except the few
//! things on the always-ask list**". Without this module that exception is
//! whatever one operator remembered to type into `always_approve`. With it, the
//! irreversible acts stop on their own merits.
//!
//! # This can only ever ADD a stop
//!
//! The single most important property here, and the one the tests pin. This is
//! consulted **only where the rest of the chain has already decided to allow**,
//! and its only possible answers are "stop" and "say nothing". It cannot
//! un-deny a `readonly` refusal, cannot skip `always_approve`, cannot spend a
//! grant, and cannot soften the reserved `never_do` slot. Static configuration
//! stays authoritative everywhere it speaks; this speaks only in the silence
//! after it.
//!
//! The invariant, phrased so it survives the next tier: **this arm only ever
//! speaks where the mode allowed.** A tier that already parks or denies a call
//! keeps its own answer, whatever the tier is called and however many there
//! are.
//!
//! Which tiers that leaves the arm speaking on is deliberately **not
//! enumerated** — not here, not in [`crate::harness::policy`], and not in the
//! spec. A prose tier list is a copy to miss the next time a tier lands, and
//! `auto` (issue #560) landing mid-review is the proof that they land. The
//! question is answered instead by the one copy that cannot go stale:
//! `the_arm_adds_nothing_under_auto` walks **every declared tool** and asserts
//! this arm is silent on each one a tier still allows — so a new tier or a new
//! declaration fails a test rather than outdating a paragraph.
//!
//! # Where the classification lives
//!
//! The issue asks whether this is a property of the tool definition or a runtime
//! judgement per candidate call. It is a **hybrid**, and deliberately not the
//! third option:
//!
//! - **Declared**, from [`crate::policy::consequence`]'s table — the same
//!   declaration the approval card and the standing-grant rule already read. No
//!   new column and no second taxonomy: a second list would drift against the
//!   first, and the drift would be silent in the safe-looking direction.
//! - **Runtime**, from the arguments of the actual call — a `composio_execute`
//!   slug that names a send, or a declared amount of money. Deterministic
//!   inspection, not inference.
//! - **Not a model call.** A classifier LLM on the approval path costs money and
//!   latency per candidate call, is itself an external effect being made to
//!   decide whether external effects are allowed, and — fatally — returns a
//!   different verdict on different days. "Why did it stop?" must have an
//!   answer that can be re-derived from the record months later, which #242's
//!   trace exists to make possible. A sampled verdict cannot be audited.
//!
//! # It does not learn
//!
//! Firmly, and not as an oversight. If an operator approves the same shape of
//! call five times, the sixth still stops. Consent is an operator writing a
//! rule — that is issue #563, and a rule is a thing they can read, revoke and
//! be shown. A classifier noticing a habit is not consent: nobody agreed to it,
//! it cannot be pointed at, and it converts a security boundary into a
//! frequency count. The two mechanisms must not converge, so this one holds no
//! state across calls at all.
//!
//! That is also why **novelty is not a signal here**, though #338 lists it.
//! Every use of "we have seen this before" makes the gate *looser*, and a
//! looser second call is precisely what #243's single-use grant already governs:
//! the operator approves one call, redeeming it consumes it, and the next
//! identical call parks again. A novelty rule that skipped the second stop would
//! quietly repeal that. It is recorded as a signal on the trace instead, where
//! it informs a human without deciding anything.
//!
//! # Which path a call arrives on decides whether this speaks at all
//!
//! Issue #674's ruling, and the reason this module takes a [`CallPath`] rather
//! than judging every call the same way.
//!
//! A workflow `tool_call` node is **refused at author time** unless its
//! namespace is one the workflow invoker wires *and* the company's
//! `[tools].allow` grants it. So a saved `shell` node has already passed **two
//! operator gates** — the manifest grant, then authoring — and the operator has
//! seen the actual command they saved.
//!
//! An agent turn has passed **neither**. The model picks the tool and the
//! arguments at run time. That is a real difference in what the operator
//! consented to, which is why `full` meaning "do not ask me about what I
//! authored" is coherent while "do not ask me about anything the model decides
//! to run" is not.
//!
//! So: **#338's rules govern [`CallPath::Agent`]. #614's position governs
//! [`CallPath::AuthoredWorkflowNode`]** — on that path this module is silent,
//! and the operator's controls are the two gates above plus `always_approve`,
//! which still gates a node the tier would allow.
//!
//! ## The boundary condition, without which the split is a hole
//!
//! **A node whose arguments are templated from an upstream node's output is not
//! pre-declared**, and follows the *agent* rule. The operator declared the
//! *shape*; the *content* arrives at run time from data they never saw.
//!
//! Without this the split is trivially defeated: author one `shell` node whose
//! `command` is `=previous.output` and the two-gate argument stops describing
//! what actually runs. See [`any_argument_is_templated`].
//!
//! # What this deliberately does NOT stop
//!
//! #338's acceptance says "send, publish, pay, or delete". Read against the
//! split above: on the agent path **send, pay and delete are gated and publish
//! is not**; on the authored-node path none of the four are gated here unless
//! the boundary condition returns the node to the agent rule.
//!
//! `publish_artifact` is carved out on **both** paths, permanently — see
//! `DEFERRED` and issue #658.
//!
//! `shell` stays gated on the agent path, which is what keeps "or delete" real:
//! destruction has no `EffectGroup` of its own, and `shell` is how a run
//! deletes.
//!
//! # Failure is a stop
//!
//! There is no "unclassified, so allow" path. A tool nobody declared stops
//! ([`StopReason::Undeclared`]) unless `consequence_of` can see from its name
//! that it only reads, and a Composio slug nobody classified is already treated
//! as a send by the catalogue's own cautious fallback.
//!
//! The undeclared case gets its **own** reason rather than borrowing the group
//! one, because for an undeclared tool the group is a guess made from words in
//! the name. That guess is good enough to label a card and not good enough to
//! justify a stop to an operator: `write_file` contains "file", so the guess is
//! `Sign`, and "signs or files a document" is a confident false statement about
//! a file write. The call stops either way; only the sentence differs, and the
//! sentence is what a human reads.

use serde_json::Value;

use crate::policy::consequence::{Consequence, Reach, consequence_of, declared_tools};
use crate::policy::floor;
use crate::ports::types::EffectGroup;

/// Why a call stopped, in the operator's terms.
///
/// Carried into the reason string on the parked effect, so the answer to "why
/// did this stop?" is on the approval card and in the attempt's trace rather
/// than only in this module's head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The tool's declared consequence class is one the company cannot take
    /// back: spending, sending, signing, publishing, hiring, or touching the
    /// company's identity.
    Irreversible(EffectGroup),
    /// The call changes state or reaches out, and what it can reach is not
    /// bounded by anything this layer can see — arbitrary code, an arbitrary
    /// address, or a saved workflow that performs whatever it performs.
    UnboundedReach,
    /// The arguments declare money leaving the company.
    MoneyLeaves,
    /// Nobody declared this tool, so what it does cannot be established here.
    ///
    /// The fail-closed case, and its own reason rather than a borrowed one. A
    /// tool absent from the table still gets a `group` — [`consequence_of`]
    /// falls back to matching *words in the name*, which exists so an approval
    /// card for an unknown tool is still labelled. That heuristic is fine for a
    /// label and wrong for a verdict: `write_file` contains "file", so it is
    /// labelled `Sign`, and reporting "signs or files a document" to an operator
    /// about a file write is a confident false statement. Saying "this is not
    /// declared" is the true one, and it stops the call just the same.
    Undeclared,
}

impl StopReason {
    /// The operator-facing half of the reason string.
    ///
    /// Deliberately says what the *call* does, not which rule fired: an operator
    /// deciding whether to approve needs the consequence, not this module's
    /// name. The tool name is prepended by the caller.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Irreversible(EffectGroup::Spend) => "spends money, which cannot be taken back",
            Self::Irreversible(EffectGroup::Send) => {
                "sends to a counterparty, which cannot be taken back"
            }
            Self::Irreversible(EffectGroup::Sign) => {
                "signs or files a document, which cannot be taken back"
            }
            Self::Irreversible(EffectGroup::Publish) => {
                "publishes externally, which cannot be taken back"
            }
            Self::Irreversible(EffectGroup::Hire) => {
                "hires or contracts, which cannot be taken back"
            }
            Self::Irreversible(EffectGroup::Identity) => {
                "changes the company's identity, which cannot be taken back"
            }
            // Unreachable via `judge` — `Other` is the unclassified bucket and
            // never produces `Irreversible` — but stated rather than
            // `unreachable!()`, because a panic on the approval path would be a
            // denial of service triggered by a tool name.
            Self::Irreversible(EffectGroup::Other) => "has a consequence that cannot be taken back",
            Self::UnboundedReach => {
                "can run arbitrary code or reach an arbitrary address, so what it \
                 changes cannot be bounded from here"
            }
            Self::MoneyLeaves => "moves money out of the company",
            Self::Undeclared => {
                "is not a declared tool, so what it does cannot be established here"
            }
        }
    }
}

/// The verdict for one candidate call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judgement {
    /// This module has nothing to add; whatever the chain decided stands.
    Silent,
    /// Stop for a human, whatever the mode says.
    Stop(StopReason),
}

impl Judgement {
    /// The reason this stopped, if it did.
    pub fn stop_reason(self) -> Option<StopReason> {
        match self {
            Self::Stop(reason) => Some(reason),
            Self::Silent => None,
        }
    }
}

/// Is this consequence class one the company cannot take back?
///
/// Every named class is. [`EffectGroup::Other`] is the *unclassified* bucket
/// rather than a "harmless" one — it is where a tool lands when the classifier
/// found no particular consequence to name — so it is decided by reach below
/// instead of being waved through here.
///
/// Lives in [`crate::policy::floor`] with the rest of the consequence rule, and
/// is re-exported here so this module's own tests keep reading one definition.
pub use crate::policy::floor::is_irreversible_group;

/// The tools whose reach this layer cannot bound: arbitrary code, an arbitrary
/// address, or a saved workflow that performs whatever it performs.
///
/// # Why this is a list and not a derivation
///
/// It was a derivation, and the derivation was wrong. `Reach::Consequence` +
/// `Standing::PerCall` looked like it named exactly this set — issue #444
/// refuses a standing grant to everything here — so it was tempting to read
/// "an operator cannot consent to this for a week" as the same judgement as
/// "an operator should see this once".
///
/// It is not. `Standing::PerCall` is refused for **two** different reasons that
/// the flag does not distinguish: a tool can be unbounded (`shell`), or it can
/// be bounded but overwrite state the operator authored (`workspace_write`).
/// Only the first warrants stopping a call that changes nothing outside the
/// company. Deriving the set from the flag swept up `workspace_write` and
/// `workspace_create`, which is how the company publishes to its own note
/// tree — and stopping those broke publishing outright: thirteen of the
/// `publish_turn_test` cases as the suite then stood went from passing to "the
/// model was never handed a publish receipt", because a parked call is a call
/// that never happens.
///
/// So the set is named. `every_unbounded_tool_is_declared` keeps it honest: a
/// rename in the declaration table fails that test rather than silently
/// dropping a tool out of this list.
const UNBOUNDED: &[&str] = &[
    // Arbitrary code, in a sandbox whose permissions this layer cannot see.
    // Also the way a run *deletes*: destruction has no `EffectGroup` of its
    // own, so this is what makes #338's "or delete" acceptance real.
    "shell",
    // An arbitrary address, with the tenant's own credentials and a mutating
    // method.
    "http_request",
    // Writes an arbitrary response body into the workspace `downloads/`
    // directory — a write this layer cannot bound.
    "curl",
    // Can push to a configured remote — an address this layer does not see.
    // The declaration table already singles it out for that, refusing it the
    // standing grant its filesystem siblings get.
    "git_operations",
    // Performs whatever the saved workflow performs, which is not visible here.
    "run_workflow",
];

/// Does this call change things without a bound this layer can see?
///
/// The [`Reach::Consequence`] half still matters: it is what keeps a metered
/// read out. `web_search` is [`Reach::Money`] — nothing changes and nothing
/// leaves — and parking it would be worse than useless, because openhuman
/// resolves a `RequireApproval` inline, so a parked search is a search that
/// never happens and an agent with no search invents citations.
fn is_unbounded(tool: &str, consequence: Consequence) -> bool {
    consequence.reach == Reach::Consequence && UNBOUNDED.contains(&tool)
}

/// Tools this gate never speaks about, on either path.
///
/// **This is a deliberate exclusion, not an oversight, and it is not
/// provisional. Do not delete it without reading issue #658.**
///
/// `publish_artifact` is declared `EffectGroup::Publish` + `Reach::Consequence`,
/// under a table comment reading "Externally visible and not reversible by the
/// company alone" — so by rule 1 below it qualifies, and #338's acceptance names
/// "publish" outright. It is excluded anyway, and **#658 has now ruled that this
/// is the correct behaviour rather than a stopgap**: under `full` a company
/// publishes without asking, and `always_approve` is the operator's override for
/// companies that want to be asked.
///
/// The argument #658 settled: this gate has exactly one escape — a single-use,
/// argument-exact grant per call (#243). There is no manifest knob. So stopping
/// `publish_artifact` here would mean **every company running `full` stops for a
/// human on every deliverable it publishes**, permanently, with no way to opt
/// out short of editing the declaration table. That is not a call to make as a
/// side effect of adding a classifier, and #658 declined to make it: an operator
/// who wants publishing gated names it in `always_approve`, which is read before
/// this arm and is a thing they can see, change and revoke.
///
/// The size of the alternative is visible in the tests: excluding it keeps all
/// 19 `publish_turn_test` cases green, and gating it would need a grant minted
/// per scripted call in a suite that is about publish mechanics, not approvals.
/// That count is re-checked against #659's merged fixtures rather than carried
/// forward — #659 landed first and touched this suite, so the evidence the
/// carve-out rests on was re-measured on top of it, not assumed.
///
/// `http_request`, `curl` and `web_fetch` **were** on this list pending #674 and
/// are not deferred any more. #674 ruled that they are governed by #614 on the
/// authored-node path and by #338 on the agent path — which is a scoping of the
/// rule, not an exclusion from it, so it is expressed by [`CallPath`] rather
/// than here. See the module docs.
pub use crate::policy::floor::DEFERRED;

/// The prefix tinyflows gives an expression bound at run time.
///
/// Both binding forms wear it: the dotted shorthand (`=item.brief`) and a jq
/// program (`=.items | length`). `t_transform_resolves_expr_bindings_engine_side`
/// in [`crate::workflows::runner`] pins that they resolve engine-side, and
/// `every_reachable_workflow_tool_is_classified_by_name_alone` in
/// [`crate::workflows::gate`] pins that a node's args may still be carrying them
/// unresolved when the gate pass runs — which is exactly the window this reads.
const EXPRESSION_PREFIX: &str = "=";

/// Which of the two paths a candidate call arrived on.
///
/// Issue #674's ruling made this the first thing [`judge`] asks. The variants
/// are not two flavours of caller; they are two different things an operator
/// consented to. See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallPath {
    /// A model picked this tool and these arguments during a turn. Nobody saw
    /// the call before it was made, so #338's rules apply in full.
    Agent,
    /// An operator authored this node into a saved workflow, past the manifest
    /// grant and the authoring refusal. #614's position applies: this module is
    /// silent — **unless the arguments are templated**, which un-declares the
    /// call and returns it to the agent rule.
    AuthoredWorkflowNode,
}

/// Does any argument of this call arrive from the run rather than from the
/// author?
///
/// **The boundary condition of #674's split, and the part that stops it being a
/// hole.** The authored-node path is silent because an operator saw the call.
/// They did not see `=previous.output`; they saw the *shape*, and the content
/// arrives at run time from data they never read. So a templated node is judged
/// as an agent call.
///
/// Without this, one `shell` node whose `command` is `=previous.output` defeats
/// the split outright: the two-gate argument stops describing what actually runs
/// while still being cited as the reason not to ask.
///
/// Recurses through objects and arrays, because an expression is a *value*
/// anywhere in the descriptor, not a top-level key — `{"body": {"cmd":
/// "=item.x"}}` is as templated as `{"cmd": "=item.x"}`.
///
/// Errs toward "templated": a literal string that merely starts with `=` is read
/// as an expression, which sends the call to the stricter rule. That is the
/// direction to be wrong in, and tinyflows reads it that way too.
fn any_argument_is_templated(args: &Value) -> bool {
    match args {
        Value::String(text) => text.starts_with(EXPRESSION_PREFIX),
        Value::Array(items) => items.iter().any(any_argument_is_templated),
        Value::Object(fields) => fields.values().any(any_argument_is_templated),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// The argument key a declared amount of money arrives under, and the reader
/// for it.
///
/// Both moved to [`crate::policy::floor`] with the money arm they serve, and
/// are re-exported so this module's tests keep reading one definition. The key
/// mirrors the harness policy's own reader, which is where the spend arms
/// already look; `amount_key_matches_the_harness_reader` keeps them in sync.
pub use crate::policy::floor::{AMOUNT_KEY, declared_amount_usd};

/// Should this call stop for a human on its own merits?
///
/// Pure, total and deterministic: the same call judged twice gives the same
/// answer, which is what makes a stop explainable from the trace long after the
/// run. Consulted only where the rest of the chain has already decided to
/// allow — see the module docs for why that placement is the whole safety
/// argument.
///
/// `path` decides whether this speaks at all (issue #674). It is a required
/// argument rather than a defaulted one on purpose: a caller that has not
/// thought about which path it is on should not compile.
pub fn judge(tool: &str, args: &Value, path: CallPath) -> Judgement {
    // Issue #674's split, FIRST — before any rule can reach the call.
    //
    // An authored node passed two operator gates (the manifest grant, then
    // authoring) and the operator saw the call. An agent turn passed neither.
    //
    // The `!any_argument_is_templated` half is the boundary condition, not a
    // refinement: an authored node whose arguments come from an upstream node's
    // output was never pre-declared — the operator declared the shape, the
    // content arrives at run time — so it is judged as an agent call. Deleting
    // that half leaves a split that one `=previous.output` defeats.
    if path == CallPath::AuthoredWorkflowNode && !any_argument_is_templated(args) {
        return Judgement::Silent;
    }

    let consequence = consequence_of(tool, args);
    let name = tool.to_ascii_lowercase();
    let declared = declared_tools().any(|d| d == name);

    // Carved out on both paths — see `DEFERRED` and issue #658. Ahead of every
    // rule, so none of them can reach it and so the exclusion is impossible to
    // miss. It stays here as well as inside the floor because the two arms
    // below are this module's own: a deferred tool must clear the mechanism
    // rules too, and the floor does not own those.
    if floor::is_deferred(&name) {
        return Judgement::Silent;
    }

    // The consequence arms live in
    // [`crate::policy::floor`] — the same rule the shadow measurement above the
    // `policy_hitl_enabled` bypass reads (issue #2147). `None` for the cap is
    // what preserves this path's answer exactly: with no cap, any declared
    // amount stops, which is what this arm has always done. The cap belongs to
    // the caller that has one, and this one does not — `auto_approve_under_usd`
    // and the daily budget have both already spoken above.
    //
    // `evaluate_consequence`, not `evaluate`: `consequence` above is already
    // this call's answer, and `evaluate` would compute it a second time —
    // doubling the `catalogue_miss` warning an uncatalogued `composio_execute`
    // logs on every `consequence_of` call.
    //
    // The two arms below stay here on purpose. They stop on *mechanism* — a
    // tool nobody declared, or one whose reach cannot be bounded — which is a
    // different question from whether the call commits the company, and one the
    // floor deliberately does not ask.
    match floor::evaluate_consequence(tool, consequence, args, None) {
        floor::FloorVerdict::Irreversible(group) => {
            return Judgement::Stop(StopReason::Irreversible(group));
        }
        floor::FloorVerdict::MoneyLeaves => return Judgement::Stop(StopReason::MoneyLeaves),
        floor::FloorVerdict::Silent => {}
    }

    // Fail closed on anything nobody declared that is not a pure read. The
    // read carve-out is what keeps fail-closed from collapsing into
    // stop-everything: `consequence_of` already answers `Reach::Nothing` for a
    // name that reads, and an unknown read changes nothing by definition.
    if !declared && consequence.reach != Reach::Nothing {
        return Judgement::Stop(StopReason::Undeclared);
    }

    // Then reach: declared, unclassified, but able to do anything.
    if is_unbounded(&name, consequence) {
        return Judgement::Stop(StopReason::UnboundedReach);
    }

    Judgement::Silent
}

#[cfg(test)]
#[path = "judgement_tests.rs"]
mod tests;
