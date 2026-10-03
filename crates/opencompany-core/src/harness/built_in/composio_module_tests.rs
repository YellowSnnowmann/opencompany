//! What the module is told, and when it is told again.
//!
//! These cover the two things that are this host's responsibility rather than
//! the module's: the shape of the route blob — which is what carries a company's
//! credential — and the fingerprint that decides whether a route is re-sent. A
//! wrong blob sends a call as the wrong account; a fingerprint that fails to
//! change leaves the module holding the previous company's credential. Neither
//! needs a loaded module to check, so neither is left to the live path.

use super::*;

fn direct() -> Route {
    Route::Direct {
        api_key: "ak_company_one".into(),
        entity_id: "default".into(),
    }
}

fn proxy() -> Route {
    Route::Proxy {
        base_url: "https://backend.example.invalid".into(),
        auth_token: "tiny_company_one".into(),
    }
}

#[test]
fn a_direct_route_names_the_company_key_and_no_managed_bearer() {
    let blob = direct().blob();
    assert_eq!(blob["route"], "direct");
    assert_eq!(blob["api_key"], "ak_company_one");
    assert_eq!(blob["entity_id"], "default");
    // The BYOK tier's whole point is that no platform identity is presented.
    // A blob carrying both would let the module choose, and the one it chose
    // would be billed to whoever it picked.
    assert!(blob.get("auth_token").is_none(), "{blob}");
    assert!(blob.get("base_url").is_none(), "{blob}");
}

#[test]
fn a_proxy_route_names_the_managed_bearer_and_no_company_key() {
    let blob = proxy().blob();
    assert_eq!(blob["route"], "proxy");
    assert_eq!(blob["base_url"], "https://backend.example.invalid");
    assert_eq!(blob["auth_token"], "tiny_company_one");
    assert!(blob.get("api_key").is_none(), "{blob}");
}

#[test]
fn a_proxy_route_pins_utc_rather_than_the_host_s_zone() {
    // Upstream sends the user's own zone, which is right for a single-user
    // desktop. Here a tenant's timestamps must not depend on which server their
    // request landed on, and UTC is what upstream itself falls back to.
    assert_eq!(proxy().blob()["timezone"], "UTC");
}

#[test]
fn no_route_carries_a_state_dir() {
    // Load-time only: the trigger archive opens once, and naming a directory on
    // a reconfigure would strand the history already written there. Upstream
    // strips it before every reconcile; this host never adds it.
    for blob in [direct().blob(), proxy().blob()] {
        assert!(blob.get("state_dir").is_none(), "{blob}");
    }
}

#[test]
fn a_rotated_key_is_a_different_route() {
    let rotated = Route::Direct {
        api_key: "ak_company_one_rotated".into(),
        entity_id: "default".into(),
    };
    // If these matched, a company that rotated its key would keep being charged
    // against the revoked one until some unrelated call happened to reconfigure.
    assert_ne!(
        fingerprint(&direct().blob()),
        fingerprint(&rotated.blob()),
        "a rotated key must reconfigure the module"
    );
}

#[test]
fn two_companies_are_never_the_same_route() {
    let other = Route::Direct {
        api_key: "ak_company_two".into(),
        entity_id: "default".into(),
    };
    // The failure this prevents is the serious one: a second company's call
    // running on the first company's Composio account.
    assert_ne!(fingerprint(&direct().blob()), fingerprint(&other.blob()));
    let other_managed = Route::Proxy {
        base_url: "https://backend.example.invalid".into(),
        auth_token: "tiny_company_two".into(),
    };
    assert_ne!(
        fingerprint(&proxy().blob()),
        fingerprint(&other_managed.blob())
    );
}

#[test]
fn the_two_tiers_are_never_the_same_route() {
    assert_ne!(fingerprint(&direct().blob()), fingerprint(&proxy().blob()));
}

#[test]
fn an_unchanged_route_fingerprints_the_same() {
    // The other half of the contract: the common case inside a turn is the same
    // company calling twice, and that must not pay for a bus round-trip.
    assert_eq!(fingerprint(&direct().blob()), fingerprint(&direct().blob()));
    assert_eq!(fingerprint(&proxy().blob()), fingerprint(&proxy().blob()));
}

#[test]
fn a_listing_names_only_what_the_caller_asked_for() {
    let asked = [
        "github".to_string(),
        "  gmail  ".to_string(),
        String::new(),
        "   ".to_string(),
    ];
    assert_eq!(named(Some(&asked)), vec!["github", "gmail"]);
    // Empty is the module's "not narrowed", and it is skipped on the wire
    // rather than sent as an empty list.
    assert!(named(None).is_empty());
    assert!(named(Some(&[])).is_empty());
}
