use super::*;
use crate::server::users::token::OsTokens;

struct FixedTokens(u8);
impl TokenSource for FixedTokens {
    fn fill(&self, out: &mut [u8]) {
        out.fill(self.0);
    }
}

#[test]
fn a_hash_round_trips_and_rejects_the_wrong_password() {
    let phc = hash(&OsTokens, "correct horse battery").unwrap();
    assert!(verify("correct horse battery", &phc));
    assert!(!verify("Correct horse battery", &phc), "case must matter");
    assert!(!verify("wrong", &phc));
    assert!(!verify("", &phc));
}

#[test]
fn a_stored_hash_is_argon2id_and_never_the_password() {
    let phc = hash(&OsTokens, "correct horse battery").unwrap();
    assert!(phc.starts_with("$argon2id$"), "{phc}");
    assert!(
        !phc.contains("correct horse battery"),
        "the password leaked into its own hash: {phc}"
    );
}

#[test]
fn the_same_password_hashes_differently_every_time() {
    // Distinct salts: two users with the same password must not share a
    // hash, or one crack breaks both and the store leaks who matches whom.
    let a = hash(&OsTokens, "correct horse battery").unwrap();
    let b = hash(&OsTokens, "correct horse battery").unwrap();
    assert_ne!(a, b);
    assert!(verify("correct horse battery", &a));
    assert!(verify("correct horse battery", &b));
}

#[test]
fn the_salt_comes_from_the_token_source() {
    // Proves hashing draws from the crate's one randomness seam rather than
    // argon2's own RNG — otherwise this would differ.
    let a = hash(&FixedTokens(7), "correct horse battery").unwrap();
    let b = hash(&FixedTokens(7), "correct horse battery").unwrap();
    assert_eq!(a, b);
    assert_ne!(a, hash(&FixedTokens(9), "correct horse battery").unwrap());
}

#[test]
fn a_malformed_stored_hash_fails_closed() {
    for junk in ["", "not-a-hash", "$argon2id$broken", "$2y$10$bcryptish"] {
        assert!(
            !verify("anything", junk),
            "{junk:?} must not verify as a password"
        );
    }
}

#[test]
fn the_dummy_hash_is_valid_work_nobody_can_log_in_with() {
    // If DUMMY_PHC were malformed, verify() would bail immediately and the
    // timing equalization it exists for would silently do nothing.
    dummy_verify("anything");
    const DUMMY_PHC: &str = "$argon2id$v=19$m=19456,t=2,p=1$\
                             c29tZXNhbHRzb21lc2FsdA$\
                             Ik8jitpTS4/1sMkKY0YMlUj3PYm3W2v0wNKPRLGSaBM";
    assert!(
        PasswordHash::new(DUMMY_PHC).is_ok(),
        "the dummy hash must parse, or it does no work and leaks timing"
    );
}

#[test]
fn policy_enforces_length_but_not_composition() {
    let email = "ada@example.com";
    // Long enough, no uppercase/digit/symbol: accepted on purpose.
    assert!(validate("correct horse battery", email).is_ok());
    assert!(validate(&"a".repeat(MIN_PASSWORD_LEN), email).is_ok());

    let err = validate(&"a".repeat(MIN_PASSWORD_LEN - 1), email).unwrap_err();
    assert_eq!(err.code(), "invalid_request");
    assert!(format!("{err}").contains("12"));

    assert!(validate(&"a".repeat(MAX_PASSWORD_LEN + 1), email).is_err());
    assert!(validate("            ", email).is_err(), "whitespace only");
}

#[test]
fn policy_counts_characters_not_bytes() {
    // 12 multi-byte characters is 12 characters. Counting bytes would let a
    // shorter password through and reject a legitimate one.
    let emoji = "🔑".repeat(MIN_PASSWORD_LEN);
    assert!(validate(&emoji, "ada@example.com").is_ok());
    let short = "🔑".repeat(MIN_PASSWORD_LEN - 1);
    assert!(validate(&short, "ada@example.com").is_err());
}

#[test]
fn a_password_cannot_be_the_account_email() {
    let email = "ada.lovelace@example.com";
    assert!(validate(email, email).is_err());
    assert!(validate("Ada.Lovelace@Example.com", email).is_err());
    assert!(validate(" ada.lovelace@example.com ", email).is_err());
}
