//! `[policy].always_approve` — the operator's override, and the one matcher
//! both approval paths read (issue #684).
//!
//! ## Two matchers, one list, two different answers
//!
//! `always_approve` is the list that, per
//! [`PolicyMode::Full`](crate::harness::policy::PolicyMode), *"wins over
//! everything else, including Full autonomy"*. It is what an operator is
//! pointed at when they ask how to make sure a company never sends money
//! without asking.
//!
//! Until this module it was matched in two places, against two namespaces,
//! under two different rules:
//!
//! * [`ManifestApprovalGate::evaluate`](crate::policy::gate::ManifestApprovalGate)
//!   — the **native-effect** path, where a hosted or sidecar brain emits an
//!   effect frame. Matched `entry == effect.kind()`: exact, no prefix.
//! * [`ApprovalPolicy::check`](crate::harness::policy::ApprovalPolicy) — the
//!   **harness tool-call** path, where an openhuman agent calls a tool.
//!   Matched the **tool name**, exact *or* leading dotted segment.
//!
//! So one operator list meant two things depending on which brain was running,
//! and `always_approve = ["payment"]` parked `payment.send` on one path and
//! silently did nothing on the other. The shipped default —
//! `payment.send` / `filing.submit` / `external.publish` — is dotted effect
//! kinds, so it reached the first matcher and was **inert on the second**,
//! which is the path every company using the openhuman toolbelt actually runs.
//!
//! ## They were never two namespaces
//!
//! The fix is not to pick a winner, because there is no genuine conflict to
//! resolve. [`ApprovalPolicy::effect_for`](crate::harness::policy::ApprovalPolicy::effect_for)
//! projects a flagged tool call onto an [`Effect`](crate::ports::types::Effect)
//! by making **the tool name the effect kind, verbatim**. A tool name already
//! *is* an effect kind — the single-segment, undotted case of one. The two
//! matchers were reading the same namespace through two different rules, and
//! the rules disagreeing is the whole defect.
//!
//! So there is one namespace and, now, one matcher: [`matches`]. Both call
//! sites read it. Both syntaxes keep working, because they were always one
//! syntax.
//!
//! ## Why configured entries are not validated against a registry
//!
//! Tool names are a closed set, but native effect kinds are deliberately open:
//! `effect_from_frame` copies the brain's `frame.kind` verbatim. That means a
//! company may legitimately gate a kind this repository has never seen. The
//! gate consults this matcher **before** its `EffectGroup` fallback, so even an
//! otherwise-unclassified kind can be exactly the one an operator meant to
//! stop.
//!
//! Consequently there is no sound load-time "does this entry resolve?" check.
//! Treating the effect classifier as a registry would reject working custom
//! fences, while accepting a dotted string merely because it contains words
//! such as `pay` or `sign` would still let typos through. The shipped default is
//! held to a stricter standard in tests: every future default entry must name a
//! declared tool and its intended target explicitly. Operator-authored entries
//! remain open, just like the effect namespace they match.

/// Whether `target` is gated by the operator's `always_approve` list.
///
/// `target` is an effect kind — which, on the harness path, is the tool name
/// (see the module docs for why those are the same thing).
///
/// Matches the exact entry or a **leading dotted segment**, so `payment` gates
/// `payment.send` but not `payments_report`. The segment boundary is load
/// bearing: a bare `starts_with` would let `pay` gate `payroll.export`, which
/// is a different capability an operator did not name.
///
/// The comparison is **ASCII-case-insensitive**. Tool names reach the harness
/// matcher from openhuman's own registry, and
/// [`consequence_of`](crate::policy::consequence::consequence_of) already
/// lowercases defensively before consulting the declaration table — so a name
/// whose case differs from the operator's spelling would classify correctly and
/// then slip the override. That is the same silent-miss this module exists to
/// end, and the fail-safe direction is to match rather than to skip.
pub fn matches(always_approve: &[String], target: &str) -> bool {
    let target = target.trim();
    always_approve
        .iter()
        .any(|entry| gates(entry.trim(), target))
}

/// Whether one `always_approve` entry gates `target`. Allocation-free: this
/// runs on every gated tool call, and the old form built a `String` per entry
/// per call to test a prefix.
fn gates(entry: &str, target: &str) -> bool {
    if entry.is_empty() {
        return false;
    }
    if target.eq_ignore_ascii_case(entry) {
        return true;
    }
    // Leading dotted segment: `payment` gates `payment.send`.
    //
    // The `.` check runs before the slice, which is what makes the slice safe:
    // a byte equal to `.` is ASCII, and an ASCII byte in valid UTF-8 is always
    // a char boundary, so `target[..entry.len()]` cannot split a code point.
    target.len() > entry.len()
        && target.as_bytes()[entry.len()] == b'.'
        && target[..entry.len()].eq_ignore_ascii_case(entry)
}

#[cfg(test)]
#[path = "always_approve_tests.rs"]
mod tests;
