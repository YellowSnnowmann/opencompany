use super::*;

#[test]
fn normalize_email_folds_case_and_trims() {
    assert_eq!(normalize_email("  Ada@Example.COM \n"), "ada@example.com");
    assert_eq!(normalize_email("ada@example.com"), "ada@example.com");
}

/// A base58 address is case-sensitive: two addresses differing only in case
/// are two different keys. Folding case the way an email is folded would map
/// them onto one identity, and a signature verified against one wallet would
/// mint a session for the other.
#[test]
fn normalize_wallet_trims_but_never_folds_case() {
    assert_eq!(normalize_wallet("  7xKXtg2C  "), "7xKXtg2C");
    assert_ne!(normalize_wallet("7xKXtg2C"), normalize_wallet("7xkxtg2c"));
}

/// The three identity keyspaces share one storage column, so they must be
/// mutually unparseable. `normalize_email` only lowercases and trims, so an
/// email that happens to start with `wallet:` or `local:` is a normalized
/// key too — see `an_email_that_looks_like_a_scheme_prefix_still_parses_as_email`
/// below for that case. Here, an unrelated email and an unrelated wallet
/// simply do not collide.
#[test]
fn login_identities_round_trip_and_stay_disjoint() {
    let email = LoginIdentity::Email("Ada@Example.com".into());
    assert_eq!(email.key(), "ada@example.com");
    assert_eq!(
        LoginIdentity::parse(&email.key()),
        LoginIdentity::Email("ada@example.com".into())
    );

    let address = bs58::encode([7u8; 32]).into_string();
    let wallet = LoginIdentity::Wallet(address.clone());
    assert_eq!(wallet.key(), format!("wallet:{address}"));
    assert_eq!(LoginIdentity::parse(&wallet.key()), wallet);

    assert_eq!(LoginIdentity::Local.key(), "local:owner");
    assert_eq!(LoginIdentity::parse("local:owner"), LoginIdentity::Local);

    // No key parses as two things.
    assert_ne!(LoginIdentity::parse(&format!("wallet:{address}")), email);
}

/// Every record written before wallet and local identities existed holds a
/// bare address, and must keep loading as one. This is the whole migration.
#[test]
fn an_unprefixed_key_is_an_email() {
    assert_eq!(
        LoginIdentity::parse("ada@example.com"),
        LoginIdentity::Email("ada@example.com".into())
    );
}

/// `normalize_email` only lowercases and trims, so an email that happens to
/// start with `wallet:` or `local:` is a normalized key `parse` must still
/// read back as an email — not misparse into the other scheme just because
/// the prefix matches. A wallet remainder must actually be base58, and a
/// local remainder must be exactly `owner`.
#[test]
fn an_email_that_looks_like_a_scheme_prefix_still_parses_as_email() {
    assert_eq!(
        LoginIdentity::parse("wallet:ada@example.com"),
        LoginIdentity::Email("wallet:ada@example.com".into())
    );
    assert_eq!(
        LoginIdentity::parse("local:owner@example.com"),
        LoginIdentity::Email("local:owner@example.com".into())
    );
    // The email path in `normalize_email` lowercases "Wallet:" to
    // "wallet:", so the collision is real, not merely hypothetical.
    assert_eq!(
        normalize_email("Wallet:ada@example.com"),
        "wallet:ada@example.com"
    );
}

/// A `wallet:` remainder that is valid base58 but not 32 bytes is not a
/// wallet — checking mere base58-decodability would still misclassify an
/// email whose local part happens to be base58-alphabet characters (no
/// `@` needed for the collision to matter here, only for `parse` to fall
/// to `Email` on decode failure). `LoginIdentity::parse` must check the
/// same length `decode_wallet_address` enforces everywhere else.
#[test]
fn a_wallet_remainder_that_is_not_thirty_two_bytes_is_not_a_wallet() {
    assert_eq!(
        LoginIdentity::parse("wallet:abc"),
        LoginIdentity::Email("wallet:abc".into())
    );
}

/// A stray `local:` key that is not exactly `local:owner` must not silently
/// merge into the one local-owner identity — that would collapse two
/// distinct stored records onto one key.
#[test]
fn a_local_prefixed_key_that_is_not_exactly_owner_is_not_local() {
    assert_ne!(LoginIdentity::parse("local:attacker"), LoginIdentity::Local);
    assert_eq!(
        LoginIdentity::parse("local:attacker"),
        LoginIdentity::Email("local:attacker".into())
    );
}

/// The guard that keeps `wallet:7xKX…` out of an SMTP envelope. Mail paths
/// ask for a mailbox rather than reading the column, so the absence of one
/// is a type, not a convention.
#[test]
fn only_an_email_identity_has_a_mailbox() {
    assert_eq!(
        LoginIdentity::Email("ada@example.com".into()).mailbox(),
        Some("ada@example.com")
    );
    assert_eq!(LoginIdentity::Wallet("7xKXtg2C".into()).mailbox(), None);
    assert_eq!(LoginIdentity::Local.mailbox(), None);
}

#[test]
fn wallet_addresses_decode_to_thirty_two_bytes() {
    // A real Solana-style address: 32 bytes of base58.
    let address = bs58::encode([7u8; 32]).into_string();
    assert_eq!(decode_wallet_address(&address).unwrap(), [7u8; 32]);
    // Whitespace is tolerated, since it is what a paste carries.
    assert!(decode_wallet_address(&format!("  {address} ")).is_ok());
}

/// Both refusals are prosumer-facing: they are rendered by manifest
/// validation, where the reader is an operator who typed the thing.
#[test]
fn a_bad_wallet_address_says_what_is_wrong_with_it() {
    // `0` is not in the base58 alphabet.
    let err = decode_wallet_address("0OIl").unwrap_err().to_string();
    assert!(err.contains("not a base58"), "{err}");

    // Valid base58, wrong length — the mistake a truncated paste makes.
    let short = bs58::encode([1u8; 16]).into_string();
    let err = decode_wallet_address(&short).unwrap_err().to_string();
    assert!(err.contains("16 bytes"), "{err}");
}

#[test]
fn only_admins_may_administer() {
    assert!(UserRole::Admin.may_administer());
    assert!(!UserRole::Member.may_administer());
}

#[test]
fn roles_and_statuses_default_to_least_privilege() {
    // A record deserialized without these fields must not become an admin.
    assert_eq!(UserRole::default(), UserRole::Member);
    assert_eq!(UserStatus::default(), UserStatus::Active);
}

#[test]
fn invite_is_redeemable_only_while_outstanding_and_unexpired() {
    let mut invite = InviteRecord {
        id: "i1".to_string(),
        email: "ada@example.com".to_string(),
        role: UserRole::Member,
        invited_by: "operator".to_string(),
        created_at_millis: 0,
        expires_at_millis: 100,
        accepted_at_millis: None,
        notified_at_millis: None,
    };
    assert!(invite.is_redeemable(99));
    // Expiry is exclusive: at the boundary the invite is already dead.
    assert!(!invite.is_redeemable(100));
    assert!(!invite.is_redeemable(101));

    invite.accepted_at_millis = Some(50);
    assert!(!invite.is_redeemable(60), "a redeemed invite is single-use");
}

/// The no-migration claim for issue #584, asserted rather than assumed.
///
/// Every store persists invites as a JSON blob, so the only thing standing
/// between an existing deployment and a boot failure is `serde(default)`.
/// This is a blob in the shape written *before* the field existed.
#[test]
fn an_invite_stored_before_invite_mail_loads_as_unmailed() {
    let legacy = serde_json::json!({
        "id": "i1",
        "email": "ada@example.com",
        "role": "member",
        "invitedBy": "u1",
        "createdAtMillis": 1,
        "expiresAtMillis": 100,
    });
    let invite: InviteRecord = serde_json::from_value(legacy).expect("a pre-#584 row loads");
    assert_eq!(
        invite.notified_at_millis, None,
        "a row written before invite mail must read as un-mailed, not as sent"
    );

    // And an unmailed invite serializes exactly as it did before the field
    // existed, so nothing downstream sees a new key it did not expect.
    let json = serde_json::to_value(&invite).unwrap();
    assert!(
        json.get("notifiedAtMillis").is_none(),
        "an unmailed invite must not emit the key: {json}"
    );

    let mailed = InviteRecord {
        notified_at_millis: Some(7),
        ..invite
    };
    assert_eq!(
        serde_json::to_value(&mailed).unwrap()["notifiedAtMillis"],
        7,
        "a mailed invite must report when"
    );
}

#[test]
fn user_record_round_trips_as_camel_case() {
    let user = UserRecord {
        id: "u1".to_string(),
        email: "ada@example.com".to_string(),
        display_name: Some("Ada".to_string()),
        avatar: None,
        role: UserRole::Admin,
        status: UserStatus::Active,
        password_hash: None,
        must_change_password: false,
        created_at_millis: 1,
        last_seen_at_millis: None,
        updated_at_millis: 2,
    };
    let json = serde_json::to_value(&user).unwrap();
    assert_eq!(json["createdAtMillis"], 1);
    assert_eq!(json["role"], "admin");
    assert_eq!(json["status"], "active");
    // Absent optionals stay absent rather than serializing as null.
    assert!(json.get("lastSeenAtMillis").is_none());
    assert!(
        json.get("passwordHash").is_none(),
        "a user with no password must not carry a null hash field"
    );
    assert_eq!(serde_json::from_value::<UserRecord>(json).unwrap(), user);
}

#[test]
fn a_user_stored_before_passwords_existed_still_loads() {
    // Records written by the magic-link-only build carry neither field.
    // They must load as "no password, nothing to change" rather than fail.
    let json = serde_json::json!({
        "id": "u1",
        "email": "ada@example.com",
        "role": "member",
        "status": "active",
        "createdAtMillis": 1,
        "updatedAtMillis": 2,
    });
    let user: UserRecord = serde_json::from_value(json).unwrap();
    assert_eq!(user.password_hash, None);
    assert!(!user.must_change_password);
}

#[test]
fn a_password_hash_round_trips_when_set() {
    let user = UserRecord {
        id: "u1".to_string(),
        email: "ada@example.com".to_string(),
        display_name: None,
        avatar: None,
        role: UserRole::Member,
        status: UserStatus::Active,
        password_hash: Some("$argon2id$v=19$...".to_string()),
        must_change_password: true,
        created_at_millis: 1,
        last_seen_at_millis: None,
        updated_at_millis: 2,
    };
    let json = serde_json::to_value(&user).unwrap();
    assert_eq!(json["mustChangePassword"], true);
    assert_eq!(serde_json::from_value::<UserRecord>(json).unwrap(), user);
}

#[test]
fn a_name_is_guessed_from_the_local_part() {
    for (identity, expected) in [
        ("steven.enamakel@acme.com", "Steven Enamakel"),
        ("steven_enamakel@acme.com", "Steven Enamakel"),
        ("steven-enamakel@acme.com", "Steven Enamakel"),
        // A routing tag is plumbing, not a middle name.
        ("steven+board@acme.com", "Steven"),
        ("stevent95@acme.com", "Stevent95"),
        // Already-capitalised local parts are left as written: lower-casing
        // the rest would turn McDonald into Mcdonald.
        ("McDonald@acme.com", "McDonald"),
        // The domain is dropped — it names the mailbox, not the person.
        ("ada@a.very.long.domain.example", "Ada"),
    ] {
        assert_eq!(
            derive_display_name(identity).as_deref(),
            Some(expected),
            "{identity}"
        );
    }
}

/// "Cannot say" is a real answer, and has to stay distinguishable from a
/// guess: a base58 key title-cased would *look* like a name.
#[test]
fn nothing_is_guessed_where_there_is_no_name() {
    for identity in [
        "wallet:7cVfgArCheMR6Cs29HGxwPFXhAxrJ6UP3TcTZqSKz8bE",
        "local:owner",
        "123.456@acme.com",
        "@acme.com",
    ] {
        assert_eq!(derive_display_name(identity), None, "{identity}");
    }
}

/// A chosen name always wins the guess, and a blank one is not a name.
#[test]
fn display_label_prefers_what_the_person_chose() {
    let mut user = UserRecord {
        id: "u1".to_string(),
        email: "steven.enamakel@acme.com".to_string(),
        display_name: Some("Steve".to_string()),
        avatar: None,
        role: UserRole::Member,
        status: UserStatus::Active,
        password_hash: None,
        must_change_password: false,
        created_at_millis: 1,
        last_seen_at_millis: None,
        updated_at_millis: 1,
    };
    assert_eq!(user.display_label().as_deref(), Some("Steve"));
    user.display_name = Some("   ".to_string());
    assert_eq!(user.display_label().as_deref(), Some("Steven Enamakel"));
    user.display_name = None;
    assert_eq!(user.display_label().as_deref(), Some("Steven Enamakel"));
}
