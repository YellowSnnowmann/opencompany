use crate::app::config::AuthMode;
use crate::server::router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::provision_test_support_1::*;

// ---------------------------------------------------------------------------
// Provisioning + status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provision_then_list_then_status() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    // Provision with a platform-scope token.
    let response = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    assert_eq!(body["id"], "acme");
    assert_eq!(body["lifecycle"], "running");

    // List shows it.
    let list = app
        .clone()
        .oneshot(get_req("/api/v1/companies", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_body = json_body(list).await;
    assert_eq!(list_body.as_array().unwrap().len(), 1);

    // Status by id.
    let status = app
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(json_body(status).await["id"], "acme");
}

/// The same refusal boot applies to a `none`-mode company on a routable bind:
/// a company with no sign-in reachable from anywhere is an unauthenticated
/// admin console. A tenant's manifest can request `[users].mode = "none"`, but
/// this host must not silently serve it, regardless of which path created the
/// runtime.
#[tokio::test]
async fn provisioning_a_none_mode_company_on_a_routable_bind_is_refused() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = routable_platform_state(&home);
    let app = router(state);

    let toml = "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n[users]\nmode = \"none\"\n";
    let response = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), toml))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["code"], "auth_mode_none_not_allowed");

    // Refused, so nothing was registered.
    let response = app
        .clone()
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The error above tells the caller to fix the manifest (`email` or `wallet`)
/// and retry. Before this durable-store duplicate check existed,
/// `RuntimeBuilder::build` had already saved a `CompanyRecord` for `id` when
/// the refusal fired, so that recovery hit `company_exists` forever — the id
/// was reserved by a provision that never succeeded (issue #1828 comment
/// 3866012835). The auth-mode check now runs before `id` is even resolved, so
/// a rejected `none`-mode request must never reach the store at all, and the
/// exact same id must provision cleanly right after.
#[tokio::test]
async fn retrying_after_a_none_mode_refusal_with_a_valid_mode_succeeds() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = routable_platform_state(&home);
    let app = router(state);

    let rejected =
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n[users]\nmode = \"none\"\n";
    let response = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), rejected))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["code"], "auth_mode_none_not_allowed");

    // Same company name, so the same id — corrected to a mode with sign-in.
    let corrected =
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n[users]\nmode = \"email\"\n";
    let response = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), corrected))
        .await
        .unwrap();
    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "retry with a valid auth mode must provision the id the rejected \
         request never should have reserved, got: {body:?}"
    );
    assert_eq!(body["id"], "acme");
}

/// A manifest built the way the console's create/reset dialog always builds
/// one — `[users].admins` only, never `[users].mode` or `[users].wallets`
/// (`buildManifestToml`, `frontend/src/lib/company-manifest.ts`) — must be
/// refused on a host whose auth override forces `wallet`: the manifest's own
/// admin bootstrap is read in `email` mode only, and unlike `email` there is
/// no deployment-wide `OPENCOMPANY_ADMIN_EMAIL`-style fallback for `wallet`
/// (`manifest_wallets`, `server/users/wallet.rs` — "there is deliberately no
/// environment counterpart"). Provisioning this manifest as-is would create a
/// company nobody, ever, can sign in to.
///
/// `manifest.validate()` alone cannot catch this: the manifest's own
/// `[users].mode` defaults to `email`, which is perfectly self-consistent
/// with a non-empty `admins` list. The mismatch only exists against the
/// host's override, which is why this is checked against
/// `effective_auth_mode` in the handler rather than in `CompanyManifest`
/// itself (issue #1828 comment 3866132491).
#[tokio::test]
async fn provisioning_admins_only_manifest_on_a_wallet_mode_host_is_refused() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    state.set_auth_mode_override(Some(AuthMode::Wallet));
    let app = router(state.clone());

    // Exactly the shape `buildManifestToml` emits: a name and an admin email,
    // no `[users].mode`, no `[users].wallets`.
    let toml = "[company]\nname = \"Acme\"\n[users]\nadmins = [\"ada@example.com\"]\n";
    let response = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), toml))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["code"], "auth_mode_wallet_no_wallets");

    // Refused, so nothing was registered and the id was not reserved.
    let response = app
        .clone()
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The refusal above must not permanently burn the id the way a post-build
/// refusal would (see `retrying_after_a_none_mode_refusal_with_a_valid_mode_
/// succeeds` for the same property on the `none`-mode check): it runs before
/// `id` is resolved, so a caller who adds a wallet address and retries with
/// the exact same company name must provision cleanly.
#[tokio::test]
async fn retrying_after_a_wallet_mode_refusal_with_a_wallet_listed_succeeds() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    state.set_auth_mode_override(Some(AuthMode::Wallet));
    let app = router(state.clone());

    let rejected = "[company]\nname = \"Acme\"\n[users]\nadmins = [\"ada@example.com\"]\n";
    let response = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), rejected))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert_eq!(body["code"], "auth_mode_wallet_no_wallets");

    // Same company name, so the same id — corrected to carry a wallet AND
    // declare `mode = "wallet"`: `manifest.validate()`'s own self-consistency
    // check (`validate_users`) reads `[users].wallets` only when the manifest
    // itself says `mode = "wallet"` — its default is `email` — and refuses a
    // wallets-with-no-mode manifest on that unrelated ground before this
    // request would ever reach the effective-mode check under test. The wallet
    // address itself is built rather than pasted, like `CompanyManifest`'s own
    // `wallet_address()` test helper, so it cannot drift from what the decoder
    // accepts (a base58 32-byte Ed25519 public key).
    let wallet_address = bs58::encode([9u8; 32]).into_string();
    let corrected = format!(
        "[company]\nname = \"Acme\"\n[users]\nmode = \"wallet\"\nwallets = [\"{wallet_address}\"]\n"
    );
    let response = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), &corrected))
        .await
        .unwrap();
    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "retry with a wallet listed must provision the id the rejected \
         request never should have reserved, got: {body:?}"
    );
    assert_eq!(body["id"], "acme");
}

/// With no host-wide auth override, the preflight reports `email` — each
/// company's own `[users].mode` decides, and the console builds an `email`
/// manifest by default.
#[tokio::test]
async fn provisioning_info_reports_email_by_default() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    let response = app
        .oneshot(get_req(
            "/api/v1/companies/provisioning",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["auth_mode"], "email");
    assert_eq!(body["wallets_required"], false);
}

/// A host whose override forces `wallet` reports it, so the create/reset dialog
/// collects wallet addresses before it builds a manifest the backend would
/// otherwise refuse with `auth_mode_wallet_no_wallets`.
#[tokio::test]
async fn provisioning_info_reports_wallet_when_overridden() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    state.set_auth_mode_override(Some(AuthMode::Wallet));
    let app = router(state);

    let response = app
        .oneshot(get_req(
            "/api/v1/companies/provisioning",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["auth_mode"], "wallet");
    assert_eq!(body["wallets_required"], true);
}

/// The preflight is a `PlatformScope` route: a session cookie can never reach
/// it (401), and a tenant token without the platform scope is refused (403).
#[tokio::test]
async fn provisioning_info_requires_platform_scope() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    let unauthorized = app
        .clone()
        .oneshot(get_req("/api/v1/companies/provisioning", None))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let token = tenant_token("tenant:acme", &["operator"]);
    let forbidden = app
        .oneshot(get_req("/api/v1/companies/provisioning", Some(&token)))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
}

/// A wallet-mode host provisions a manifest that lists `[users].wallets` (and
/// declares `mode = "wallet"`, which `manifest.validate()` requires before it
/// reads the wallets) — the positive counterpart to the empty-wallets refusal.
#[tokio::test]
async fn provisioning_a_wallet_manifest_on_a_wallet_mode_host_succeeds() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    state.set_auth_mode_override(Some(AuthMode::Wallet));
    let app = router(state);

    let wallet_address = bs58::encode([7u8; 32]).into_string();
    let toml = format!(
        "[company]\nname = \"Acme\"\n[users]\nmode = \"wallet\"\nwallets = [\"{wallet_address}\"]\n"
    );
    let response = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), &toml))
        .await
        .unwrap();
    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::CREATED, "got: {body:?}");
    assert_eq!(body["id"], "acme");
    assert_eq!(body["lifecycle"], "running");
}

/// An archived company's durable record must block ANY later provision that
/// asks for its id — not just a reset of that same company reusing its own
/// id, but a wholly unrelated company typing the archived id into Advanced.
///
/// Archive removes a company from the live registry, which is all the old
/// duplicate-id check consulted, but never deletes its durable record — and
/// `RuntimeBuilder::build` loads any existing durable record for an id before
/// building over it. So a registry-only check let a second, unrelated
/// "clean" company come back carrying the archived company's old lifecycle,
/// ledger and overlays (issue #1828 comment 3865803905). This proves the
/// server refuses regardless of which company is asking, not just the one
/// that owned the id originally.
#[tokio::test]
async fn archived_company_id_rejected_for_unrelated_provision() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    // Provision and then archive "acme".
    let created = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(json_body(created).await["id"], "acme");

    let archived = app
        .clone()
        .oneshot(post_req(
            "/api/v1/companies/acme/archive",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);

    // "acme" is gone from the live registry ...
    let missing = app
        .clone()
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // ... but a totally unrelated new company ("Beta") asking for that same
    // id must still be refused, not silently built over the archived record.
    const BETA_TOML: &str = "[company]\nname = \"Beta\"\n[policy]\nmode = \"full\"\n";
    let collision = app
        .clone()
        .oneshot(provision_req_json(Some(PLATFORM_SECRET), BETA_TOML, "acme"))
        .await
        .unwrap();
    assert_eq!(collision.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(collision).await["code"], "company_exists");

    // And the archived record's own history was not disturbed: a fresh
    // provision of "acme" cleanly denied above means nothing overwrote it, so
    // the id is still not addressable as a live company.
    let still_missing = app
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(still_missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn provision_accepts_json_envelope_with_explicit_id() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    let body = serde_json::json!({ "manifest_toml": ACME_TOML, "id": "custom-id" }).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/companies")
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {PLATFORM_SECRET}"))
        .body(Body::from(body))
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(json_body(response).await["id"], "custom-id");
}

#[tokio::test]
async fn provision_requires_platform_scope() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    // No token → 401.
    let unauthorized = app
        .clone()
        .oneshot(provision_req(None, ACME_TOML))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    // Tenant-only token (no platform scope) → 403.
    let token = tenant_token("tenant:acme", &["operator"]);
    let forbidden = app
        .oneshot(provision_req(Some(&token), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(forbidden).await["code"], "forbidden");
}

#[tokio::test]
async fn invalid_manifest_is_400() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    // Empty company name fails validation.
    let bad = "[company]\nname = \"\"\n";
    let response = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), bad))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(response).await["code"], "manifest_invalid");
}

#[tokio::test]
async fn quota_rejects_when_exceeded() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, Some(1));
    let app = router(state);

    let first = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);

    let globex = "[company]\nname = \"Globex\"\n";
    let second = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), globex))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(json_body(second).await["code"], "quota_exceeded");
}

#[tokio::test]
async fn duplicate_id_conflicts() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    let first = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);

    let dup = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(dup).await["code"], "company_exists");
}

#[tokio::test]
async fn provision_namespaces_id_by_workload_tenant() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    // Workload tenant is `tenant-a`.
    let state = namespaced_state(&home, "tenant-a");
    // Keep a handle on the shared ownership map to inspect what boot hydration
    // (which filters `owners` rows by the configured namespace) would reload.
    let observed = state.clone();
    let app = router(state);

    // A *full-platform* token provisions the Acme template. Its acting tenant is
    // `tenant:platform`, not `tenant-a` — yet the id and owner must be keyed to
    // the workload tenant, or the company is orphaned at reboot.
    let response = app
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    // The derived id `acme` is namespaced with the workload tenant, not the
    // acting `tenant:platform`.
    assert_eq!(json_body(response).await["id"], "tenant-a--acme");
    // The ownership row records the workload tenant — exactly what boot
    // hydration filters on — so the company survives a restart.
    let id = CompanyId::new("tenant-a--acme");
    assert_eq!(observed.owner_of(&id).as_deref(), Some("tenant-a"));
}

#[tokio::test]
async fn same_template_under_two_tenant_workloads_does_not_conflict() {
    // Two tenants are two separate workloads (containers), each with its own
    // `OPENCOMPANY_TENANT_ID`, writing to one shared logical database. In a
    // shared DB the derived id `acme` used to collide; per-workload namespacing
    // keeps them distinct.
    let home_a_dir = home();
    let home_a = home_a_dir.path().to_path_buf();
    let app_a = router(namespaced_state(&home_a, "tenant-a"));
    let a = tenant_token("tenant-a", &["platform", "operator"]);
    let first = app_a
        .oneshot(provision_req(Some(&a), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);
    assert_eq!(json_body(first).await["id"], "tenant-a--acme");

    let home_b_dir = home();
    let home_b = home_b_dir.path().to_path_buf();
    let app_b = router(namespaced_state(&home_b, "tenant-b"));
    let b = tenant_token("tenant-b", &["platform", "operator"]);
    let second = app_b
        .oneshot(provision_req(Some(&b), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CREATED);
    assert_eq!(json_body(second).await["id"], "tenant-b--acme");
}

#[tokio::test]
async fn claim_shaped_tenant_manages_namespaced_company() {
    // Shared-single-DB workload for tenant slug `acme` (its bare
    // `OPENCOMPANY_TENANT_ID`). A full-platform token provisions the company; it
    // is namespaced `acme--acme` and its owner is recorded under the bare slug.
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = namespaced_state(&home, "acme");
    let observed = state.clone();
    let app = router(state);

    let created = app
        .clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(json_body(created).await["id"], "acme--acme");
    // Ownership is recorded canonically (bare slug), matching the namespace.
    let id = CompanyId::new("acme--acme");
    assert_eq!(observed.owner_of(&id).as_deref(), Some("acme"));

    // The tenant's own token carries the platform-issued *claim* shape
    // `tenant:acme`, which differs textually from the bare `acme` owner. It must
    // still be authorized to address and manage its own company.
    let claim_shaped = tenant_token("tenant:acme", &["operator"]);
    let status = app
        .clone()
        .oneshot(get_req("/api/v1/companies/acme--acme", Some(&claim_shaped)))
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(json_body(status).await["id"], "acme--acme");

    let paused = app
        .clone()
        .oneshot(post_req(
            "/api/v1/companies/acme--acme/pause",
            Some(&claim_shaped),
        ))
        .await
        .unwrap();
    assert_eq!(paused.status(), StatusCode::OK);
    assert_eq!(json_body(paused).await["lifecycle"], "paused");

    // A different tenant — whatever its representation — is still denied.
    let intruder = tenant_token("tenant:globex", &["operator"]);
    let denied = app
        .oneshot(get_req("/api/v1/companies/acme--acme", Some(&intruder)))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_toggles_and_chat_409() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    // Pause → paused.
    let paused = app
        .clone()
        .oneshot(post_req(
            "/api/v1/companies/acme/pause",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(paused.status(), StatusCode::OK);
    assert_eq!(json_body(paused).await["lifecycle"], "paused");

    // Chat is 409 while paused.
    let conflict = app
        .clone()
        .oneshot(chat_req(
            "/api/v1/companies/acme/chat",
            Some(PLATFORM_SECRET),
            "hi",
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    // Resume → running, chat 200.
    let resumed = app
        .clone()
        .oneshot(post_req(
            "/api/v1/companies/acme/resume",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    assert_eq!(json_body(resumed).await["lifecycle"], "running");

    let ok = app
        .oneshot(chat_req(
            "/api/v1/companies/acme/chat",
            Some(PLATFORM_SECRET),
            "hi",
        ))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
}
