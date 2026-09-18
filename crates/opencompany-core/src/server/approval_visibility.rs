//! Who may read an approval's *contents* (issue #618).
//!
//! Membership decides whether you may know an approval **exists**. It does not
//! decide whether you may read what it is about. `payload` carries
//! recipient-bearing tool arguments and `amount_usd` carries money; the rest of
//! [`ApprovalSummary`] — the id, the kind, who asked, which task, when — is what
//! makes stalled work visible, and every member needs it.
//!
//! This is the product's **first per-resource, per-role field restriction**.
//! Everywhere else, role decides whether you may *act* (see
//! [`AdminScopedCompany`](crate::server::ops::scope::AdminScopedCompany)); here
//! it decides which fields of a read you receive. That is a new shape and worth
//! knowing before it is copied.
//!
//! ## Why the split lands here and not on the whole route
//!
//! Refusing the route to non-admins was the obvious alternative and is worse: a
//! Member would lose sight of *why* their work is stalled. Issue #468
//! deliberately keeps a "waiting on approval" indicator on the task card, and
//! that indicator has to survive for the people doing the work, not only for
//! the people who can sign it off.
//!
//! ## Hidden is not absent
//!
//! Redaction does **not** simply blank the fields. `payload: None` already
//! means "this effect carries no arguments", so blanking would make a withheld
//! payment indistinguishable from a no-argument tool call — the console would
//! render an approval that looks empty rather than one it may not show. So a
//! redacted summary sets [`ApprovalSummary::contents_hidden`], and the console
//! says "hidden by your role" instead of showing nothing.

use crate::runtime::types::ApprovalSummary;
use crate::server::graphql::auth::GqlAuth;

/// May this principal read an approval's payload and amount?
///
/// Both arms are written out. `GqlAuth` has exactly two, and a wildcard here
/// would silently grant contents to any arm added later — which is how a role
/// guard decays into a deny-list.
pub(crate) fn may_read_approval_contents(auth: &GqlAuth) -> bool {
    match auth {
        // The role already rides on the principal the route resolved, so
        // nothing has to be threaded down into the domain layer to ask this.
        GqlAuth::User(user) => user.may_administer(),
        // **Fail closed, deliberately.** A platform bearer is the hosting
        // control plane — it provisions and suspends containers and is not a
        // person in the company. It has no need for a tenant's message bodies
        // or payment amounts, and "the machine credential sees everything" is
        // the assumption worth not making. This costs nothing today: the
        // console authenticates as a user, and a prosumer deployment has no
        // platform credential at all (`resolve_claims` requires
        // `platform_auth` to be configured), so no human loses anything.
        //
        // If the hosting layer ever genuinely needs contents, that is a
        // deliberate scope to add here, not a default to inherit.
        GqlAuth::Platform(_) => false,
    }
}

/// May this principal read a run's **deep** trace — the unredacted reasoning,
/// tool arguments and raw output the Observatory shows behind a fold?
///
/// The same rule as [`may_read_approval_contents`]: those bodies can carry
/// credentials and file contents, exactly as an approval payload can, so the
/// admin/tenant boundary applies to them too. Kept beside the approval rule so
/// the two sensitive-content gates cannot drift.
pub(crate) fn may_read_deep_trace(auth: &GqlAuth) -> bool {
    may_read_approval_contents(auth)
}

/// Applies [`may_read_approval_contents`] to a projection on its way out.
///
/// Takes the whole list rather than one summary because every caller has a
/// list, and because doing it per-item at three call sites is three chances to
/// forget one.
pub(crate) fn for_principal(
    auth: &GqlAuth,
    mut approvals: Vec<ApprovalSummary>,
) -> Vec<ApprovalSummary> {
    if may_read_approval_contents(auth) {
        return approvals;
    }
    for approval in &mut approvals {
        hide_contents(approval);
    }
    approvals
}

/// Strips the two contents fields and records that it happened.
///
/// `contents_hidden` is what keeps this honest — see the module note.
pub(crate) fn hide_contents(approval: &mut ApprovalSummary) {
    approval.payload = None;
    approval.amount_usd = None;
    approval.contents_hidden = true;
}

/// The same rule applied to the **executed** effects on a task's detail read
/// (issue #705).
///
/// #618 restricted the money on an approval — the effect a person has *not yet*
/// signed off. The identical amount on the effect once it has run was projected
/// by a different DTO on a different route, and that route was never covered:
/// any Member could `GET` a card and read the dollar value of every irreversible
/// effect on it.
///
/// It lives here, beside [`for_principal`], rather than in the task module,
/// because the thing that must not fork is the *rule*. A second redactor in the
/// tasks route could drift from this one, and two role checks disagreeing about
/// what an operator may see is worse than the leak either was written to close.
///
/// Takes `may_read_contents` rather than the principal because
/// [`ScopedCompany`](crate::server::ops::ScopedCompany) deliberately drops the
/// role at the edge, carrying this decision forward instead — see the field's
/// own note. The decision is still made by
/// [`may_read_approval_contents`] and nowhere else.
///
/// Takes the whole list, like [`for_principal`], so a caller cannot apply it to
/// some entries and forget others.
pub(crate) fn effects_for_principal(
    may_read_contents: bool,
    mut effects: Vec<crate::server::ops::tasks::IrreversibleEffect>,
) -> Vec<crate::server::ops::tasks::IrreversibleEffect> {
    if may_read_contents {
        return effects;
    }
    for effect in &mut effects {
        // `amount_hidden` only where something was actually withheld: an effect
        // that never carried money must not report itself as redacted, or
        // "nothing to show" and "not shown to you" stop being distinguishable
        // in the direction that matters.
        if effect.amount_usd.is_some() {
            effect.amount_usd = None;
            effect.amount_hidden = true;
        }
    }
    effects
}

#[cfg(test)]
#[path = "approval_visibility_tests.rs"]
mod tests;
