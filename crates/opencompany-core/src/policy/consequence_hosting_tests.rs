use super::*;
use serde_json::json;

pub(super) fn c(tool: &str) -> Consequence {
    consequence_of(tool, &json!({}))
}

// ── hosting (issue #1079) ───────────────────────────────────────────────

/// The three tools openhuman's `hosting/README.md` labels "Read-only." ask
/// the provider what exists and what it did. Asking whether a build
/// finished must not cost an operator an approval.
#[test]
pub(super) fn a_hosting_read_does_not_park() {
    for tool in [
        "hosting_deployment_status",
        "hosting_list_sites",
        "hosting_analytics",
        "hosting_list_deployments",
        "hosting_domain_status",
    ] {
        let consequence = c(tool);
        assert_eq!(
            consequence.reach,
            Reach::Nothing,
            "`{tool}` only reads the provider"
        );
        assert!(
            !consequence.parks_under_auto(),
            "`{tool}` must not interrupt anybody"
        );
    }
}

/// The outward effects still park. Without this the downgrade above would
/// pass against a table that stopped gating the whole namespace.
#[test]
pub(super) fn a_hosting_effect_still_parks() {
    for tool in [
        "hosting_launch_site",
        "hosting_add_domain",
        "hosting_set_env",
        "hosting_rollback",
    ] {
        let consequence = c(tool);
        assert_eq!(
            consequence.reach,
            Reach::Consequence,
            "`{tool}` changes provider state"
        );
        assert!(
            consequence.parks_under_auto(),
            "`{tool}` must still park under auto"
        );
    }
}

/// **The label inversion this fixes.** Before declaring these, the fallback's
/// `undeclared_group` matched on substrings: `hosting_launch_site` — the
/// actual public deployment — contains no `deploy`/`publish`/`post` and fell
/// through to `Other`, while `hosting_deployment_status` — a read — contains
/// `deploy` and came back `Publish`. The operator's card described the
/// risky call as nothing in particular and the harmless one as a publish.
#[test]
pub(super) fn the_deployment_is_labelled_publish_and_the_status_read_is_not() {
    assert_eq!(c("hosting_launch_site").group, EffectGroup::Publish);
    assert_eq!(c("hosting_add_domain").group, EffectGroup::Publish);
    assert_ne!(
        c("hosting_deployment_status").group,
        EffectGroup::Publish,
        "a status read must not announce itself as a deployment"
    );
}

/// `hosting_set_env` is `Other`, not `Publish`, and the tool's own
/// description is why: "The site must be redeployed afterwards for a
/// build-time variable to take effect." It changes what the NEXT deployment
/// serves and does not itself deploy, so a `Publish` card would tell an
/// operator a deployment is happening when none is.
#[test]
pub(super) fn setting_env_is_not_labelled_as_a_deployment() {
    let consequence = c("hosting_set_env");
    assert_eq!(consequence.group, EffectGroup::Other);
    assert_eq!(consequence.reach, Reach::Consequence);
}

/// Every hosting tool answers from the table, not from `undeclared()`.
///
/// This is the regression guard for the mechanism itself: the fallback's
/// `READ_ONLY_PREFIXES` are matched with `name.starts_with`, so a
/// `hosting_`-prefixed read can never match one and the fallback cannot
/// classify any of these correctly. If a row is dropped, the tool silently
/// returns to that fallback rather than erroring — so the coverage is
/// asserted directly.
///
/// **This list is a floor, not the coverage guard, and issue #913 is why
/// the difference matters.** A hardcoded list only fails when a row is
/// *removed*; it says nothing when the vendor pin *adds* a tool. That is
/// exactly what happened — `hosting_rollback`, `hosting_list_deployments`
/// and `hosting_domain_status` arrived in the pin, were wired onto live
/// agents by `hosting_tools`, and this test stayed green while all three
/// fell through to `undeclared()`. The exhaustive check is
/// `every_wired_hosting_tool_is_declared` in
/// [`crate::harness::built_in::hosting`], which enumerates the belt itself;
/// it lives there because it needs the `openhuman` feature, and this file
/// compiles in lanes that do not have it. Keep both: this one holds in
/// every lane, that one is exhaustive in the lane that ships.
#[test]
pub(super) fn every_hosting_tool_is_declared() {
    let declared: std::collections::BTreeSet<&str> = declared_tools().collect();
    for tool in [
        "hosting_deployment_status",
        "hosting_list_sites",
        "hosting_analytics",
        "hosting_list_deployments",
        "hosting_domain_status",
        "hosting_launch_site",
        "hosting_add_domain",
        "hosting_set_env",
        "hosting_rollback",
    ] {
        assert!(
            declared.contains(tool),
            "`{tool}` fell back to `undeclared()`, where the `hosting_` prefix \
             defeats the read test — declare it in DECLARED"
        );
    }
}

/// The mechanism, pinned on a name that is *not* declared: a namespaced read
/// still cannot be seen by the prefix test.
///
/// Kept as documentation of why declaring is the fix rather than teaching
/// the fallback to split on `_`. Widening that test would extend trust to
/// tools no belt registered and no reviewer saw, and would turn a
/// fail-closed miss into a fail-open one.
#[test]
pub(super) fn the_fallback_cannot_see_a_read_verb_behind_a_namespace() {
    assert_eq!(
        c("hosting_list_something_undeclared").reach,
        Reach::Consequence,
        "an undeclared namespaced read gates — inconvenient, and the safe direction"
    );
    assert_eq!(
        c("list_something_undeclared").reach,
        Reach::Nothing,
        "the same verb at the front is seen, which is what makes the namespace the problem"
    );
}
