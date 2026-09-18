use super::*;
use crate::company::DEFAULT_ALWAYS_APPROVE;
use crate::policy::consequence::declared_tools;

fn list(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|e| e.to_string()).collect()
}

/// The bug in issue #684, pinned from the harness side: a dotted effect
/// kind must gate a tool call of the same name.
///
/// Before this module the harness matcher and the gate matcher were
/// separate functions, and this list reached only one of them.
#[test]
fn an_entry_gates_the_same_name_on_either_path() {
    assert!(matches(&list(&["publish_artifact"]), "publish_artifact"));
    assert!(matches(&list(&["payment.send"]), "payment.send"));
}

/// The divergence the shared matcher removes: leading-segment matching was
/// the harness rule and the gate was exact-only, so `["payment"]` gated a
/// tool call and silently missed the identically-named native effect.
#[test]
fn a_leading_segment_gates_everything_under_it() {
    assert!(matches(&list(&["payment"]), "payment.send"));
    assert!(matches(&list(&["payment"]), "payment.refund"));
}

/// The segment boundary, which is what stops the prefix rule from being a
/// bare `starts_with` that gates capabilities the operator never named.
#[test]
fn a_prefix_that_is_not_a_whole_segment_does_not_gate() {
    assert!(!matches(&list(&["pay"]), "payroll.export"));
    assert!(!matches(&list(&["payment"]), "payments_report"));
}

#[test]
fn case_and_surrounding_space_do_not_defeat_the_override() {
    assert!(matches(&list(&["  Publish_Artifact "]), "publish_artifact"));
    assert!(matches(&list(&["payment.send"]), "PAYMENT.SEND"));
}

/// An empty entry must not become a wildcard. `"".starts_with(..)` logic is
/// exactly how a list of typos turns into "gate everything", which would be
/// fail-safe but would also brick every company that had one.
#[test]
fn an_empty_entry_gates_nothing() {
    assert!(!matches(&list(&["", "   "]), "publish_artifact"));
    assert!(!matches(&[], "publish_artifact"));
    // The empty *target* is the case that actually reaches the guard in
    // `gates`. Against any other name an empty entry already falls out of
    // both arms on its own, so dropping the guard changes nothing and the
    // two assertions above keep passing — they pin the contract without
    // exercising the mechanism. Here the guard is the only thing standing
    // between a blank entry and `"" == ""`, which would make a
    // whitespace-only line in an operator's list gate a blank effect kind.
    assert!(!matches(&list(&["", "   "]), ""));
}

/// The drift guard issue #684 asks for, and the reason the default is
/// empty.
///
/// **The loop is vacuous today, deliberately and visibly**: the default
/// ships `[]` because nothing in this build emits the three kinds it used
/// to name, and the one real name behind them (`publish_artifact`) must not
/// be defaulted — issue #658 ruled that `full` publishes unattended. The
/// assertion below the loop is what makes this test non-vacuous now: it
/// pins the emptiness as a decision, so restoring an entry has to come
/// through here and face the loop.
#[test]
fn every_default_entry_names_a_declared_target_and_the_default_is_empty() {
    // A future non-empty default must add one explicit `(entry, target)`
    // pair here. That makes the intended effect executable as a test rather
    // than leaving another unvalidated string list behind.
    const INTENDED_TARGETS: &[(&str, &str)] = &[];
    assert_eq!(
        DEFAULT_ALWAYS_APPROVE.len(),
        INTENDED_TARGETS.len(),
        "every shipped entry needs an explicit target in this test"
    );
    for ((entry, target), shipped) in INTENDED_TARGETS.iter().zip(DEFAULT_ALWAYS_APPROVE.iter()) {
        assert_eq!(entry, shipped, "the target table must track the default");
        assert!(
            declared_tools().any(|tool| tool == *target),
            "the intended target `{target}` is not a declared tool"
        );
        assert!(
            matches(&list(&[*entry]), target),
            "the shipped default `{entry}` does not gate its intended target \
             `{target}` — issue #684 all over again"
        );
    }
    assert!(
        DEFAULT_ALWAYS_APPROVE.is_empty(),
        "the default is empty on purpose (issue #684): a default that \
         cannot be proven to fire must not ship as though it were \
         protection. Adding an entry is a product decision — see #658 for \
         why `publish_artifact` in particular is not it."
    );
}
