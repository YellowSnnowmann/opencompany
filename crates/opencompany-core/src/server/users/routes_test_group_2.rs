use crate::ports::types::CompanyId;
use crate::server::ops::ConnectionsRuntime;
use crate::server::router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::routes_test_support_1::*;

// ---------------------------------------------------------------------------
// Admin routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_an_admin_can_invite() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) = state_with_mail(&home).await;
    let admin = login_via_link(&state, &sender, "ada@example.com").await;

    // Anonymous.
    let app = router(state.clone());
    let response = app
        .oneshot(post(
            "/api/v1/companies/acme/users/invites",
            serde_json::json!({ "email": "bob@example.com" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // The admin can, and the address is normalized on the way in.
    let app = router(state.clone());
    let response = app
        .oneshot(post_with_cookie(
            "/api/v1/companies/acme/users/invites",
            serde_json::json!({ "email": "Bob@Example.com" }),
            &admin,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["email"], "bob@example.com");

    // Bob logs in as a member, and cannot invite.
    let bob = login_via_link(&state, &sender, "bob@example.com").await;
    let app = router(state.clone());
    let response = app
        .oneshot(post_with_cookie(
            "/api/v1/companies/acme/users/invites",
            serde_json::json!({ "email": "eve@example.com" }),
            &bob,
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a member must not be able to invite"
    );
}

#[tokio::test]
async fn an_uninvited_address_cannot_log_in() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, _) = state_with_mail(&home).await;
    // Not invited, not in the manifest: no code is minted at all.
    assert_eq!(
        request_dev_code(&state, "eve@example.com").await,
        None,
        "an uninvited address must not receive a code"
    );
}

#[tokio::test]
async fn suspending_a_user_kills_their_session_at_once() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) = state_with_mail(&home).await;
    let admin = login_via_link(&state, &sender, "ada@example.com").await;

    let app = router(state.clone());
    app.oneshot(post_with_cookie(
        "/api/v1/companies/acme/users/invites",
        serde_json::json!({ "email": "bob@example.com" }),
        &admin,
    ))
    .await
    .unwrap();
    let bob_cookie = login_via_link(&state, &sender, "bob@example.com").await;
    let bob_id = user_id(&state, &admin, "bob@example.com").await;

    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/companies/acme/users/{bob_id}"))
                .header("content-type", "application/json")
                .header("cookie", &admin)
                .body(Body::from(r#"{"status":"suspended"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // His live cookie stops working immediately, not at expiry.
    let app = router(state.clone());
    let response = app
        .oneshot(get_with_cookie(
            "/api/v1/companies/acme/auth/me",
            &bob_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // And he cannot get a new link either.
    assert_eq!(request_dev_code(&state, "bob@example.com").await, None);
}

/// The same bound the self-service route enforces, on the admin route too: an
/// over-long name written for somebody else would render on every surface that
/// shows them and ride in every roster payload.
#[tokio::test]
async fn an_admin_cannot_set_an_over_long_display_name() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) = state_with_mail(&home).await;
    let admin = login_via_link(&state, &sender, "ada@example.com").await;

    let app = router(state.clone());
    app.oneshot(post_with_cookie(
        "/api/v1/companies/acme/users/invites",
        serde_json::json!({ "email": "bob@example.com" }),
        &admin,
    ))
    .await
    .unwrap();
    login_via_link(&state, &sender, "bob@example.com").await;
    let bob_id = user_id(&state, &admin, "bob@example.com").await;

    let long = "A".repeat(crate::server::users::MAX_DISPLAY_NAME_CHARS + 1);
    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/companies/acme/users/{bob_id}"))
                .header("content-type", "application/json")
                .header("cookie", &admin)
                .body(Body::from(
                    serde_json::json!({ "display_name": long }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_last_admin_cannot_be_demoted() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) = state_with_mail(&home).await;
    let admin = login_via_link(&state, &sender, "ada@example.com").await;
    let ada_id = user_id(&state, &admin, "ada@example.com").await;

    // Demoting the only admin would lock the company out of its own directory,
    // and there is no operator token to recover with.
    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/companies/acme/users/{ada_id}"))
                .header("content-type", "application/json")
                .header("cookie", &admin)
                .body(Body::from(r#"{"role":"member"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn an_admin_reset_forces_a_change_and_kills_sessions() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) = state_with_mail(&home).await;
    let admin = login_via_link(&state, &sender, "ada@example.com").await;

    let app = router(state.clone());
    app.oneshot(post_with_cookie(
        "/api/v1/companies/acme/users/invites",
        serde_json::json!({ "email": "bob@example.com" }),
        &admin,
    ))
    .await
    .unwrap();
    let bob_cookie = login_via_link(&state, &sender, "bob@example.com").await;
    let bob_id = user_id(&state, &admin, "bob@example.com").await;

    // The admin issues a temporary password.
    let app = router(state.clone());
    let response = app
        .oneshot(post_with_cookie(
            &format!("/api/v1/companies/acme/users/{bob_id}/password"),
            serde_json::json!({ "password": "temporary pass phrase" }),
            &admin,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let summary = body_json(response).await;
    assert_eq!(summary["mustChangePassword"], true);
    assert_eq!(summary["hasPassword"], true);
    assert!(
        summary.get("passwordHash").is_none(),
        "a response must never carry the hash"
    );

    // Bob's old session is gone: a reset is what you do when you believe the
    // account is compromised.
    let app = router(state.clone());
    let response = app
        .oneshot(get_with_cookie(
            "/api/v1/companies/acme/auth/me",
            &bob_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // He logs in with the temporary password and is told to replace it.
    let app = router(state.clone());
    let response = app
        .oneshot(post(
            "/api/v1/companies/acme/auth/login",
            serde_json::json!({
                "email": "bob@example.com",
                "password": "temporary pass phrase",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let new_cookie = session_cookie(&response);
    assert_eq!(body_json(response).await["mustChangePassword"], true);

    // Setting his own password clears the flag.
    let app = router(state.clone());
    let response = app
        .oneshot(post_with_cookie(
            "/api/v1/companies/acme/auth/password",
            serde_json::json!({ "password": "his own long secret" }),
            &new_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(body_json(response).await["mustChangePassword"], false);
}

#[tokio::test]
async fn a_temporary_password_is_a_boundary_not_a_suggestion() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) = state_with_mail(&home).await;
    let admin = login_via_link(&state, &sender, "ada@example.com").await;

    let app = router(state.clone());
    app.oneshot(post_with_cookie(
        "/api/v1/companies/acme/users/invites",
        serde_json::json!({ "email": "bob@example.com" }),
        &admin,
    ))
    .await
    .unwrap();
    login_via_link(&state, &sender, "bob@example.com").await;
    let bob_id = user_id(&state, &admin, "bob@example.com").await;

    let app = router(state.clone());
    app.oneshot(post_with_cookie(
        &format!("/api/v1/companies/acme/users/{bob_id}/password"),
        serde_json::json!({ "password": "temporary pass phrase" }),
        &admin,
    ))
    .await
    .unwrap();

    // Bob signs in with the temporary password the admin chose — and knows.
    let app = router(state.clone());
    let response = app
        .oneshot(post(
            "/api/v1/companies/acme/auth/login",
            serde_json::json!({
                "email": "bob@example.com",
                "password": "temporary pass phrase",
            }),
        ))
        .await
        .unwrap();
    let temp_cookie = session_cookie(&response);

    // That session is good for exactly one thing: replacing the password. The
    // admin knows this secret and conveyed it over some channel they do not
    // control, so it must not be a working session for anything else.
    let app = router(state.clone());
    let response = app
        .oneshot(post_with_cookie(
            "/api/v1/companies/acme/tasks",
            serde_json::json!({ "title": "work" }),
            &temp_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(response).await["code"],
        "password_change_required"
    );

    // Chat too — this is enforced at the extractors, not per-route.
    let app = router(state.clone());
    let response = app
        .oneshot(post_with_cookie(
            "/api/v1/companies/acme/chat",
            serde_json::json!({ "message": "hi" }),
            &temp_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // But `me` and set-password stay open, or the user could never escape.
    let app = router(state.clone());
    let response = app
        .oneshot(get_with_cookie(
            "/api/v1/companies/acme/auth/me",
            &temp_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // The read stays open, the write does not: a temporary password must not be
    // spendable on the account's public name or face before it is replaced —
    // the admin who reset it knows the value and conveyed it over a channel
    // they do not control.
    let app = router(state.clone());
    let response = app
        .oneshot(patch_with_cookie(
            "/api/v1/companies/acme/auth/me",
            serde_json::json!({ "displayName": "Bob the Temp" }),
            &temp_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(response).await["code"],
        "password_change_required"
    );

    let app = router(state.clone());
    let response = app
        .oneshot(post_with_cookie(
            "/api/v1/companies/acme/auth/password",
            serde_json::json!({ "password": "his own long secret" }),
            &temp_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Once replaced, the same session works normally.
    let app = router(state.clone());
    let response = app
        .oneshot(post_with_cookie(
            "/api/v1/companies/acme/tasks",
            serde_json::json!({ "title": "work" }),
            &temp_cookie,
        ))
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "the flag must clear once the password is replaced, got {}",
        response.status()
    );
}

#[tokio::test]
async fn a_manifest_admin_invite_cannot_be_revoked_through_the_api() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) = state_with_mail(&home).await;
    let admin = login_via_link(&state, &sender, "ada@example.com").await;

    // Revoking a synthetic manifest invite must say so rather than silently
    // succeed — the manifest would re-grant on the next login anyway.
    let app = router(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/companies/acme/users/invites/manifest:ada@example.com")
                .header("cookie", &admin)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_mail_transport_still_returns_202_and_echoes_for_dev() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    // No mail wired at all — the default offline build, on the default
    // loopback bind.
    let state = state_with(&home, ConnectionsRuntime::new()).await;
    assert!(
        request_dev_code(&state, "ada@example.com").await.is_some(),
        "without a transport the code must be echoed so local dev works"
    );
}

#[tokio::test]
async fn a_routable_host_never_echoes_the_code_even_with_no_mail() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    // Routable bind, no mail transport: the code cannot be delivered — and it
    // must NOT come back in the response instead. Returning a credential to
    // whoever asked is worse than nobody being able to sign in.
    let state = state_bound_to(&home, "0.0.0.0:8080", ConnectionsRuntime::new()).await;

    assert_eq!(
        request_dev_code(&state, "ada@example.com").await,
        None,
        "a routable host must never echo a login code"
    );
}

#[tokio::test]
async fn a_loopback_host_with_no_mail_reissues_inside_the_resend_window() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with(&home, ConnectionsRuntime::new()).await;

    // Two sign-ins back to back, well inside the 60s resend window. Nothing is
    // mailed here — the code is echoed — so there is no mailbox to spare, and
    // the only thing a throttle could achieve is locking the sole local sign-in
    // path for a minute after each use. That is issue #271: the console's
    // Playwright bootstrap re-authenticates on every run and would fail on the
    // second one within a minute, reporting a broken host.
    let first = request_dev_code(&state, "ada@example.com")
        .await
        .expect("the first request must echo a code");
    let second = request_dev_code(&state, "ada@example.com")
        .await
        .expect("a second request inside the window must still echo a code");
    assert_ne!(first, second, "the second request must mint a fresh code");

    // And the fresh one is the live one: one live code per address still holds,
    // so the reissue invalidated its predecessor rather than leaving two open.
    let app = router(state.clone());
    let stale = app
        .oneshot(post(
            "/api/v1/companies/acme/auth/verify",
            serde_json::json!({ "code": first }),
        ))
        .await
        .unwrap();
    assert_eq!(
        stale.status(),
        StatusCode::UNAUTHORIZED,
        "reissuing must invalidate the previous code"
    );

    let app = router(state.clone());
    let live = app
        .oneshot(post(
            "/api/v1/companies/acme/auth/verify",
            serde_json::json!({ "code": second }),
        ))
        .await
        .unwrap();
    assert_eq!(live.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_routable_host_still_throttles_even_with_no_mail() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    // No transport wired, but the host looks reachable from elsewhere. The
    // reissue exemption is for the echo path only: this host echoes nothing, so
    // it keeps the throttle and keeps the live code alone.
    let state = state_bound_to(&home, "0.0.0.0:8080", ConnectionsRuntime::new()).await;
    let id = CompanyId::new("acme");
    let runtime = state.registry().get(&id).unwrap();

    assert_eq!(request_dev_code(&state, "ada@example.com").await, None);
    let minted = runtime
        .login_codes()
        .latest_for_email(&id, "ada@example.com")
        .await
        .unwrap()
        .expect("the first request must have minted a code");

    assert_eq!(request_dev_code(&state, "ada@example.com").await, None);
    let after = runtime
        .login_codes()
        .latest_for_email(&id, "ada@example.com")
        .await
        .unwrap()
        .expect("the throttled request must leave the live code in place");
    assert_eq!(
        minted.code_hash, after.code_hash,
        "a throttled request must not replace the live code"
    );
}

#[tokio::test]
async fn with_mail_wired_the_code_is_never_echoed() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) = state_with_mail(&home).await;
    assert_eq!(
        request_dev_code(&state, "ada@example.com").await,
        None,
        "a host that can send mail must never return the code in the response"
    );
    // It went to the mailbox instead.
    let sent = sender.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1.body.contains("/login?company=acme&code="));
}

// ---------------------------------------------------------------------------
// The deployment bootstrap admin (`OPENCOMPANY_ADMIN_EMAIL`, issue #321)
// ---------------------------------------------------------------------------

/// The bug this fixes: a platform-provisioned company's manifest names nobody,
/// so before the variable existed *no address at all* could get in. The
/// injected address is the only one that can, and it comes out an admin — the
/// same grant a manifest entry gives, minted only on redemption.
#[tokio::test]
async fn the_env_admin_can_sign_in_to_a_company_whose_manifest_names_nobody() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) =
        state_with_admin_email(&home, manifest_without_admins(), Some("zoe@example.com")).await;

    let cookie = login_via_link(&state, &sender, "zoe@example.com").await;

    let app = router(state.clone());
    let response = app
        .oneshot(get_with_cookie("/api/v1/companies/acme/auth/me", &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let me = body_json(response).await;
    assert_eq!(me["email"], "zoe@example.com");
    assert_eq!(
        me["role"], "admin",
        "the injected address bootstraps as an admin, exactly like a manifest entry"
    );
}

/// Unset, empty, and whitespace-only are one behaviour: the company as it was
/// before #321. Empty matters on its own — the platform renders the variable
/// for every tenant, so a tenant with no recorded creator gets an empty value
/// rather than no variable, and that must not grant anyone anything.
#[tokio::test]
async fn no_env_admin_leaves_a_provisioned_company_refusing_everyone() {
    for admin_email in [None, Some(""), Some("   ")] {
        let home_dir = home();
        let home = home_dir.path().to_path_buf();
        let (state, sender) =
            state_with_admin_email(&home, manifest_without_admins(), admin_email).await;

        assert_eq!(request_dev_code(&state, "zoe@example.com").await, None);
        assert!(
            sender.sent().is_empty(),
            "{admin_email:?} must grant no eligibility, so no link is ever sent"
        );
    }
}

/// The injected address admits exactly one address, not "anyone the platform
/// vouches for". Everyone else meets the same silence as before.
#[tokio::test]
async fn a_different_address_is_still_refused() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) =
        state_with_admin_email(&home, manifest_without_admins(), Some("zoe@example.com")).await;

    assert_eq!(request_dev_code(&state, "eve@example.com").await, None);
    assert!(
        sender.sent().is_empty(),
        "an address the platform did not name must get no link"
    );
}

/// Case and surrounding whitespace are normalized the way the manifest path
/// normalizes them. A value that only matched with the right capitalization
/// would be a lockout that reads as a typo.
#[tokio::test]
async fn the_env_admin_is_normalized_like_a_manifest_admin() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let (state, sender) = state_with_admin_email(
        &home,
        manifest_without_admins(),
        Some("  ZOE@Example.COM  "),
    )
    .await;

    let cookie = login_via_link(&state, &sender, "zoe@example.com").await;
    let app = router(state.clone());
    let response = app
        .oneshot(get_with_cookie("/api/v1/companies/acme/auth/me", &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["email"], "zoe@example.com");
}

/// An address named in both places is one standing invite, not two. It stays a
/// `manifest:` row: that grant outlives the deployment's variable, so it is the
/// one the operator has to withdraw.
#[tokio::test]
async fn an_env_admin_already_in_the_manifest_is_not_invited_twice() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let manifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [users]\nadmins = [\"Ada@Example.com\", \"Bob@Example.com\"]\n",
    )
    .unwrap();
    let (state, sender) = state_with_admin_email(&home, manifest, Some("Bob@Example.com")).await;

    // Ada signs in so there is an admin to read the invite page with; she
    // becomes a user, which is why only bob's synthetic row is left.
    let admin = login_via_link(&state, &sender, "ada@example.com").await;
    let app = router(state.clone());
    let response = app
        .oneshot(get_with_cookie(
            "/api/v1/companies/acme/users/invites",
            &admin,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let invites = body_json(response).await;
    let rows = invites.as_array().expect("invite list");
    assert_eq!(
        rows.len(),
        1,
        "bob is named twice but is one invite: {invites}"
    );
    assert_eq!(rows[0]["email"], "bob@example.com");
    assert_eq!(rows[0]["id"], "manifest:bob@example.com");
    assert_eq!(rows[0]["role"], "admin", "the role must not change");
    assert_eq!(rows[0]["invitedBy"], "manifest");
}
