//! The consequence floor: which calls commit the company, and would stop for a
//! human whatever tier the company runs (issue #2147, epic #1817).
//!
//! ## What this module is for
//!
//! Since #1925 no policy classification creates an approval request in
//! production: [`ApprovalPolicy::check`](crate::harness::policy::ApprovalPolicy)
//! returns `Allow` before `always_approve`, the daily cap, the tier dispatch and
//! [`judge`](crate::policy::judge), and the roster build disables policy HITL on
//! both the tool path and the native-effect gate. Whether an outward,
//! irreversible act reaches a person is therefore decided by the agent choosing
//! to call `request_approval`.
//!
//! Epic #1817 asks for that decision to be enforced again, on **consequence**
//! rather than on which tool was reached for. Before proposing to enforce
//! anything, #2147 measures: this module owns the predicate, and a shadow
//! reader records what it *would* have stopped without changing any decision.
//!
//! ## One implementation, not two
//!
//! The predicate is not new behaviour invented here — it is the pair of
//! consequence arms [`judge`] already applies, lifted out so the shadow reader
//! and `judge` cannot drift. [`always_approve`](crate::policy::always_approve)
//! exists because that drift already happened once (#684: one operator list,
//! two matchers, two answers). `judge` calls [`evaluate`] with no cap, which is
//! exactly its previous behaviour; the shadow reader calls it with the
//! company's cap.
//!
//! ## What the floor deliberately does not cover
//!
//! `judge`'s other two arms — an undeclared tool, and a declared tool whose
//! reach is unbounded — stop on *mechanism*: `shell` stops for `ls`, and a tool
//! stops for being unrecognised. That is the interruption #1925 removed, and
//! re-proposing it under a new name would earn the same rejection. They stay in
//! `judge` and are not part of the floor.
//!
//! [`DEFERRED`] is likewise honoured rather than quietly reversed: #658 ruled
//! that a `full` company publishes its own artifact unattended. A shadow reader
//! that silently counted it would be measuring a rule nobody has agreed to, so
//! [`deferred_group`] reports it on its own axis instead — the cost of that
//! carve-out becomes visible without this module pre-judging it.

use serde_json::Value;

use crate::policy::consequence::{Consequence, Reach, consequence_of, declared_tools};
use crate::ports::types::EffectGroup;

/// The argument key a call declares a dollar amount under.
///
/// The same key [`judge`](crate::policy::judge) reads. Kept here because the
/// money arm moved here with it.
pub const AMOUNT_KEY: &str = "amount_usd";

/// The alias [`AMOUNT_KEY`] accepts. `ApprovalPolicy::amount_usd`
/// (`src/harness/built_in/policy.rs`) reads both `amount_usd` and `amount`,
/// so [`declared_amount_usd`] has to as well — reading only [`AMOUNT_KEY`]
/// would undercount every `amount`-only call the shadow measurement exists to
/// count.
const AMOUNT_KEY_ALIAS: &str = "amount";

/// Tools withheld from the consequence rule by an explicit product ruling.
///
/// `publish_artifact` publishes into the company's own shared workspace, which
/// #658 ruled is not an outward commitment: the artifact chain versions it and
/// the company can undo it alone. The floor does not silently overturn that —
/// see [`deferred_group`] for how it is measured instead.
pub const DEFERRED: &[&str] = &["publish_artifact"];

/// Is this tool withheld from the consequence rule by [`DEFERRED`]?
pub fn is_deferred(tool: &str) -> bool {
    DEFERRED.contains(&tool.to_ascii_lowercase().as_str())
}

/// What the floor says about one call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FloorVerdict {
    /// Nothing about this call commits the company.
    Silent,
    /// The call's declared consequence class is one the company cannot take
    /// back: a publish, a send, a signature, a hire, an identity change, or a
    /// spend that leaves.
    Irreversible(EffectGroup),
    /// The call carries a dollar amount at or above the cap in force — or any
    /// amount at all, when no cap is set.
    MoneyLeaves,
}

impl FloorVerdict {
    /// Would this call stop for a human?
    pub fn requires_human(self) -> bool {
        !matches!(self, Self::Silent)
    }

    /// A short, stable word for logs and counters.
    ///
    /// Stable because a shadow measurement is aggregated across a week of a
    /// staging tenant's logs: renaming these mid-run splits one count into two.
    pub fn reason_word(self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::Irreversible(EffectGroup::Spend) => "irreversible_spend",
            Self::Irreversible(EffectGroup::Send) => "irreversible_send",
            Self::Irreversible(EffectGroup::Sign) => "irreversible_sign",
            Self::Irreversible(EffectGroup::Publish) => "irreversible_publish",
            Self::Irreversible(EffectGroup::Hire) => "irreversible_hire",
            Self::Irreversible(EffectGroup::Identity) => "irreversible_identity",
            Self::Irreversible(EffectGroup::Other) => "irreversible_other",
            Self::MoneyLeaves => "money_leaves",
        }
    }
}

/// Does this consequence class commit the company in a way it cannot take back?
pub fn is_irreversible_group(group: EffectGroup) -> bool {
    match group {
        EffectGroup::Spend
        | EffectGroup::Send
        | EffectGroup::Sign
        | EffectGroup::Publish
        | EffectGroup::Hire
        | EffectGroup::Identity => true,
        EffectGroup::Other => false,
    }
}

/// The consequence class a [`DEFERRED`] tool would have carried, or `None` for
/// every other tool.
///
/// The measurement axis for #658's carve-out. `judge` and [`evaluate`] both
/// stay silent on these tools; this reports what the silence is costing, so the
/// question "should a publish be human?" is answered with a number rather than
/// with two opinions.
pub fn deferred_group(tool: &str, args: &Value) -> Option<EffectGroup> {
    let name = tool.to_ascii_lowercase();
    if !DEFERRED.contains(&name.as_str()) {
        return None;
    }
    let consequence = consequence_of(tool, args);
    is_irreversible_group(consequence.group).then_some(consequence.group)
}

/// Whether one call commits the company.
///
/// `cap_usd` is the spend the company allows without asking. `None` means no
/// cap is configured, and then **any** declared amount is a stop — which is
/// both the cautious reading and the behaviour
/// [`judge`](crate::policy::judge) has always had, since it is the caller that
/// passes `None`.
///
/// Pure, total and deterministic, for the reason `judge` is: a stop has to be
/// explainable from the trace long after the run.
pub fn evaluate(tool: &str, args: &Value, cap_usd: Option<f64>) -> FloorVerdict {
    evaluate_consequence(tool, consequence_of(tool, args), args, cap_usd)
}

/// Same verdict as [`evaluate`], for a caller that has already computed the
/// call's [`Consequence`].
///
/// [`judge`](crate::policy::judge) is exactly that caller: it needs its own
/// `consequence_of` result for the fail-closed arm below this one regardless,
/// and `consequence_of` is not free of side effect for `composio_execute` — an
/// uncatalogued or unrecognised-toolkit slug logs a `catalogue_miss` warning
/// each time it runs. Computing it twice per call would double that telemetry
/// for exactly the traffic issue #754 exists to make visible, which is the
/// wrong direction to be wrong in.
pub fn evaluate_consequence(
    tool: &str,
    consequence: Consequence,
    args: &Value,
    cap_usd: Option<f64>,
) -> FloorVerdict {
    let name = tool.to_ascii_lowercase();
    if is_deferred(&name) {
        return FloorVerdict::Silent;
    }

    let declared = declared_tools().any(|d| d == name);

    // Declared, named as a consequence class, and this call actually has one.
    //
    // Both halves are load-bearing, and the pairing is not obvious — the same
    // reasoning `judge` records. The group names *what kind* of consequence a
    // tool has; the reach says whether this call has one at all. `web_search`
    // and the media tools declare `Spend` because the backend bills per
    // request, but their reach is `Money`: nothing leaves and nothing changes,
    // the money buys the call itself. Stopping on the group alone parks every
    // search, and an agent with no search invents citations. The daily cap is
    // the boundary for that spend.
    //
    // Restricted to declared tools for the same reason: an undeclared tool is
    // given a group by matching words in its name, a fallback that exists to
    // label a card rather than to decide one.
    if declared
        && is_irreversible_group(consequence.group)
        && consequence.reach == Reach::Consequence
    {
        return FloorVerdict::Irreversible(consequence.group);
    }

    // Then the arguments of this actual call: a tool the table calls
    // unclassified can still be carrying money.
    if let Some(amount) = declared_amount_usd(args)
        && cap_usd.is_none_or(|cap| amount >= cap)
    {
        return FloorVerdict::MoneyLeaves;
    }

    FloorVerdict::Silent
}

/// The dollar amount this call declares, when it declares a positive one.
///
/// Reads [`AMOUNT_KEY`] first and [`AMOUNT_KEY_ALIAS`] second, same order and
/// same two keys as `ApprovalPolicy::amount_usd`.
///
/// Zero and negative are not "money leaving": a zero-amount call moves nothing,
/// and a negative one is a refund shape this layer has no business guessing at.
pub fn declared_amount_usd(args: &Value) -> Option<f64> {
    args.get(AMOUNT_KEY)
        .or_else(|| args.get(AMOUNT_KEY_ALIAS))
        .and_then(Value::as_f64)
        .filter(|amount| *amount > 0.0)
}

#[cfg(test)]
#[path = "floor_tests.rs"]
mod tests;
