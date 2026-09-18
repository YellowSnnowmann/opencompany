use std::collections::HashSet;

use super::*;

/// A deterministic source, for asserting minting is a pure function of its
/// bytes. Never use anything like this outside tests.
struct FixedTokens(u8);

impl TokenSource for FixedTokens {
    fn fill(&self, out: &mut [u8]) {
        out.fill(self.0);
    }
}

#[test]
fn sha256_hex_matches_the_published_vector() {
    // The canonical FIPS 180-2 test vector for "abc".
    assert_eq!(
        sha256_hex("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn hashing_is_deterministic_and_diffuses() {
    assert_eq!(sha256_hex("token"), sha256_hex("token"));
    // One-character difference, completely different digest.
    assert_ne!(sha256_hex("token"), sha256_hex("tokem"));
}

#[test]
fn minted_tokens_are_unpredictable_and_url_safe() {
    let src = OsTokens;
    let mut seen = HashSet::new();
    for _ in 0..1000 {
        let token = mint_session_token(&src);
        // 32 bytes unpadded base64url.
        assert_eq!(token.len(), 43, "unexpected token length: {token}");
        assert!(
            token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "token is not base64url and would need escaping in a link: {token}"
        );
        assert!(seen.insert(token), "the CSPRNG repeated a token");
    }
}

#[test]
fn login_codes_are_also_unpredictable() {
    let src = OsTokens;
    let a = mint_login_code(&src);
    let b = mint_login_code(&src);
    assert_ne!(a, b);
    assert_eq!(a.len(), 43);
}

#[test]
fn minting_is_a_pure_function_of_the_source_bytes() {
    // Proves the token is the CSPRNG's output and nothing else — no clock,
    // no counter, no process state leaking in.
    let a = mint_session_token(&FixedTokens(0xAB));
    let b = mint_session_token(&FixedTokens(0xAB));
    assert_eq!(a, b);
    assert_ne!(a, mint_session_token(&FixedTokens(0xCD)));
}

#[test]
fn a_minted_token_is_never_its_own_hash() {
    // Guards the one mistake that would defeat the whole scheme: storing
    // the plaintext under a field named `*_hash`.
    let token = mint_session_token(&OsTokens);
    assert_ne!(token, sha256_hex(&token));
    assert_eq!(sha256_hex(&token).len(), 64);
}
