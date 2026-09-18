use super::*;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use crate::economy::signer::LocalSigner;
use crate::ports::types::EventSeq;

use super::test_support::*;

#[tokio::test]
async fn promptguard_sanitizes_control_chars_before_event() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let runtime = state.registry().sole().unwrap();
    let app = router().with_state(state);

    // A bell (0x07) and ESC (0x1b) must be stripped; newline survives.
    let rpc = JsonRpcRequest::new(
        "tasks/send",
        json!({ "skill": "seo.free", "note": "hi\u{0007}there\u{001b}\nok" }),
    );
    let body = serde_json::to_vec(&rpc).unwrap();
    let header = siwx_header(&client, "acme", &body, now_secs());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/a2a/acme")
                .header(AUTHORIZATION, header)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let stored = runtime
        .events
        .read_from(runtime.id(), EventSeq::new(0), 10)
        .await
        .unwrap();
    let task = stored
        .iter()
        .find_map(|e| match &e.event {
            CompanyEvent::A2aTaskReceived { task, .. } => Some(task.clone()),
            _ => None,
        })
        .expect("a2a event");
    let note = task["note"].as_str().unwrap();
    assert_eq!(note, "hithere\nok");
}

#[tokio::test]
async fn well_known_and_skill_md_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _client) = seeded_state(dir.path()).await;
    let app = router().with_state(state);

    // The platform well-known returns the card with the a2a endpoint.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/companies/acme/.well-known/agent-card.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let card: AgentCard = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(card.endpoint, "http://127.0.0.1:8080/a2a/acme");
    assert!(card.skills.contains(&"seo.audit".to_string()));

    // skill.md lists each priced skill line.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/a2a/acme/skill.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/markdown; charset=utf-8")
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let md = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(md.contains("`seo.audit` — 25.00 USDC (solana)"));
}

/// PLAT-067 / PLAT-066-067: `tasks/send` is fully synchronous, so a cycle
/// that never returns must not be able to hold the connection (and the
/// task behind it) open forever.
#[tokio::test(start_paused = true)]
async fn a_task_that_never_finishes_is_bounded_by_a_cycle_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state_with_brain(dir.path(), Arc::new(HangingBrain)).await;
    let app = router().with_state(state);

    let body = task_body("seo.free");
    let header = siwx_header(&client, "acme", &body, now_secs());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/a2a/acme")
                .header(AUTHORIZATION, header)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::GATEWAY_TIMEOUT,
        "a company cycle that never returns must not hold the connection open forever"
    );
}

/// PLAT-067: one `tasks/send` POST must append exactly one
/// `A2aTaskReceived` event — not a batch, not a loop that could run the
/// counterparty's task more than once.
#[tokio::test]
async fn exactly_one_cycle_runs_per_inbound_task() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let runtime = state.registry().sole().unwrap();
    let app = router().with_state(state);

    let body = task_body("seo.free");
    let header = siwx_header(&client, "acme", &body, now_secs());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/a2a/acme")
                .header(AUTHORIZATION, header)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let stored = runtime
        .events
        .read_from(runtime.id(), EventSeq::new(0), 10)
        .await
        .unwrap();
    let received = stored
        .iter()
        .filter(|e| matches!(&e.event, CompanyEvent::A2aTaskReceived { .. }))
        .count();
    assert_eq!(
        received, 1,
        "one POST to tasks/send must append exactly one A2aTaskReceived event: {stored:?}"
    );
}

/// PLAT-066: the prosumer fallback is scoped to a genuinely sole company.
/// With two companies registered, an unmatched handle must 404 rather than
/// silently answering as either of them.
#[tokio::test]
async fn the_prosumer_fallback_does_not_fire_when_more_than_one_company_is_registered() {
    let dir = tempfile::tempdir().unwrap();
    let state = two_company_state(dir.path()).await;
    let app = router().with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/a2a/nonexistent-handle/skill.md")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "with two companies registered, an unmatched handle must not resolve to either"
    );
}

/// PLAT-066-067: this IS the SIWX design — a self-issued identity, not an
/// allow-listed one. Two independently generated keypairs, neither ever
/// provisioned or seen before, must each transact on their very first
/// request.
#[tokio::test]
async fn two_independent_strangers_each_transact_without_prior_registration() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _seed_client) = seeded_state(dir.path()).await;
    let app = router().with_state(state);

    for _ in 0..2 {
        let stranger = LocalSigner::generate();
        let body = task_body("seo.free");
        let header = siwx_header(&stranger, "acme", &body, now_secs());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/a2a/acme")
                    .header(AUTHORIZATION, header)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "a freshly generated, never-before-seen keypair must transact on its first request"
        );
    }
}
