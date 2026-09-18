use std::collections::HashSet;

use super::*;

fn sample_challenge() -> X402Challenge {
    X402Challenge {
        amount: "25.00".into(),
        recipient: "RecipientAddr".into(),
        asset: "USDC".into(),
        network: "solana".into(),
    }
}

#[test]
fn parses_flat_challenge_body() {
    let body = serde_json::json!({
        "amount": "25.00",
        "recipient": "RecipientAddr",
        "asset": "USDC",
        "network": "solana"
    });
    assert_eq!(X402Challenge::from_body(&body).unwrap(), sample_challenge());
}

#[test]
fn parses_accepts_envelope_with_aliases() {
    let body = serde_json::json!({
        "accepts": [ { "maxAmountRequired": "10.00", "payTo": "Somebody" } ]
    });
    let ch = X402Challenge::from_body(&body).unwrap();
    assert_eq!(ch.amount, "10.00");
    assert_eq!(ch.recipient, "Somebody");
    assert_eq!(ch.asset, "USDC");
    assert_eq!(ch.network, "solana");
}

#[test]
fn missing_amount_is_an_error() {
    let body = serde_json::json!({ "recipient": "x" });
    assert!(X402Challenge::from_body(&body).is_err());
}

#[test]
fn authorize_signs_a_verifiable_payload() {
    let signer = LocalSigner::generate();
    let ch = sample_challenge();
    let auth = authorize(&signer, &ch, 1_700_000_000);

    assert_eq!(auth.agent_id, signer.agent_id());
    assert_eq!(auth.amount, "25.00");
    assert_eq!(auth.recipient, "RecipientAddr");
    verify(&auth, &NonceCache::new(), 1_700_000_000)
        .expect("authorization verifies against its own key");
}

#[test]
fn authorize_upto_carries_the_cap() {
    let signer = LocalSigner::generate();
    let ch = sample_challenge();
    let auth = authorize_upto(&signer, &ch, "100.00", 1_700_000_000);
    assert_eq!(auth.amount, "100.00");
    verify(&auth, &NonceCache::new(), 1_700_000_000).expect("upto authorization verifies");
}

#[test]
fn tampered_authorization_fails_verification() {
    let signer = LocalSigner::generate();
    let ch = sample_challenge();
    let mut auth = authorize(&signer, &ch, 1_700_000_000);
    auth.amount = "0.01".into();
    assert!(
        verify(&auth, &NonceCache::new(), 1_700_000_000).is_err(),
        "changed amount must break the signature"
    );
}

/// A deterministic source, for asserting minting is a pure function of its
/// bytes. Never use anything like this outside tests.
struct FixedTokens(u8);

impl TokenSource for FixedTokens {
    fn fill(&self, out: &mut [u8]) {
        out.fill(self.0);
    }
}

#[test]
fn nonce_is_a_pure_function_of_the_source_bytes() {
    // The property, not the encoding: the nonce is the CSPRNG's output and
    // nothing else. If a clock or a counter were mixed in, two mints from
    // the same bytes would differ.
    assert_eq!(
        mint_nonce(&FixedTokens(0xAB)),
        mint_nonce(&FixedTokens(0xAB))
    );
    assert_ne!(
        mint_nonce(&FixedTokens(0xAB)),
        mint_nonce(&FixedTokens(0xCD))
    );
}

#[test]
fn nonces_minted_in_the_same_millisecond_differ() {
    let signer = LocalSigner::generate();
    let ch = sample_challenge();
    let mut seen = HashSet::new();
    // A tight loop lands many mints inside one millisecond, which is
    // exactly where a clock-prefixed id has only its counter left.
    for _ in 0..1000 {
        let auth = authorize(&signer, &ch, 1_700_000_000);
        assert!(seen.insert(auth.nonce), "a nonce repeated");
    }
}

#[test]
fn nonces_carry_no_monotonic_counter() {
    let signer = LocalSigner::generate();
    let ch = sample_challenge();
    let minted: Vec<String> = (0..64)
        .map(|_| authorize(&signer, &ch, 1_700_000_000).nonce)
        .collect();

    // An id built from a timestamp plus an incrementing counter sorts in
    // mint order. Random values do not: 64 draws land sorted with
    // probability 1/64!, so this failing means order leaked back in.
    assert!(
        minted.windows(2).any(|w| w[0] > w[1]),
        "nonces arrived in ascending order, which implies a counter"
    );

    // And no shared structure: a common prefix is what a clock component
    // would leave behind across mints in the same millisecond.
    let first = minted[0].as_bytes();
    assert!(
        minted[1..]
            .iter()
            .any(|n| n.as_bytes().first() != first.first()),
        "every nonce shared a leading byte, which implies a fixed prefix"
    );
}

#[test]
fn nonce_is_url_safe_and_full_width() {
    let auth = authorize(&LocalSigner::generate(), &sample_challenge(), 1_700_000_000);
    // 32 bytes unpadded base64url.
    assert_eq!(auth.nonce.len(), 43, "unexpected nonce: {}", auth.nonce);
    assert!(
        auth.nonce
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "nonce is not base64url: {}",
        auth.nonce
    );
}

#[test]
fn a_changed_nonce_breaks_the_signature() {
    // The nonce is inside the signed payload, so replaying an
    // authorization under a fresh nonce is not something a payer can do
    // without the key.
    let signer = LocalSigner::generate();
    let mut auth = authorize(&signer, &sample_challenge(), 1_700_000_000);
    auth.nonce = mint_nonce(&OsTokens);
    assert!(
        verify(&auth, &NonceCache::new(), 1_700_000_000).is_err(),
        "changed nonce must break the signature"
    );
}

#[test]
fn a_replayed_authorization_is_refused() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let auth = authorize(&signer, &sample_challenge(), now);
    let spent = NonceCache::with_ttl(MAX_AGE_SECS);

    verify(&auth, &spent, now).expect("first presentation is the payment");
    assert!(
        verify(&auth, &spent, now).is_err(),
        "one signed authorization must not buy a second task"
    );
}

#[test]
fn a_second_authorization_is_still_admitted() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let spent = NonceCache::with_ttl(MAX_AGE_SECS);

    for _ in 0..3 {
        let auth = authorize(&signer, &sample_challenge(), now);
        verify(&auth, &spent, now).expect("each fresh nonce pays its own way");
    }
}

#[test]
fn a_stale_authorization_is_refused() {
    let signer = LocalSigner::generate();
    let signed_at = 1_700_000_000;
    let auth = authorize(&signer, &sample_challenge(), signed_at);
    let spent = NonceCache::with_ttl(MAX_AGE_SECS);

    assert!(
        verify(&auth, &spent, signed_at + MAX_AGE_SECS + 1).is_err(),
        "an authorization older than the spent set's memory must not verify"
    );
}

#[test]
fn an_extreme_timestamp_is_refused_rather_than_wrapping() {
    let signer = LocalSigner::generate();
    let auth = authorize(&signer, &sample_challenge(), i64::MIN);
    let spent = NonceCache::with_ttl(MAX_AGE_SECS);

    assert!(
        verify(&auth, &spent, 0).is_err(),
        "a timestamp whose distance from now cannot be held in an i64 must be refused"
    );
}

/// Builds a validly-signed authorization around an arbitrary nonce, so a
/// test can prove `verify` rejects a malformed nonce on its own merits
/// rather than piggybacking on a broken signature.
fn authorization_with_nonce(
    signer: &LocalSigner,
    ch: &X402Challenge,
    now: i64,
    nonce: &str,
) -> X402Authorization {
    let agent_id = signer.agent_id();
    let msg = canonical_bytes(
        &agent_id,
        &ch.amount,
        &ch.recipient,
        &ch.asset,
        &ch.network,
        nonce,
        now,
    );
    X402Authorization {
        agent_id,
        amount: ch.amount.clone(),
        recipient: ch.recipient.clone(),
        asset: ch.asset.clone(),
        network: ch.network.clone(),
        nonce: nonce.to_string(),
        timestamp: now,
        signature_b58: signer.sign_b58(&msg),
    }
}

#[test]
fn an_oversized_nonce_is_refused_before_touching_the_cache() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let oversized = "A".repeat(NONCE_LEN + 1);
    let auth = authorization_with_nonce(&signer, &sample_challenge(), now, &oversized);
    let spent = NonceCache::with_ttl(MAX_AGE_SECS);

    assert!(
        verify(&auth, &spent, now).is_err(),
        "a validly-signed authorization around an oversized nonce must still be refused, \
         so a counterparty cannot grow the shared cache with unbounded nonce strings"
    );
}

#[test]
fn a_nonce_outside_the_base64url_alphabet_is_refused() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let mut malformed = mint_nonce(&OsTokens);
    malformed.replace_range(0..1, "/");
    let auth = authorization_with_nonce(&signer, &sample_challenge(), now, &malformed);
    let spent = NonceCache::with_ttl(MAX_AGE_SECS);

    assert!(
        verify(&auth, &spent, now).is_err(),
        "a nonce containing a character mint_nonce never produces must be refused"
    );
}

#[test]
fn a_stale_authorization_is_refused_before_its_nonce_is_forgotten() {
    // The two windows are the same constant, so at the moment the spent set
    // would prune a nonce the authorization carrying it is already too old.
    // This is the property that makes a bounded store sufficient.
    let signer = LocalSigner::generate();
    let signed_at = 1_700_000_000;
    let auth = authorize(&signer, &sample_challenge(), signed_at);
    let spent = NonceCache::with_ttl(MAX_AGE_SECS);

    verify(&auth, &spent, signed_at).expect("fresh");
    // Late enough for the prune to drop the nonce — and late enough for the
    // age check to refuse the authorization anyway.
    assert!(verify(&auth, &spent, signed_at + MAX_AGE_SECS * 2).is_err());
}

#[test]
fn a_future_dated_authorization_cannot_outlive_its_own_nonce() {
    // A future-dated authorization is accepted (the age check is
    // symmetric), but its nonce must be remembered for as long as the
    // authorization itself would still be considered fresh — not merely
    // for MAX_AGE_SECS past the moment it happened to be verified. A
    // nonce recorded under the verification time rather than the
    // authorization's own timestamp would be forgotten while a replay of
    // the same future-dated authorization still passes the age check.
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    // Maximally future-dated: still exactly inside the ±MAX_AGE_SECS window.
    let auth = authorize(&signer, &sample_challenge(), now + MAX_AGE_SECS);
    let spent = NonceCache::with_ttl(MAX_AGE_SECS);

    verify(&auth, &spent, now).expect("future-dated but within tolerance");
    // Past the point at which keying the nonce off verification time
    // (`now`) would have pruned it, but the authorization's own claimed
    // timestamp is still within MAX_AGE_SECS of this later clock.
    let replay_at = now + MAX_AGE_SECS + 1;
    assert!(
        verify(&auth, &spent, replay_at).is_err(),
        "a future-dated authorization's nonce must not be forgotten while \
         the authorization is still within its own age window"
    );
}

#[test]
fn an_unusable_spent_set_refuses_the_payment() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let auth = authorize(&signer, &sample_challenge(), now);
    let spent = NonceCache::with_ttl(MAX_AGE_SECS);
    spent.poison_for_tests();

    assert!(
        verify(&auth, &spent, now).is_err(),
        "a spent set that cannot answer must refuse, not admit"
    );
}

#[test]
fn an_unverifiable_authorization_cannot_burn_a_nonce() {
    let signer = LocalSigner::generate();
    let now = 1_700_000_000;
    let auth = authorize(&signer, &sample_challenge(), now);
    let spent = NonceCache::with_ttl(MAX_AGE_SECS);

    let mut forged = auth.clone();
    forged.amount = "0.01".into();
    assert!(verify(&forged, &spent, now).is_err(), "forgery is refused");

    verify(&auth, &spent, now).expect("the payer's own nonce is still unspent");
}

#[test]
fn authorization_json_round_trips() {
    let signer = LocalSigner::generate();
    let auth = authorize(&signer, &sample_challenge(), 1_700_000_000);
    let json = serde_json::to_string(&auth).expect("serialize");
    assert!(json.contains("\"agentId\""));
    assert!(json.contains("\"signature\""));
    let back: X402Authorization = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, auth);
}
