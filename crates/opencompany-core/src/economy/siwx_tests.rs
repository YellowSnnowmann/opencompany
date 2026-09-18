use super::*;

fn payload<'a>(now: i64) -> SiwxPayload<'a> {
    SiwxPayload {
        method: "POST",
        path: "/a2a/acme",
        timestamp: now,
        body_hash: "abc123",
    }
}

#[test]
fn sign_and_verify_round_trip() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let header = header_value(&build_header(&signer, &payload(now)));
    let cache = NonceCache::new();

    let id = verify(&header, "POST", "/a2a/acme", "abc123", now, &cache).expect("verifies");
    assert_eq!(id, signer.agent_id());
}

#[test]
fn tampered_body_hash_fails() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let header = header_value(&build_header(&signer, &payload(now)));
    let cache = NonceCache::new();

    let err = verify(&header, "POST", "/a2a/acme", "DIFFERENT", now, &cache);
    assert!(
        err.is_err(),
        "signature must not verify over a changed body"
    );
}

#[test]
fn skewed_timestamp_is_rejected() {
    let signer = LocalSigner::generate();
    let signed_at = 1_700_000_000;
    let header = header_value(&build_header(&signer, &payload(signed_at)));
    let cache = NonceCache::new();

    let now = signed_at + SKEW_SECS + 100;
    let err = verify(&header, "POST", "/a2a/acme", "abc123", now, &cache);
    assert!(err.is_err(), "stale timestamp must be rejected");
}

#[test]
fn an_extreme_timestamp_is_rejected_rather_than_wrapping() {
    let signer = LocalSigner::generate();
    let header = header_value(&build_header(&signer, &payload(i64::MIN)));
    let cache = NonceCache::new();

    let err = verify(&header, "POST", "/a2a/acme", "abc123", 0, &cache);
    assert!(
        err.is_err(),
        "a timestamp whose distance from now cannot be held in an i64 must be refused"
    );
}

#[test]
fn the_cache_prunes_an_entry_whose_distance_from_now_overflows() {
    let cache = NonceCache::with_ttl(SKEW_SECS);
    assert!(cache.check_and_insert("sig-a", 0, i64::MIN).unwrap());

    assert!(
        cache.check_and_insert("sig-a", 0, 0).unwrap(),
        "an entry that far outside the window must expire rather than live forever"
    );
}

#[test]
fn replayed_signature_is_rejected() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let header = header_value(&build_header(&signer, &payload(now)));
    let cache = NonceCache::new();

    verify(&header, "POST", "/a2a/acme", "abc123", now, &cache).expect("first accepted");
    let err = verify(&header, "POST", "/a2a/acme", "abc123", now, &cache);
    assert!(err.is_err(), "second presentation must be rejected");
}

#[test]
fn wrong_path_fails_because_signature_binds_it() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let header = header_value(&build_header(&signer, &payload(now)));
    let cache = NonceCache::new();

    let err = verify(&header, "POST", "/a2a/other", "abc123", now, &cache);
    assert!(err.is_err());
}

#[test]
fn malformed_headers_are_rejected() {
    assert!(parse_header("Bearer xyz").is_err());
    assert!(parse_header("tiny.place only-one-part").is_err());
    assert!(parse_header("tiny.place a:b:notanint").is_err());
}

#[test]
fn nonce_cache_prunes_stale_entries() {
    let cache = NonceCache::new();
    assert!(cache.check_and_insert("sig-a", 1_000, 1_000).unwrap());
    // Far in the future: the stale entry is pruned, so re-inserting is fine.
    assert!(
        cache
            .check_and_insert("sig-a", 1_000 + SKEW_SECS * 4, 1_000 + SKEW_SECS * 4)
            .unwrap()
    );
}

#[test]
fn nonce_cache_honours_a_custom_ttl() {
    let cache = NonceCache::with_ttl(SKEW_SECS * 4);
    assert!(cache.check_and_insert("sig-a", 1_000, 1_000).unwrap());
    // Still inside the wider window, so still remembered as spent.
    assert!(
        !cache
            .check_and_insert("sig-a", 1_000 + SKEW_SECS * 3, 1_000 + SKEW_SECS * 3)
            .unwrap()
    );
}

#[test]
fn nonce_cache_prunes_by_record_timestamp_not_insertion_time() {
    // A signature stamped at the far edge of tolerated skew (future-dated,
    // but still within the freshness window when first checked) must stay
    // remembered for as long as *its own claimed timestamp* would still
    // pass a freshness check — not merely for `ttl_secs` past the moment
    // it happened to be verified. Keying the stored entry off the
    // verifier's clock instead of the claimed timestamp would prune it
    // early, reopening the slot for a replay of the same signature while
    // the claimed timestamp is still "fresh" by the caller's own check.
    let cache = NonceCache::with_ttl(SKEW_SECS);
    let claimed_ts = 1_000 + SKEW_SECS; // maximally future-dated, still fresh at now=1_000
    assert!(cache.check_and_insert("sig-a", 1_000, claimed_ts).unwrap());
    // Past the point at which keying off insertion time (1_000) would have
    // pruned the entry, but still within `ttl_secs` of `claimed_ts`.
    let replay_now = 1_000 + SKEW_SECS + 1;
    assert!(
        !cache
            .check_and_insert("sig-a", replay_now, claimed_ts)
            .unwrap(),
        "entry must still be remembered because its claimed timestamp is \
         still within the freshness window, even though insertion time is not"
    );
}

#[test]
fn an_unusable_cache_refuses_rather_than_admits() {
    let cache = NonceCache::new();
    cache.poison_for_tests();
    assert!(
        cache.check_and_insert("sig-a", 1_000, 1_000).is_err(),
        "a cache that cannot answer must not report a value as fresh"
    );
}

#[test]
fn an_unusable_cache_refuses_the_siwx_signature() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let header = header_value(&build_header(&signer, &payload(now)));
    let cache = NonceCache::new();
    cache.poison_for_tests();

    assert!(
        verify(&header, "POST", "/a2a/acme", "abc123", now, &cache).is_err(),
        "a correctly signed header must not pass when replay protection is down"
    );
}
