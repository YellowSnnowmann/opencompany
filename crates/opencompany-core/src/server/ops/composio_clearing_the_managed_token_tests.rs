use super::composio_test_support::*;
use axum::http::StatusCode;
use serde_json::json;

/// A managed-token clear needs no confirmation while the company is on
/// BYOK: `composio/mode` no longer selects the managed slot, so this
/// clear is not touching what any live call resolves through, and the
/// response carries no `usedBy` — omitted, not null or empty.
#[tokio::test]
async fn clearing_the_managed_token_while_mode_is_byok_needs_no_confirmation() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "clearunguarded", GRANTED).await;
    let runtime = runtime_of(&state, "clearunguarded");
    crate::company::composio::store_token(runtime.id(), runtime.secrets().as_ref(), TOKEN)
        .await
        .unwrap();
    crate::company::composio::store_api_key(
        runtime.id(),
        runtime.secrets().as_ref(),
        "ak_not_a_real_key_0123456789",
    )
    .await
    .unwrap();

    let (status, body, raw) = send_for(
        &state,
        "clearunguarded",
        "PUT",
        "/api/v1/company/composio/token",
        Some(json!({ "token": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert!(body.get("usedBy").is_none(), "{body}");
    assert_eq!(
        crate::company::composio::load_tinyhumans_key(runtime.id(), runtime.secrets().as_ref())
            .await
            .unwrap(),
        None
    );
}

/// Setting or rotating a non-empty managed token is never guarded — only
/// a clear can strand anything the mode currently resolves through.
#[tokio::test]
async fn setting_a_managed_token_is_never_guarded_even_while_mode_is_managed() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "setnoguard", GRANTED).await;

    let (status, body, raw) = send_for(
        &state,
        "setnoguard",
        "PUT",
        "/api/v1/company/composio/token",
        Some(json!({ "token": TOKEN })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert!(body.get("usedBy").is_none(), "a set is not guarded: {body}");
}

/// The first move onto BYOK needs no confirmation from Composio's own
/// guard: before the write `composio/mode` still reads `"managed"`, so
/// `composio/byok/key` is not the slot the mode currently selects, and
/// setting it strands nothing (in-use-guards.md §2 — the criterion is
/// the mode BEFORE the write, not the one the write is heading to). The
/// probe is forced clean so this does not depend on network access under
/// the `composio` feature.
#[tokio::test]
async fn switching_to_byok_while_mode_is_managed_needs_no_confirmation() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "byokswitchunguarded", GRANTED).await;
    let runtime = runtime_of(&state, "byokswitchunguarded");
    super::probe_override::set("byokswitchunguarded", Ok(()));

    let (status, body, raw) = send_for(
        &state,
        "byokswitchunguarded",
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": "ak_not_a_real_key_0123456789" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert!(body.get("usedBy").is_none(), "{body}");
    assert_eq!(
        crate::company::composio::load_mode(runtime.id(), runtime.secrets().as_ref())
            .await
            .unwrap(),
        crate::company::composio::ComposioMode::Byok
    );
}

/// Round-2 review, comment 4012457339 (keys rework #2306): `set_api_key`
/// writes `composio/mode`, the same fact `company_key::fan_out`'s own
/// in-use check reads to decide whether an unconfirmed account-key clear
/// may touch `composio/tinyhumans/key`. Before this fix the route took no
/// lock at all, so a confirmed mode switch landing here and a concurrent,
/// unconfirmed account-key clear could each act on the OTHER's pre-image
/// — the clear sees the old mode and decides the managed slot is
/// inactive, the switch then makes it active, and it is left keyless with
/// neither request ever having confirmed that outcome.
///
/// This proves the route now shares `company_key::fan_out`'s own
/// `slot_guard`: while a test holds that company's guard directly (the
/// same acquisition `fan_out` makes for the whole of an account-key
/// save), the real `PUT …/composio/api-key` handler must not complete —
/// it has to be waiting on the same lock, not racing past it.
#[tokio::test]
async fn set_api_key_blocks_while_the_account_keys_fan_out_lock_is_held() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "composio-lock-blocks", GRANTED).await;
    let runtime = runtime_of(&state, "composio-lock-blocks");
    super::probe_override::set("composio-lock-blocks", Ok(()));

    // The exact acquisition `company_key::fan_out` makes for the whole of
    // an account-key save.
    let held = crate::company::company_key::slot_guard(runtime.id()).await;

    let request = send_for(
        &state,
        "composio-lock-blocks",
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": "ak_not_a_real_key_0123456789" })),
    );
    tokio::pin!(request);
    tokio::select! {
        _ = &mut request => panic!(
            "set_api_key must not write composio/mode while the account-key \
             fan-out's own lock is held elsewhere"
        ),
        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
    }

    drop(held);
    let (status, _body, raw) =
        tokio::time::timeout(std::time::Duration::from_millis(1000), request)
            .await
            .expect("set_api_key proceeds once the lock is released");
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(
        crate::company::composio::load_mode(runtime.id(), runtime.secrets().as_ref())
            .await
            .unwrap(),
        crate::company::composio::ComposioMode::Byok
    );
}

/// Giving the managed route back while BYOK is active IS refused without
/// confirmation: `composio/mode` already reads `"byok"`, which is
/// exactly the slot `composio/byok/key` being cleared belongs to.
#[tokio::test]
async fn switching_back_to_managed_while_mode_is_byok_is_refused_without_confirmation() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "managedswitchguard", GRANTED).await;
    let runtime = runtime_of(&state, "managedswitchguard");
    crate::company::composio::store_api_key(
        runtime.id(),
        runtime.secrets().as_ref(),
        "ak_not_a_real_key_0123456789",
    )
    .await
    .unwrap();

    let (status, body, raw) = send_for(
        &state,
        "managedswitchguard",
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{raw}");
    assert_eq!(body["code"], "in_use", "{body}");
    assert_eq!(
        body["usedBy"],
        json!({ "surfaces": ["composio"] }),
        "{body}"
    );

    // Refused: the company is still on BYOK, key untouched.
    assert_eq!(
        crate::company::composio::load_mode(runtime.id(), runtime.secrets().as_ref())
            .await
            .unwrap(),
        crate::company::composio::ComposioMode::Byok
    );
}

/// The same switch back to managed, confirmed, lands and echoes
/// `usedBy` (in-use-guards.md §3).
#[tokio::test]
async fn a_confirmed_switch_back_to_managed_succeeds_and_echoes_used_by() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "managedswitchconfirmed", GRANTED).await;
    let runtime = runtime_of(&state, "managedswitchconfirmed");
    crate::company::composio::store_api_key(
        runtime.id(),
        runtime.secrets().as_ref(),
        "ak_not_a_real_key_0123456789",
    )
    .await
    .unwrap();

    let (status, body, raw) = send_for(
        &state,
        "managedswitchconfirmed",
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": "", "confirmInUse": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(
        body["usedBy"],
        json!({ "surfaces": ["composio"] }),
        "{body}"
    );
    assert_eq!(
        crate::company::composio::load_mode(runtime.id(), runtime.secrets().as_ref())
            .await
            .unwrap(),
        crate::company::composio::ComposioMode::Managed
    );
}

/// Rotating a key while staying on the SAME route is never guarded — a
/// rotate is not a switch at all (in-use-guards.md §2), so the mode
/// match is never even consulted.
#[tokio::test]
async fn rotating_an_already_byok_key_is_never_guarded() {
    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "byokrotate", GRANTED).await;
    let runtime = runtime_of(&state, "byokrotate");
    crate::company::composio::store_api_key(
        runtime.id(),
        runtime.secrets().as_ref(),
        "ak_not_a_real_key_0123456789",
    )
    .await
    .unwrap();
    super::probe_override::set("byokrotate", Ok(()));

    let (status, body, raw) = send_for(
        &state,
        "byokrotate",
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": "ak_not_a_real_key_9999999999" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert!(
        body.get("usedBy").is_none(),
        "a same-route rotation is not a switch: {body}"
    );
}
