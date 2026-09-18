use super::*;

fn candidate(slug: &str, enabled: bool, has_key: bool) -> Candidate {
    Candidate {
        provider: SearchProvider {
            slug: slug.to_string(),
            enabled,
            endpoint: None,
        },
        has_key,
    }
}

fn searxng(enabled: bool, endpoint: Option<&str>) -> Candidate {
    Candidate {
        provider: SearchProvider {
            slug: "searxng".to_string(),
            enabled,
            endpoint: endpoint.map(str::to_string),
        },
        has_key: false,
    }
}

#[test]
fn nothing_connected_is_managed() {
    assert_eq!(effective_slug(active(&[], None)), "managed");
    assert_eq!(effective_slug(active(&[], Some("exa"))), "managed");
}

#[test]
fn the_marked_provider_wins_when_it_is_enabled_and_complete() {
    let candidates = [candidate("exa", true, true), candidate("brave", true, true)];
    assert_eq!(
        effective_slug(active(&candidates, Some("brave"))),
        "brave",
        "the marker, not the list order"
    );
}

#[test]
fn an_unmarked_company_uses_the_first_usable_provider() {
    let candidates = [
        candidate("exa", true, false),
        candidate("brave", true, true),
    ];
    assert_eq!(
        effective_slug(active(&candidates, None)),
        "brave",
        "the keyless exa entry is skipped rather than resolving to nothing"
    );
}

#[test]
fn a_marked_provider_that_lost_its_key_falls_back_to_managed_not_to_a_sibling() {
    // The spend stays where the operator put it, or it goes nowhere. It does
    // not quietly move to another company account.
    let candidates = [
        candidate("exa", true, false),
        candidate("brave", true, true),
    ];
    assert_eq!(effective_slug(active(&candidates, Some("exa"))), "managed");
}

#[test]
fn a_marked_provider_that_was_disabled_out_of_band_falls_through() {
    let candidates = [
        candidate("exa", false, true),
        candidate("brave", true, true),
    ];
    assert_eq!(effective_slug(active(&candidates, Some("exa"))), "brave");
}

#[test]
fn a_marker_naming_nothing_falls_through() {
    let candidates = [candidate("brave", true, true)];
    assert_eq!(effective_slug(active(&candidates, Some("gone"))), "brave");
}

#[test]
fn everything_disabled_is_managed() {
    let candidates = [
        candidate("exa", false, true),
        candidate("brave", false, true),
    ];
    assert_eq!(effective_slug(active(&candidates, None)), "managed");
}

#[test]
fn searxng_is_complete_on_an_endpoint_and_needs_no_key() {
    assert!(searxng(true, Some("https://search.acme.internal")).is_complete());
    assert!(!searxng(true, None).is_complete());
    let candidates = [searxng(true, Some("https://search.acme.internal"))];
    assert_eq!(effective_slug(active(&candidates, None)), "searxng");
}

#[test]
fn an_account_provider_is_not_complete_on_an_endpoint_alone() {
    let mut exa = candidate("exa", true, false);
    exa.provider.endpoint = Some("https://api.exa.ai".to_string());
    assert!(!exa.is_complete());
}
