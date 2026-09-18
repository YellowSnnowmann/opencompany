use super::composio_test_support::*;
use crate::AppState;
use crate::server::ops::composio_toolkits;
use axum::http::StatusCode;
use serde_json::{Value, json};

/// Issue #397: an **empty** manifest allowlist means "defer to the backend"
/// — allow everything — so the status must report open mode and hand the
/// console a non-empty starting set. The old console gate keyed off
/// `toolkits.length > 0` and therefore rendered nothing in exactly the case
/// where everything is permitted.
#[tokio::test]
async fn an_empty_toolkit_list_is_open_mode_and_still_offers_providers() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    // No `[tools.composio]` section at all — the shape 19 of 20 templates ship.
    let state = state_with_manifest(
        &home,
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n[tools]\nallow = [\"composio\"]\n",
    )
    .await;

    let (status, dto, _) = send(&state, "GET", "/api/v1/company/composio", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(dto["granted"], true);
    assert_eq!(dto["toolkits"], json!([]), "the manifest list is verbatim");
    assert_eq!(dto["openMode"], true);
    let offered = dto["effectiveToolkits"]
        .as_array()
        .expect("open mode carries an effective toolkit list");
    assert!(
        !offered.is_empty(),
        "open mode means allow-everything, so the console must be offered providers"
    );
    assert!(
        offered.iter().any(|t| t == "github"),
        "the fallback set covers the common providers: {offered:?}"
    );
    // No credential and (in the default build) no client, so the catalog
    // cannot be read. The list is still offered — but it says so.
    assert_eq!(
        dto["catalogSource"], "fallback",
        "an unfetchable catalog must never be reported as the backend's: {dto}"
    );
    assert!(
        dto["catalogNotice"]
            .as_str()
            .is_some_and(|n| n.contains("incomplete")),
        "a fallback tells the operator it may be incomplete: {dto}"
    );
}

/// The heart of the reopened issue: in open mode the console is offered the
/// **backend's** catalog, not a list maintained by hand in this repo.
///
/// The cache is seeded rather than a backend stood up, because what is under
/// test is the decision — "open mode serves the fetched catalog" — and not
/// the HTTP call, which has its own test beside
/// `list_catalog_toolkits`. Seeding proves the thing that regressed: that a
/// fetched answer actually reaches the response instead of being discarded
/// in favour of the constant.
#[tokio::test]
async fn open_mode_serves_the_fetched_catalog_rather_than_a_hardcoded_list() {
    #[cfg(feature = "composio")]
    let _env = composio_backend_env_guard();
    let home_dir = home();
    let state = state_with_manifest_id(
        home_dir.path(),
        "catalogco",
        "[company]\nname = \"Catalog Co\"\n[policy]\nmode = \"full\"\n[tools]\nallow = [\"composio\"]\n",
    )
    .await;
    let catalog = hundred_entries();
    composio_toolkits::cache().store(
        &super::catalog_cache_key(runtime_of(&state, "catalogco").as_ref()),
        Ok(catalog.clone()),
        std::time::Instant::now(),
    );

    let (status, dto, raw) =
        send_for(&state, "catalogco", "GET", "/api/v1/company/composio", None).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(dto["openMode"], true);
    assert_eq!(
        dto["catalogSource"], "backend",
        "a real catalog is reported as the backend's answer: {dto}"
    );
    assert_eq!(
        dto["catalogNotice"],
        Value::Null,
        "nothing to apologise for"
    );
    assert_eq!(
        dto["effectiveToolkits"],
        json!(hundred_slugs()),
        "open mode must serve what the backend permits, verbatim"
    );
    assert!(
        dto["effectiveToolkits"].as_array().unwrap().len()
            > composio_toolkits::FALLBACK_TOOLKITS.len(),
        "the fetched catalog is far longer than the built-in list — serving the \
         constant here is the exact regression this test exists to catch: {dto}"
    );
}

/// The important one. A company that deliberately narrowed its belt must
/// **not** be silently widened by the catalog.
///
/// The same hundred-slug catalog is in the cache; this company asked for
/// `gmail` and gets `gmail`. Anything that unioned, defaulted-to, or fell
/// back to the catalog for an explicit manifest would hand the company
/// ninety-nine providers it decided against.
#[tokio::test]
async fn an_explicit_allowlist_is_never_widened_by_the_catalog() {
    #[cfg(feature = "composio")]
    let _env = composio_backend_env_guard();
    let home_dir = home();
    let state = state_with_manifest_id(
        home_dir.path(),
        "narrowco",
        "[company]\nname = \"Narrow Co\"\n[policy]\nmode = \"full\"\n[tools]\nallow = [\"composio\"]\n[tools.composio]\ntoolkits = [\"gmail\"]\n",
    )
    .await;
    composio_toolkits::cache().store(
        &super::catalog_cache_key(runtime_of(&state, "narrowco").as_ref()),
        Ok(hundred_entries()),
        std::time::Instant::now(),
    );

    let (status, dto, raw) =
        send_for(&state, "narrowco", "GET", "/api/v1/company/composio", None).await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(dto["openMode"], false);
    assert_eq!(dto["effectiveToolkits"], json!(["gmail"]));
    assert_eq!(dto["toolkits"], json!(["gmail"]));
    assert_eq!(
        dto["catalogSource"], "manifest",
        "the company's own list is the source, and the catalog is not consulted: {dto}"
    );
}

/// A catalog that cannot be fetched degrades to the built-in list — and the
/// response says so, both in `catalogSource` and in words the console can
/// show the operator. Silently serving eight slugs that look like the whole
/// catalog is the failure this pins shut.
#[tokio::test]
async fn an_unfetchable_catalog_is_marked_degraded_not_passed_off_as_real() {
    #[cfg(feature = "composio")]
    let _env = composio_backend_env_guard();
    let home_dir = home();
    let state = state_with_manifest_id(
        home_dir.path(),
        "degradedco",
        "[company]\nname = \"Degraded Co\"\n[policy]\nmode = \"full\"\n[tools]\nallow = [\"composio\"]\n",
    )
    .await;
    composio_toolkits::cache().store(
        &super::catalog_cache_key(runtime_of(&state, "degradedco").as_ref()),
        Err("connection refused".to_string()),
        std::time::Instant::now(),
    );

    let (status, dto, raw) = send_for(
        &state,
        "degradedco",
        "GET",
        "/api/v1/company/composio",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(dto["openMode"], true);
    assert_eq!(
        dto["catalogSource"], "fallback",
        "the built-in list must never claim to be the backend's: {dto}"
    );
    let notice = dto["catalogNotice"]
        .as_str()
        .expect("a degraded list must explain itself");
    assert!(notice.contains("connection refused"), "{notice}");
    assert!(notice.contains("may be incomplete"), "{notice}");
    // Still usable: the operator gets something to click, just not a claim.
    assert_eq!(
        dto["effectiveToolkits"],
        json!(composio_toolkits::FALLBACK_TOOLKITS)
    );
}

/// A credential change drops the cached catalog: a rotated BYO token can
/// resolve to a different Composio account, and serving the previous
/// account's list for the rest of the TTL is the stale-by-construction
/// answer this issue is about.
///
/// Driven through a *clear* rather than a set so the status re-read at the
/// end of the write has no credential to dial with — the assertion is about
/// the eviction, and a test that reached the network to make it would be a
/// different test.
#[tokio::test]
async fn a_credential_change_evicts_the_cached_catalog() {
    #[cfg(feature = "composio")]
    let _env = composio_backend_env_guard();
    let home_dir = home();
    let state = state_with_manifest_id(
        home_dir.path(),
        "rotateco",
        "[company]\nname = \"Rotate Co\"\n[policy]\nmode = \"full\"\n[tools]\nallow = [\"composio\"]\n",
    )
    .await;
    let key = super::catalog_cache_key(runtime_of(&state, "rotateco").as_ref());
    composio_toolkits::cache().store(&key, Ok(hundred_entries()), std::time::Instant::now());

    let (status, resp, raw) = send_for(
        &state,
        "rotateco",
        "PUT",
        "/api/v1/company/composio/token",
        // The managed token clear is guarded while the company is on the
        // managed route (in-use-guards.md §2) — this test is about
        // catalog eviction, not the guard, so it confirms.
        Some(json!({ "token": "", "confirmInUse": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_ne!(
        resp["status"]["catalogSource"], "backend",
        "the pre-change catalog must not survive the change: {resp}"
    );
}

/// The other half of #397: a company that deliberately narrowed its belt
/// sees exactly what it chose, and is NOT in open mode. A fix that offered
/// the curated set to everyone would silently widen every restrictive
/// manifest.
#[tokio::test]
async fn an_explicit_toolkit_list_is_offered_verbatim_and_is_not_open_mode() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(
        &home,
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n[tools]\nallow = [\"composio\"]\n[tools.composio]\ntoolkits = [\"gmail\"]\n",
    )
    .await;

    let (status, dto, _) = send(&state, "GET", "/api/v1/company/composio", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(dto["openMode"], false);
    assert_eq!(dto["effectiveToolkits"], json!(["gmail"]));
    assert_eq!(dto["toolkits"], json!(["gmail"]));
}

/// The `credentialSource` matrix as the console consumes it, plus the
/// write-only contract. Runs with no platform identity in the environment, so
/// the instance contributes nothing and the company's own token is the only
/// credential in play: `none` → `static` → `none`.
#[tokio::test]
async fn credential_source_tracks_the_byo_token_and_never_carries_it() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_manifest(
        &home,
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n[tools]\nallow = [\"composio\"]\n[tools.composio]\ntoolkits = [\"gmail\", \"github\"]\n",
    )
    .await;

    // Initial status: granted, toolkits surfaced, no credential obtainable.
    let (status, dto, _) = send(&state, "GET", "/api/v1/company/composio", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(dto["granted"], true);
    assert_eq!(dto["credentialSource"], "none");
    assert_eq!(dto["toolkits"], json!(["gmail", "github"]));
    assert!(dto.get("backendUrl").is_some());
    assert!(
        dto.get("token").is_none(),
        "status must never carry a token"
    );
    assert!(
        dto.get("tokenConfigured").is_none(),
        "the boolean surface is replaced by credentialSource"
    );

    // Set the write-only BYO token → the static tier.
    let (status, resp, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/composio/token",
        Some(json!({ "token": TOKEN })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(resp["status"]["credentialSource"], "static");
    assert!(!raw.contains(TOKEN), "PUT response leaked the token: {raw}");

    // GET reflects it and still never carries the token.
    let (_, dto, raw) = send(&state, "GET", "/api/v1/company/composio", None).await;
    assert_eq!(dto["credentialSource"], "static");
    assert!(!raw.contains(TOKEN), "GET status leaked the token: {raw}");

    // Clearing with "" reverts to whatever the instance offers — here
    // nothing. Guarded while the company is on the managed route
    // (in-use-guards.md §2); this test is about credentialSource, not
    // the guard, so it confirms.
    let (_, resp, _) = send(
        &state,
        "PUT",
        "/api/v1/company/composio/token",
        Some(json!({ "token": "", "confirmInUse": true })),
    )
    .await;
    assert_eq!(resp["status"]["credentialSource"], "none");
}

/// BYOK end to end over the route: storing a Composio API key moves the
/// company onto its own account, the read plane says so without ever
/// carrying the key, and clearing it hands the managed route back.
#[tokio::test]
async fn a_company_can_bring_its_own_composio_account_and_give_it_back() {
    const API_KEY: &str = "ak_not_a_real_key_0123456789";
    let home_dir = home();
    let state = state_with_manifest(home_dir.path(), GRANTED).await;
    // This test is about storing and clearing, not about the check. Pin the
    // probe clean so a `composio` build runs it against the override rather
    // than against `backend.composio.dev`.
    super::probe_override::set("acme", Ok(()));

    // Managed is the default and needs nothing stored.
    let (_, dto, _) = send(&state, "GET", "/api/v1/company/composio", None).await;
    assert_eq!(dto["mode"], "managed");

    let (status, resp, raw) = send(
        &state,
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": API_KEY })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    assert_eq!(resp["status"]["mode"], "byok");
    assert_eq!(
        resp["status"]["credentialSource"], "static",
        "the key is this company's own, which is the static tier"
    );
    assert!(
        !raw.contains(API_KEY),
        "the PUT response leaked the key: {raw}"
    );

    // The read plane reports the route and the host it reaches, and still
    // carries no credential.
    let (_, dto, raw) = send(&state, "GET", "/api/v1/company/composio", None).await;
    assert_eq!(dto["mode"], "byok");
    assert_eq!(
        dto["backendUrl"],
        crate::company::composio::DIRECT_BASE_URL,
        "a status still naming the managed backend would read as though nothing changed"
    );
    assert!(
        !raw.contains(API_KEY),
        "the GET status leaked the key: {raw}"
    );

    // Clearing it returns the company to the managed route. Guarded
    // while the company is on BYOK (in-use-guards.md §2); this test is
    // about the route round-trip, not the guard, so it confirms.
    let (_, resp, _) = send(
        &state,
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": "", "confirmInUse": true })),
    )
    .await;
    assert_eq!(resp["status"]["mode"], "managed");
    assert_ne!(
        resp["status"]["backendUrl"],
        crate::company::composio::DIRECT_BASE_URL
    );
}

// ── The managed tier, reported independently of the mode (#2275) ────

/// The field's whole reason to exist: under BYOK, `credentialSource` names
/// the Composio key the agents present, and `managedCredentialSource` still
/// names the tier the **managed** route would answer with.
///
/// Driven over the real route rather than over the resolver, because what
/// regressed before is not the chain — it is whether the chain's answer
/// survives the short-circuit `resolve_access` takes under BYOK and reaches
/// the DTO at all.
#[tokio::test]
async fn the_managed_tier_is_reported_while_byok_is_selected() {
    const BYOK_KEY: &str = "ak_not_a_real_key_0123456789";
    use crate::company::company_key;
    use crate::company::composio::store_token;

    let home_dir = home();
    let state = state_with_manifest_id(home_dir.path(), "managedtier", GRANTED).await;
    // The check is not what this test is about; pin it clean so a
    // `composio` build answers from the override instead of the network.
    super::probe_override::set("managedtier", Ok(()));
    let runtime = runtime_of(&state, "managedtier");
    let secrets = runtime.secrets();

    async fn status(state: &AppState) -> (Value, String) {
        let (code, dto, raw) = send_for(
            state,
            "managedtier",
            "GET",
            "/api/v1/company/composio",
            None,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{raw}");
        (dto, raw)
    }

    // Nothing stored, and (in the test process) no instance identity: both
    // fields agree, and agree on nothing.
    let (dto, _) = status(&state).await;
    assert_eq!(dto["credentialSource"], "none");
    assert_eq!(dto["managedCredentialSource"], "none");

    // A pasted `composio/tinyhumans/key` (formerly `composio/token`) is the
    // managed chain's first tier.
    store_token(runtime.id(), secrets.as_ref(), "byo-managed-bearer")
        .await
        .unwrap();
    let (dto, _) = status(&state).await;
    assert_eq!(dto["credentialSource"], "static");
    assert_eq!(
        dto["managedCredentialSource"], "static",
        "under managed the two fields come from the same resolver and cannot differ: {dto}"
    );

    // Withdraw it over the company's own TinyHumans key: the chain falls
    // exactly one tier, and both fields follow it.
    store_token(runtime.id(), secrets.as_ref(), "")
        .await
        .unwrap();
    company_key::store_key(runtime.id(), secrets.as_ref(), "th_company")
        .await
        .unwrap();
    let (dto, _) = status(&state).await;
    assert_eq!(dto["credentialSource"], "company");
    assert_eq!(dto["managedCredentialSource"], "company");

    // The point. BYOK is selected, so the agents present the Composio key —
    // `static`. The managed route is untouched underneath, and the console
    // can now say so without making the operator clear the key to find out.
    let (code, resp, raw) = send_for(
        &state,
        "managedtier",
        "PUT",
        "/api/v1/company/composio/api-key",
        Some(json!({ "apiKey": BYOK_KEY })),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{raw}");
    assert_eq!(resp["status"]["mode"], "byok");
    assert_eq!(resp["status"]["credentialSource"], "static");
    assert_eq!(
        resp["status"]["managedCredentialSource"], "company",
        "BYOK must not be able to hide which tier the managed route would answer with: {resp}"
    );
    assert!(!raw.contains(BYOK_KEY), "the PUT leaked the key: {raw}");

    let (dto, raw) = status(&state).await;
    assert_eq!(dto["credentialSource"], "static");
    assert_eq!(dto["managedCredentialSource"], "company");
    assert!(!raw.contains(BYOK_KEY), "the GET leaked the key: {raw}");
    assert!(
        dto.get("token").is_none(),
        "status must never carry a token"
    );
    assert!(
        dto.get("tokenConfigured").is_none(),
        "the boolean surface stays gone (#886): {dto}"
    );
}
