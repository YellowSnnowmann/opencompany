use super::composio_test_support::*;
use axum::http::StatusCode;
use serde_json::json;

/// A legacy-only BYOK key still passes the check route, and a subsequent
/// `PUT` mirrors the rotated value to both addresses.
#[tokio::test]
async fn the_api_key_test_route_reads_a_legacy_byok_key() {
    use crate::company::composio::{BYOK_KEY_KEY, LEGACY_API_KEY_KEY, MODE_KEY};

    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "legacybyok", GRANTED).await;
    let runtime = runtime_of(&state, "legacybyok");
    runtime
        .secrets()
        .set(
            runtime.id(),
            MODE_KEY,
            crate::ports::types::SecretValue(crate::company::composio::BYOK_MODE.to_string()),
        )
        .await
        .unwrap();
    runtime
        .secrets()
        .set(
            runtime.id(),
            LEGACY_API_KEY_KEY,
            crate::ports::types::SecretValue("ak-not-a-real-key".into()),
        )
        .await
        .unwrap();
    super::probe_override::set("legacybyok", Ok(()));

    let (code, body, raw) = send_for(
        &state,
        "legacybyok",
        "POST",
        "/api/v1/company/composio/api-key/test",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{raw}");
    assert_eq!(body["ok"], true, "{body}");

    let (code, _, raw) = send_for(
        &state,
        "legacybyok",
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": "ak-not-a-real-key-2" })),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{raw}");

    assert_eq!(
        read_slot(&runtime, BYOK_KEY_KEY).await.as_deref(),
        Some("ak-not-a-real-key-2")
    );
    assert_eq!(
        read_slot(&runtime, LEGACY_API_KEY_KEY).await.as_deref(),
        Some("ak-not-a-real-key-2")
    );
    assert_eq!(
        read_slot(&runtime, MODE_KEY).await.as_deref(),
        Some(crate::company::composio::BYOK_MODE)
    );
    assert!(
        !raw.contains("ak-not-a-real-key"),
        "the PUT leaked the key: {raw}"
    );
}

/// Spending the company's Composio credential against a third party is a
/// decision taken on the company's behalf, so a member cannot trigger it —
/// the same boundary the writes on this surface carry (issue #403).
#[tokio::test]
async fn a_member_cannot_test_the_company_s_composio_key() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), GRANTED).await;
    let member =
        crate::server::test_support::seed_session(&state, "acme", crate::ports::UserRole::Member)
            .await;
    let (code, body, raw) = send_as(
        &state,
        "POST",
        "/api/v1/company/composio/api-key/test",
        None,
        Auth::Cookie(member),
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN, "{raw}");
    assert_eq!(body["code"], "forbidden", "{body}");
}

/// `POST …/composio/tinyhumans/key/from-account` copies the account key
/// into the Composio slot, evicts the cached catalog and reports the one
/// slot it touched (keys rework #2306, slice 4c).
#[tokio::test]
async fn the_from_account_route_fills_the_composio_key_and_reports_one_slot() {
    use crate::company::composio::TINYHUMANS_KEY_KEY;

    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "reuse-composio", GRANTED).await;
    let runtime = runtime_of(&state, "reuse-composio");
    runtime
        .secrets()
        .set(
            runtime.id(),
            crate::company::company_key::KEY_KEY,
            crate::ports::types::SecretValue("th-not-a-real-account-key".into()),
        )
        .await
        .unwrap();

    let (code, body, raw) = send_for(
        &state,
        "reuse-composio",
        "POST",
        "/api/v1/company/composio/tinyhumans/key/from-account",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{raw}");
    assert_eq!(
        read_slot(&runtime, TINYHUMANS_KEY_KEY).await.as_deref(),
        Some("th-not-a-real-account-key")
    );
    let slots = body["slots"].as_array().expect("slots array");
    assert_eq!(slots.len(), 1, "{body}");
    assert_eq!(slots[0]["slot"], "composio", "{body}");
    assert_eq!(slots[0]["outcome"], "filled", "{body}");
    assert!(
        body["note"]
            .as_str()
            .is_some_and(|n| n.contains("Composio now uses your account key")),
        "{body}"
    );
    assert!(
        !raw.contains("th-not-a-real-account-key"),
        "the response leaked the key: {raw}"
    );
}

/// P3-3 (keys rework #2306 review): a `Filled` copy is journaled — the
/// counterpart to `copying_an_already_current_key_does_not_journal`
/// below, which proves the opposite for `Kept`.
#[tokio::test]
async fn copying_a_new_composio_key_journals_the_fill() {
    use crate::ports::types::{CompanyEvent, CompanyId, EventSeq};

    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "reuse-journal-fill", GRANTED).await;
    let runtime = runtime_of(&state, "reuse-journal-fill");
    runtime
        .secrets()
        .set(
            runtime.id(),
            crate::company::company_key::KEY_KEY,
            crate::ports::types::SecretValue("th-not-a-real-account-key".into()),
        )
        .await
        .unwrap();

    let (code, _, raw) = send_for(
        &state,
        "reuse-journal-fill",
        "POST",
        "/api/v1/company/composio/tinyhumans/key/from-account",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{raw}");

    let events = runtime
        .events()
        .read_from(&CompanyId::new("reuse-journal-fill"), EventSeq::new(0), 100)
        .await
        .unwrap();
    let journaled = events.iter().any(|stored| {
        matches!(
            &stored.event,
            CompanyEvent::ToolAccessChanged { change, .. }
                if change == "company_key_composio_filled"
        )
    });
    assert!(journaled, "a Filled copy must be journaled: {events:?}");
}

/// P3-3 (keys rework #2306 review): a copy whose outcome is
/// `Kept(AlreadyCurrent)` — the Composio slot already held exactly the
/// account key's value — changes no stored state, so it must not add a
/// journal entry either, matching 4a §3.5's "no entry for kept" rule.
#[tokio::test]
async fn copying_an_already_current_key_does_not_journal() {
    use crate::ports::types::{CompanyEvent, CompanyId, EventSeq};

    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "reuse-journal-kept", GRANTED).await;
    let runtime = runtime_of(&state, "reuse-journal-kept");
    const ACCOUNT_KEY: &str = "th-not-a-real-account-key";
    runtime
        .secrets()
        .set(
            runtime.id(),
            crate::company::company_key::KEY_KEY,
            crate::ports::types::SecretValue(ACCOUNT_KEY.into()),
        )
        .await
        .unwrap();
    // The Composio slot already agrees with the account key, so the copy
    // is a no-op (`Kept(AlreadyCurrent)`), not a fill.
    runtime
        .secrets()
        .set(
            runtime.id(),
            crate::company::composio::TINYHUMANS_KEY_KEY,
            crate::ports::types::SecretValue(ACCOUNT_KEY.into()),
        )
        .await
        .unwrap();

    let (code, body, raw) = send_for(
        &state,
        "reuse-journal-kept",
        "POST",
        "/api/v1/company/composio/tinyhumans/key/from-account",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{raw}");
    assert_eq!(body["slots"][0]["outcome"], "kept", "{body}");

    let events = runtime
        .events()
        .read_from(&CompanyId::new("reuse-journal-kept"), EventSeq::new(0), 100)
        .await
        .unwrap();
    let journaled = events.iter().any(|stored| {
        matches!(
            &stored.event,
            CompanyEvent::ToolAccessChanged { change, .. }
                if change == "company_key_composio_filled"
        )
    });
    assert!(
        !journaled,
        "a Kept(AlreadyCurrent) copy changed nothing and must not journal: {events:?}"
    );
}

/// Copying with no account key on file is refused before any write.
#[tokio::test]
async fn the_from_account_route_refuses_without_an_account_key() {
    use crate::company::composio::TINYHUMANS_KEY_KEY;

    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "reuse-no-key", GRANTED).await;
    let runtime = runtime_of(&state, "reuse-no-key");

    let (code, body, raw) = send_for(
        &state,
        "reuse-no-key",
        "POST",
        "/api/v1/company/composio/tinyhumans/key/from-account",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{raw}");
    assert_eq!(body["code"], "invalid_request", "{body}");
    assert_eq!(
        read_slot(&runtime, TINYHUMANS_KEY_KEY).await,
        None,
        "nothing is written on a refusal"
    );
}

/// Whose account the company's Composio calls present is an admin's
/// decision, exactly as the token and API-key writes on this surface are
/// (issue #403).
#[tokio::test]
async fn a_member_cannot_copy_the_account_key_to_composio() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "reuse-member", GRANTED).await;
    let runtime = runtime_of(&state, "reuse-member");
    runtime
        .secrets()
        .set(
            runtime.id(),
            crate::company::company_key::KEY_KEY,
            crate::ports::types::SecretValue("th-not-a-real-account-key".into()),
        )
        .await
        .unwrap();
    let member = crate::server::test_support::seed_session(
        &state,
        "reuse-member",
        crate::ports::UserRole::Member,
    )
    .await;

    let (code, body, raw) = send_as(
        &state,
        "POST",
        "/api/v1/company/composio/tinyhumans/key/from-account",
        None,
        Auth::Cookie(member),
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN, "{raw}");
    assert_eq!(body["code"], "forbidden", "{body}");
}

/// A build with no Composio client cannot check a key — and treats that as
/// **un-probeable**, not as a rejected credential.
///
/// No override: this drives the real `probe_transport`, which in this build
/// is the honest "not compiled in" error. Refusing the write here would make
/// BYOK unconfigurable on the default build; classifying an absent client as
/// `auth` would throw away a key nothing ever looked at.
#[cfg(not(feature = "composio"))]
#[tokio::test]
async fn a_build_without_the_composio_client_stores_the_key_and_says_it_could_not_check() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "probenobuild", GRANTED).await;

    let (code, resp, raw) = send_for(
        &state,
        "probenobuild",
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": "ak_not_a_real_key_0123456789" })),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{raw}");
    assert_eq!(resp["status"]["mode"], "byok", "{resp}");
    assert_eq!(resp["probeClass"], "unknown", "{resp}");
}

/// Whose Composio account a company acts through — and therefore who pays
/// for the calls — is an admin's decision, exactly as the backend token is.
#[tokio::test]
async fn a_member_cannot_bring_its_own_composio_account() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), GRANTED).await;
    let member =
        crate::server::test_support::seed_session(&state, "acme", crate::ports::UserRole::Member)
            .await;

    let (status, body, raw) = send_as(
        &state,
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": "ak_live" })),
        Auth::Cookie(member),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{raw}");
    assert_eq!(body["code"], "forbidden", "{body}");

    // The refusal is real: the route did not move.
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/composio", None).await;
    assert_eq!(
        dto["mode"], "managed",
        "a refused write stored nothing: {dto}"
    );
}

/// A managed-route token stored by a company that is **on BYOK** must not
/// claim agents have started using it.
///
/// The state is newly reachable: a BYOK company whose managed chain resolves
/// to nothing cannot be offered "Use this" — that would be switching into an
/// outage — so the console offers the token first and the switch second, and
/// that is the only order that works. `SWITCH_NOTE` at the end of the first
/// step would report the second as already done, while `resolve_access` is
/// still reading the BYOK key.
#[tokio::test]
async fn a_token_for_the_route_a_company_is_not_on_does_not_claim_effect() {
    use crate::company::composio::store_api_key;

    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "inactivetoken", GRANTED).await;
    let runtime = runtime_of(&state, "inactivetoken");
    // Straight to the store rather than through the route: the route probes
    // the draft key, and this test is about the note, not the probe.
    store_api_key(
        runtime.id(),
        runtime.secrets().as_ref(),
        "ak_not_a_real_key_0123456789",
    )
    .await
    .unwrap();

    let (_, resp, _) = send_for(
        &state,
        "inactivetoken",
        "PUT",
        "/api/v1/company/composio/token",
        Some(json!({ "token": TOKEN })),
    )
    .await;
    let note = resp["note"].as_str().expect("a note").to_string();
    assert!(
        !note.contains("next turn"),
        "the company is on BYOK, so agents do not pick this up next turn: {note}"
    );
    assert!(
        note.contains("still on its own Composio account"),
        "the note says why the token is not in effect: {note}"
    );
}

/// The note on a write names the operation (issue #1471): a set tells the
/// operator a new token is live, a clear must not claim one exists — the
/// effective credential after a clear is whatever tier remains, which may
/// be nothing at all.
#[tokio::test]
async fn the_clear_note_does_not_claim_a_new_token() {
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), GRANTED).await;

    let (_, resp, _) = send(
        &state,
        "PUT",
        "/api/v1/company/composio/token",
        Some(json!({ "token": TOKEN })),
    )
    .await;
    let set_note = resp["note"].as_str().expect("a note").to_string();
    assert!(
        set_note.contains("new Composio token"),
        "a set announces the new token: {set_note}"
    );

    // Guarded while the company is on the managed route
    // (in-use-guards.md §2); this test is about the note text, not the
    // guard, so it confirms.
    let (_, resp, _) = send(
        &state,
        "PUT",
        "/api/v1/company/composio/token",
        Some(json!({ "token": "", "confirmInUse": true })),
    )
    .await;
    let clear_note = resp["note"].as_str().expect("a note").to_string();
    assert_ne!(
        clear_note, set_note,
        "set and clear are told apart: {clear_note}"
    );
    assert!(
        !clear_note.contains("new Composio token"),
        "a clear must not invent a token that no longer exists: {clear_note}"
    );
    assert!(
        clear_note.contains("cleared"),
        "the clear names what it did: {clear_note}"
    );
}
