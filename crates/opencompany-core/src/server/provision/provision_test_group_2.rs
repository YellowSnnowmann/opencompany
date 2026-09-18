use crate::company::CompanyManifest;
use crate::ports::EventLog;
use crate::runtime::RuntimeBuilder;
use crate::server::router;
use crate::server::webhook::{WebhookConfig, WebhookKind};
use crate::store::FsEventLog;
use crate::{AppConfig, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

use super::provision_test_support_1::*;

/// The happy path end to end: stop, observe it in `status`, release it.
///
/// `ACME_TOML` sets `mode = "full"`, so this also pins that the stop overrides
/// the most permissive policy the manifest can ask for.
#[tokio::test]
async fn emergency_pause_shows_in_status_and_resume_clears_it() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    let paused = app
        .clone()
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-pause",
            Some(PLATFORM_SECRET),
            serde_json::json!({ "confirm": "EMERGENCY-PAUSE", "reason": "runaway loop" }),
        ))
        .await
        .unwrap();
    assert_eq!(paused.status(), StatusCode::OK);
    let body = json_body(paused).await;
    assert_eq!(body["emergency_paused"], true);
    assert_eq!(body["changed"], true);
    // Not orthogonal to lifecycle any more: the stop halts admission, so an
    // ingress that would run a turn is refused while it is engaged.
    assert_eq!(body["lifecycle"], "running");

    let ok = app
        .clone()
        .oneshot(chat_req(
            "/api/v1/companies/acme/chat",
            Some(PLATFORM_SECRET),
            "what are you doing?",
        ))
        .await
        .unwrap();
    // A chat turn calls the model and bills for it, so a stopped company cannot
    // serve one and still be stopped. 409: the operator chose this state and it
    // clears when they release it, so it is neither a fault nor a retry.
    assert_eq!(ok.status(), StatusCode::CONFLICT);

    // Release requires the company id, not the fixed phrase.
    let resumed = app
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-resume",
            Some(PLATFORM_SECRET),
            serde_json::json!({ "confirm": "acme" }),
        ))
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    let body = json_body(resumed).await;
    assert_eq!(body["emergency_paused"], false);
    assert_eq!(body["changed"], true);
}

/// The failure path that matters most: a request with no confirmation, or the
/// wrong one, must not move the switch.
#[tokio::test]
async fn emergency_routes_refuse_without_the_right_confirmation() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    // Empty body → 400, and nothing changed.
    let bare = app
        .clone()
        .oneshot(post_req(
            "/api/v1/companies/acme/emergency-pause",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(bare.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(bare).await["code"], "confirmation_required");

    // A declared JSON body that is empty (or malformed) must reach the handler
    // and read as "no step-up supplied" — the same envelope, not an opaque
    // `Json` rejection. (With `Option<Json<_>>` this request would have been
    // rejected by the extractor before the handler got to answer; the
    // error-aware arm keeps the panic button able to say *what* to send.)
    let empty_json = Request::builder()
        .method("POST")
        .uri("/api/v1/companies/acme/emergency-pause")
        .header("authorization", format!("Bearer {PLATFORM_SECRET}"))
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    let empty_json = app.clone().oneshot(empty_json).await.unwrap();
    assert_eq!(empty_json.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(empty_json).await["code"], "confirmation_required");

    // Wrong phrase → 400.
    let wrong = app
        .clone()
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-pause",
            Some(PLATFORM_SECRET),
            serde_json::json!({ "confirm": "emergency pause please" }),
        ))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);

    // The company is still running normally.
    let status = app
        .clone()
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(json_body(status).await["emergency_paused"], false);

    // Engage it, then try to release with the *pause* phrase rather than the id.
    app.clone()
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-pause",
            Some(PLATFORM_SECRET),
            serde_json::json!({ "confirm": "EMERGENCY-PAUSE" }),
        ))
        .await
        .unwrap();

    let wrong_resume = app
        .clone()
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-resume",
            Some(PLATFORM_SECRET),
            serde_json::json!({ "confirm": "EMERGENCY-PAUSE" }),
        ))
        .await
        .unwrap();
    assert_eq!(wrong_resume.status(), StatusCode::BAD_REQUEST);

    // Still stopped — a failed release must never be a release.
    let status = app
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(json_body(status).await["emergency_paused"], true);
}

/// Pressing the panic button twice is not an error, and the second press
/// reports that it changed nothing.
#[tokio::test]
async fn emergency_pause_is_idempotent() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    let body = serde_json::json!({ "confirm": "EMERGENCY-PAUSE" });
    let first = app
        .clone()
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-pause",
            Some(PLATFORM_SECRET),
            body.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(first).await["changed"], true);

    let second = app
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-pause",
            Some(PLATFORM_SECRET),
            body,
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let body = json_body(second).await;
    assert_eq!(body["changed"], false);
    assert_eq!(body["emergency_paused"], true);
}

/// The mirror idempotency case: releasing a company that is not stopped is not
/// an error, and reports that it changed nothing. The early return exists so a
/// stray release cannot journal a spurious `engaged: false` event against a
/// company that never stopped — the exact failure the engage-side guard guards
/// in reverse.
#[tokio::test]
async fn emergency_resume_when_not_stopped_is_idempotent() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    // The correct confirmation (the company id) on a company that never stopped.
    let release = app
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-resume",
            Some(PLATFORM_SECRET),
            serde_json::json!({ "confirm": "acme" }),
        ))
        .await
        .unwrap();
    assert_eq!(release.status(), StatusCode::OK);
    let body = json_body(release).await;
    assert_eq!(body["changed"], false);
    assert_eq!(body["emergency_paused"], false);
}

/// Unauthenticated callers cannot reach either route — checked before the
/// confirmation, so a correct phrase is never a substitute for a credential.
#[tokio::test]
async fn emergency_routes_require_auth() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    let anon = app
        .clone()
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-pause",
            None,
            serde_json::json!({ "confirm": "EMERGENCY-PAUSE" }),
        ))
        .await
        .unwrap();
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);

    let anon_resume = app
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-resume",
            None,
            serde_json::json!({ "confirm": "acme" }),
        ))
        .await
        .unwrap();
    assert_eq!(anon_resume.status(), StatusCode::UNAUTHORIZED);
}

/// The kill switch is journaled with the acting operator, both directions.
#[tokio::test]
async fn emergency_transitions_are_journaled_with_the_actor() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state.clone());

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    app.clone()
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-pause",
            Some(PLATFORM_SECRET),
            serde_json::json!({ "confirm": "EMERGENCY-PAUSE", "reason": "burning budget" }),
        ))
        .await
        .unwrap();
    app.oneshot(json_post_req(
        "/api/v1/companies/acme/emergency-resume",
        Some(PLATFORM_SECRET),
        serde_json::json!({ "confirm": "acme" }),
    ))
    .await
    .unwrap();

    let runtime = state
        .registry()
        .get(&CompanyId::new("acme"))
        .expect("company registered");
    let events = runtime
        .events()
        .read_from(runtime.id(), EventSeq::new(0), 1000)
        .await
        .unwrap();
    let changes: Vec<_> = events
        .iter()
        .filter_map(|stored| match &stored.event {
            CompanyEvent::EmergencyPauseChanged {
                engaged,
                by,
                reason,
            } => Some((*engaged, by.clone(), reason.clone())),
            _ => None,
        })
        .collect();

    assert_eq!(changes.len(), 2, "expected an engage and a release");
    assert!(changes[0].0, "first event should be the engage");
    assert_eq!(changes[0].2.as_deref(), Some("burning budget"));
    assert!(!changes[1].0, "second event should be the release");
    // Both carry an identified actor rather than an anonymous one.
    assert!(!changes[0].1.id.is_empty());
    assert!(!changes[1].1.id.is_empty());
}

#[tokio::test]
async fn emergency_stop_survives_a_cold_boot_and_release_does_not_stick() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    // Engage the stop over the route.
    let paused = app
        .clone()
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-pause",
            Some(PLATFORM_SECRET),
            serde_json::json!({ "confirm": "EMERGENCY-PAUSE", "reason": "pre-restart" }),
        ))
        .await
        .unwrap();
    assert_eq!(paused.status(), StatusCode::OK);

    // A fresh boot on the same home — a second CompanyRuntime with no handover,
    // so the flag must come from the journal, not from live memory — comes up
    // stopped.
    let manifest: CompanyManifest = toml::from_str(ACME_TOML).unwrap();
    let rebooted = RuntimeBuilder::new(home.clone(), manifest.clone())
        .with_id(CompanyId::new("acme"))
        .build()
        .await
        .unwrap();
    assert!(
        rebooted.is_emergency_paused(),
        "a company stopped before a restart must boot stopped"
    );

    // Release the stop on the live runtime, then boot cold once more: the
    // switch must not be sticky.
    let resumed = app
        .oneshot(json_post_req(
            "/api/v1/companies/acme/emergency-resume",
            Some(PLATFORM_SECRET),
            serde_json::json!({ "confirm": "acme" }),
        ))
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);

    let released = RuntimeBuilder::new(home, manifest)
        .with_id(CompanyId::new("acme"))
        .build()
        .await
        .unwrap();
    assert!(
        !released.is_emergency_paused(),
        "a company released before a restart must boot running"
    );
}

#[tokio::test]
async fn suspend_requires_platform_scope_and_blocks_chat() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    // A tenant-only token cannot suspend.
    let tenant = tenant_token("tenant:platform", &["operator"]);
    let forbidden = app
        .clone()
        .oneshot(post_req("/api/v1/companies/acme/suspend", Some(&tenant)))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    // Platform scope suspends.
    let suspended = app
        .clone()
        .oneshot(post_req(
            "/api/v1/companies/acme/suspend",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(suspended.status(), StatusCode::OK);
    assert_eq!(json_body(suspended).await["lifecycle"], "suspended");

    // Chat is blocked.
    let conflict = app
        .oneshot(chat_req(
            "/api/v1/companies/acme/chat",
            Some(PLATFORM_SECRET),
            "hi",
        ))
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn foreign_tenant_cannot_file_feedback() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    // acme is owned by tenant:platform.
    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    // A different tenant's token must not reach acme's feedback route.
    let other = tenant_token("tenant:other", &["operator"]);
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/companies/acme/feedback")
        .header("cookie", crate::server::test_support::fixed_cookie("acme"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {other}"))
        .body(Body::from(r#"{"category":"bug","note":"not yours"}"#))
        .unwrap();
    let denied = app.oneshot(req).await.unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn owner_cannot_resume_a_platform_suspension() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    // Platform suspends the tenant.
    let suspended = app
        .clone()
        .oneshot(post_req(
            "/api/v1/companies/acme/suspend",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(suspended.status(), StatusCode::OK);

    // The owner's tenant token must NOT be able to lift the suspension.
    let tenant = tenant_token("tenant:platform", &["operator"]);
    let denied = app
        .clone()
        .oneshot(post_req("/api/v1/companies/acme/resume", Some(&tenant)))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    // Platform scope can lift it.
    let resumed = app
        .oneshot(post_req(
            "/api/v1/companies/acme/resume",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    assert_eq!(json_body(resumed).await["lifecycle"], "running");
}

#[tokio::test]
async fn archive_removes_from_registry() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();

    let archived = app
        .clone()
        .oneshot(post_req(
            "/api/v1/companies/acme/archive",
            Some(PLATFORM_SECRET),
        ))
        .await
        .unwrap();
    assert_eq!(archived.status(), StatusCode::OK);
    assert_eq!(json_body(archived).await["lifecycle"], "archived");

    // Now unaddressable: status 404, chat 404.
    let status = app
        .clone()
        .oneshot(get_req("/api/v1/companies/acme", Some(PLATFORM_SECRET)))
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::NOT_FOUND);

    let chat = app
        .oneshot(chat_req(
            "/api/v1/companies/acme/chat",
            Some(PLATFORM_SECRET),
            "hi",
        ))
        .await
        .unwrap();
    assert_eq!(chat.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cross_tenant_access_forbidden() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    // Tenant B provisions (its token carries the platform scope).
    let b_platform = tenant_token("tenant:b", &["platform", "operator"]);
    let created = app
        .clone()
        .oneshot(provision_req(Some(&b_platform), ACME_TOML))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);

    // Tenant A (no platform scope, different tenant) cannot address it.
    let a_token = tenant_token("tenant:a", &["operator"]);
    let forbidden = app
        .oneshot(get_req("/api/v1/companies/acme", Some(&a_token)))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lifecycle_transition_recorded_as_event() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = platform_state(&home, None);
    let app = router(state);

    app.clone()
        .oneshot(provision_req(Some(PLATFORM_SECRET), ACME_TOML))
        .await
        .unwrap();
    app.oneshot(post_req(
        "/api/v1/companies/acme/pause",
        Some(PLATFORM_SECRET),
    ))
    .await
    .unwrap();

    // The audit trail carries a LifecycleChanged running -> paused.
    let events = FsEventLog::new(home.clone());
    let stored = events
        .read_from(&CompanyId::new("acme"), EventSeq::new(0), usize::MAX)
        .await
        .unwrap();
    let found = stored.iter().any(|e| {
        matches!(
            &e.event,
            CompanyEvent::LifecycleChanged { from, to, .. } if from == "running" && to == "paused"
        )
    });
    assert!(found, "expected a LifecycleChanged event, got {stored:?}");
}

#[tokio::test]
async fn webhook_emitted_on_approval_requested() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    // Prosumer mode (no platform_auth) plus a recording webhook sink.
    let (webhook, sink) = WebhookConfig::recording("tenant-secret");
    let state = AppState::new(AppConfig::default())
        .with_home(home.clone())
        .with_webhook(webhook);

    // A company whose agent explicitly asks the operator for approval.
    let manifest: CompanyManifest =
        toml::from_str("[company]\nname = \"Acme\"\n[policy]\nmode = \"supervised\"\n").unwrap();
    let sign_effect = Effect {
        kind: crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND.into(),
        group: EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::json!({
            "title": "Submit the filing",
            "question": "May I submit it?"
        }),
        agent: Some("ceo".into()),
        run_id: None,
    };
    let runtime = RuntimeBuilder::new(home.clone(), manifest)
        .with_id(CompanyId::new("acme"))
        .with_brain(Arc::new(EffectBrain {
            effect: sign_effect,
        }))
        .build()
        .await
        .unwrap();
    state
        .registry()
        .insert(CompanyId::new("acme"), Arc::new(runtime));
    crate::server::test_support::seed_fixed_admin(&state, "acme").await;

    let app = router(state);
    let chat = app
        .oneshot(chat_req("/api/v1/companies/acme/chat", None, "file it"))
        .await
        .unwrap();
    assert_eq!(chat.status(), StatusCode::OK);

    let delivered = sink.delivered();
    let approval = delivered
        .iter()
        .find(|(event, _)| event.kind == WebhookKind::ApprovalRequested)
        .expect("an approval_requested webhook was delivered");
    // The delivery carries a non-empty signature header value.
    assert!(!approval.1.is_empty());
    assert!(approval.1.starts_with("kh1="));
}
