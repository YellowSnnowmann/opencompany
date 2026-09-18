use super::*;
use crate::server::users::token::OsTokens;

#[test]
fn a_started_link_is_found_once_and_only_by_its_own_company() {
    let links = HubLinks::new();
    let started = links.start(&OsTokens, "acme");

    // Another company holding the same handle gets nothing. The handle is
    // opaque and unguessable, but the check is what makes that a property
    // of the code rather than of the entropy.
    assert!(links.take(&started.state, "other").is_none());
    // ...and the mismatch spent it, so even the right company is now too
    // late. Single-use is the safer direction to fail in: the button can be
    // clicked again, and a handle that survived a wrong guess would be one
    // an attacker could keep trying companies against.
    assert!(links.take(&started.state, "acme").is_none());
}

#[test]
fn a_link_is_taken_exactly_once() {
    let links = HubLinks::new();
    let started = links.start(&OsTokens, "acme");

    let first = links.take(&started.state, "acme");
    assert!(first.is_some());
    assert!(links.take(&started.state, "acme").is_none());
}

#[test]
fn an_unknown_state_is_simply_absent() {
    let links = HubLinks::new();
    assert!(links.take("never-minted", "acme").is_none());
}

#[test]
fn the_challenge_is_the_s256_of_the_verifier_and_the_verifier_never_appears_in_it() {
    let links = HubLinks::new();
    let started = links.start(&OsTokens, "acme");
    let link = links.take(&started.state, "acme").expect("just parked");

    assert_eq!(started.challenge, challenge_for(&link.verifier));
    assert_ne!(started.challenge, link.verifier);
    // 32 bytes of SHA-256 as unpadded base64url.
    assert_eq!(started.challenge.len(), 43);
}

#[test]
fn a_verifier_fits_rfc_7636() {
    let links = HubLinks::new();
    let started = links.start(&OsTokens, "acme");
    let link = links.take(&started.state, "acme").expect("just parked");

    assert!((43..=128).contains(&link.verifier.len()));
    assert!(
        link.verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')),
        "verifier must use the unreserved alphabet: {}",
        link.verifier
    );
}

#[test]
fn two_starts_share_nothing() {
    let links = HubLinks::new();
    let a = links.start(&OsTokens, "acme");
    let b = links.start(&OsTokens, "acme");

    assert_ne!(a.state, b.state);
    assert_ne!(a.challenge, b.challenge);
}
