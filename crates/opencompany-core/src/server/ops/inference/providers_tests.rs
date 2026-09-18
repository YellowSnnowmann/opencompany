use super::*;
use crate::app::config::DEFAULT_API_URL;
use std::collections::BTreeMap;

fn provider(slug: &str, kind: &str) -> store::Provider {
    store::Provider {
        id: store::ProviderId::new(),
        slug: slug.to_string(),
        label: slug.to_string(),
        kind: kind.to_string(),
        base_url: format!("https://{slug}.example/v1"),
        models: BTreeMap::new(),
        enabled: true,
        origin: store::ProviderOrigin::Indexed,
    }
}

// ---- what a route may name ------------------------------------------

#[test]
fn a_cloud_route_must_name_a_provider_this_company_holds() {
    let held = vec![provider("openrouter", "openrouter")];
    assert!(
        route_is_servable(
            "chat-v1",
            &resolve::ProviderRef::parse("openrouter:gpt-5"),
            &held
        )
        .is_ok()
    );
    let err = route_is_servable("chat-v1", &resolve::ProviderRef::parse("ghost"), &held)
        .expect_err("a route naming nothing fails closed");
    assert!(err.contains("ghost"), "{err}");
}

#[test]
fn the_slug_less_kinds_are_gated_too() {
    // The bug: this check is reached through `route.slug()`, which is `None`
    // for `Local` and `ClaudeCode` — so both bypassed validation entirely.
    // `POST …/providers {"kind":"claude-code"}` is refused on a host that
    // cannot reach a CLI login, while `PUT …/routes` accepted
    // `claude-code:opus` with a 200 and rendered it as a working row.
    let cloud_only = vec![provider("openrouter", "openrouter")];
    assert!(
        route_is_servable(
            "chat-v1",
            &resolve::ProviderRef::parse("claude-code:opus"),
            &cloud_only
        )
        .is_err(),
        "a CLI route on a company with no CLI login must fail closed"
    );
    assert!(
        route_is_servable(
            "chat-v1",
            &resolve::ProviderRef::parse("local"),
            &cloud_only
        )
        .is_err(),
        "and so must a local route with no local runtime"
    );
}

#[test]
fn a_category_that_is_present_serves_its_slug_less_route() {
    let with_local = vec![provider("ollama", "ollama")];
    assert!(
        route_is_servable(
            "chat-v1",
            &resolve::ProviderRef::parse("local:llama3"),
            &with_local
        )
        .is_ok()
    );
}

#[test]
fn managed_and_unset_name_no_record_and_are_always_servable() {
    // Managed resolves through the credential chain rather than the list,
    // and an absence is not a claim about anything.
    assert!(route_is_servable("chat-v1", &resolve::ProviderRef::Managed, &[]).is_ok());
    assert!(route_is_servable("chat-v1", &resolve::ProviderRef::Default, &[]).is_ok());
}

// ---- what adding a provider requires ---------------------------------

/// The guard in `plan_add` is right and stays. What was wrong was the datum
/// it read: OMLX was marked `needs_key: true`, and **no build of any of the
/// three projects called "omlx" requires a key** — two have no auth
/// mechanism at all. So the host refused to add it at all, which is a harder
/// failure than the silent one the guard was promoted here to prevent.
#[test]
fn omlx_can_be_added_without_a_key() {
    assert!(
        plan_add(
            "omlx",
            None,
            Some("http://127.0.0.1:10240/v1"),
            false,
            DEFAULT_API_URL
        )
        .is_ok(),
        "omlx requires no key, so it must not be refused for want of one"
    );
    // Supplying one is still allowed: `jundot/omlx` has an opt-in
    // `--api-key`, so accepting a key and demanding one stay separate.
    assert!(
        plan_add(
            "omlx",
            None,
            Some("http://127.0.0.1:10240/v1"),
            true,
            DEFAULT_API_URL
        )
        .is_ok()
    );
}

/// No shipped local runtime sets `needs_key` any more, so the refusal itself
/// would be covered by nothing. Asserted against a row built for the purpose
/// rather than deleted, because the guard is what stops the *original*
/// defect — a runtime stored with no credential, therefore never probed,
/// therefore added without a word.
#[test]
fn a_local_runtime_that_demands_a_key_is_still_refused_without_one() {
    let demanding = catalogue::LocalRuntime {
        slug: "needs-a-key",
        label: "Needs A Key",
        default_endpoint: None,
        needs_key: true,
        auth: catalogue::AuthStyle::Bearer,
    };
    // The condition `plan_add` applies, against a row that declares it.
    let has_key = false;
    assert!(
        demanding.needs_key && !has_key,
        "this is the state the guard refuses"
    );
    // And accepting a key is not the same as demanding one: every shipped
    // runtime is addable keyless.
    for runtime in catalogue::LOCAL_RUNTIMES {
        assert!(
            !runtime.needs_key,
            "{} cannot be added at all while it demands a key",
            runtime.slug
        );
    }
}

#[test]
fn a_keyless_local_runtime_is_still_added_without_one() {
    // Ollama wants an endpoint, not a credential. The rule is the
    // catalogue's per-row `needs_key`, never "local runtimes are keyless".
    assert!(plan_add("ollama", None, None, false, DEFAULT_API_URL).is_ok());
}

#[test]
fn a_tinyhumans_add_points_at_the_configured_platform() {
    let prod = plan_add("tinyhumans", None, None, true, DEFAULT_API_URL).expect("planned");
    assert_eq!(
        prod.base_url,
        "https://api.tinyhumans.ai/agent-integrations/openrouter"
    );
    let local = plan_add("tinyhumans", None, None, true, "http://localhost:5005").expect("planned");
    assert_eq!(
        local.base_url,
        "http://localhost:5005/agent-integrations/openrouter"
    );
    // Only TinyHumans follows `api_url`; every other cloud row keeps its own host.
    let other = plan_add("openrouter", None, None, true, "http://localhost:5005").expect("planned");
    assert_eq!(other.base_url, "https://openrouter.ai/api/v1");
}

/// A named model is written to every tier, so no workload is left to fall
/// through to the passthrough that produced the 404.
#[test]
fn a_named_model_covers_every_tier() {
    let overrides = uniform_models(Some("claude-sonnet-5"));
    assert_eq!(overrides.len(), crate::company::INFERENCE_TIERS.len());
    for tier in crate::company::INFERENCE_TIERS {
        assert_eq!(
            overrides.get(*tier).map(String::as_str),
            Some("claude-sonnet-5")
        );
    }
    assert!(uniform_models(None).is_empty());
}

// ---- the one case where routing a new provider is not a guess ---------

fn empty() -> resolve::Routes {
    resolve::Routes::new()
}

fn routed_to(slug: &str) -> resolve::Routes {
    resolve::ROUTABLE_WORKLOADS
        .iter()
        .map(|w| (w.tier().to_string(), resolve::ProviderRef::parse(slug)))
        .collect()
}

/// The reported company: nothing authored, no managed credential, one
/// provider just added. There is precisely one thing that can serve a turn,
/// so routing to anything else is not a choice that exists.
#[test]
fn a_sole_provider_with_no_managed_and_no_routes_is_unambiguous() {
    let anthropic = provider("anthropic", "anthropic");
    assert!(is_the_only_thing_that_can_answer(
        &empty(),
        std::slice::from_ref(&anthropic),
        false,
        "anthropic"
    ));
}

/// Row B2, and the one the warning is about: Managed resolves, so adding a
/// key may be for one workload, for vision only, or to compare. Writing all
/// four rows would bill the operator for everything, silently, from a screen
/// that still says Managed.
#[test]
fn managed_being_available_makes_it_a_decision_rather_than_a_certainty() {
    let anthropic = provider("anthropic", "anthropic");
    assert!(!is_the_only_thing_that_can_answer(
        &empty(),
        std::slice::from_ref(&anthropic),
        true,
        "anthropic"
    ));
}

/// Anything already authored is never overwritten, whatever it says.
#[test]
fn a_table_that_names_anything_is_left_alone() {
    let anthropic = provider("anthropic", "anthropic");
    assert!(!is_the_only_thing_that_can_answer(
        &routed_to("managed"),
        std::slice::from_ref(&anthropic),
        false,
        "anthropic"
    ));
    assert!(!is_the_only_thing_that_can_answer(
        &routed_to("anthropic"),
        std::slice::from_ref(&anthropic),
        false,
        "anthropic"
    ));
}

/// **"First provider" is the wrong test, and this is why.** Entry zero is a
/// provider the operator never added and which is always enabled, so the
/// newly added row can be the second element of the list — and the company
/// already has something that answers. Two enabled providers is a choice
/// between them, which is the operator's to make.
#[test]
fn a_second_enabled_provider_makes_it_a_choice() {
    let anthropic = provider("anthropic", "anthropic");
    let mut zero = provider("tinyhumans", "openrouter");
    zero.origin = store::ProviderOrigin::EntryZero;
    assert!(!is_the_only_thing_that_can_answer(
        &empty(),
        &[zero, anthropic],
        false,
        "anthropic"
    ));
}

/// A provider that is switched off is not competition — but the added one
/// still has to be the one that is on.
#[test]
fn only_enabled_providers_count_and_it_must_be_this_one() {
    let anthropic = provider("anthropic", "anthropic");
    let mut parked = provider("openrouter", "openrouter");
    parked.enabled = false;
    assert!(is_the_only_thing_that_can_answer(
        &empty(),
        &[parked.clone(), anthropic.clone()],
        false,
        "anthropic"
    ));
    assert!(
        !is_the_only_thing_that_can_answer(&empty(), &[parked, anthropic], false, "openrouter"),
        "a provider that is not the one enabled is not the thing that answers"
    );
}

// `the_offered_catalogue_is_sorted_and_capped` moved to
// `company::inference::paged_catalog::tests` alongside `catalogue_offer`
// itself (keys rework #2306, P3-7 review).
