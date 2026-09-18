use super::*;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use crate::economy::x402::X402Challenge;
use crate::ports::types::{CompanyId, EventSeq};

use super::test_support::*;

#[tokio::test]
async fn siwx_invalid_inbound_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _client) = seeded_state(dir.path()).await;
    let app = router().with_state(state);

    let body = task_body("seo.free");
    // No Authorization header at all.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/a2a/acme")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn priced_skill_without_payment_returns_402() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    // Our address is the on-disk signer for the company id.
    let our_id = signer_for(dir.path(), &CompanyId::new("acme"))
        .await
        .unwrap()
        .agent_id();
    let app = router().with_state(state);

    let body = task_body("seo.audit");
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

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let challenge: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(challenge["amount"], "25.00");
    assert_eq!(challenge["recipient"], our_id);
    assert_eq!(challenge["asset"], "USDC");
    assert_eq!(challenge["network"], "solana");
}

#[tokio::test]
async fn valid_signed_free_task_routes_to_cycle() {
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
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let envelope: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(envelope["jsonrpc"], "2.0");
    assert!(envelope["result"]["cycleId"].is_string());

    // The A2aTaskReceived event was persisted by the cycle.
    let stored = runtime
        .events
        .read_from(runtime.id(), EventSeq::new(0), 10)
        .await
        .unwrap();
    assert!(stored.iter().any(|e| matches!(
        &e.event,
        CompanyEvent::A2aTaskReceived { from, .. } if from == &client.agent_id()
    )));
}

#[tokio::test]
async fn paid_skill_with_valid_x402_routes_and_journals() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let runtime = state.registry().sole().unwrap();
    let our_id = signer_for(dir.path(), &CompanyId::new("acme"))
        .await
        .unwrap()
        .agent_id();
    let app = router().with_state(state);

    // Build a valid x402 authorization paying the 25.00 seo.audit price.
    let challenge = X402Challenge {
        amount: "25.00".into(),
        recipient: our_id,
        asset: "USDC".into(),
        network: "solana".into(),
    };
    let auth = x402::authorize(&client, &challenge, now_secs());
    let rpc = JsonRpcRequest::new(
        "tasks/send",
        json!({ "skill": "seo.audit", "input": {}, "payment": auth }),
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
    // The inbound receipt was journaled as x402.in.
    let record = runtime.store.load(runtime.id()).await.unwrap().unwrap();
    let inflow = record
        .ledger
        .iter()
        .find(|e| e.kind == "x402.in")
        .expect("x402.in row");
    assert_eq!(inflow.amount_usd, 25.0);
}

/// The same signed authorization, presented on two different tasks. Each
/// request carries its own SIWX signature, so the transport replay cache
/// admits both; only the payment layer can refuse the second.
#[tokio::test]
async fn replayed_x402_authorization_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let our_id = signer_for(dir.path(), &CompanyId::new("acme"))
        .await
        .unwrap()
        .agent_id();
    let app = router().with_state(state);

    let challenge = X402Challenge {
        amount: "25.00".into(),
        recipient: our_id,
        asset: "USDC".into(),
        network: "solana".into(),
    };
    let auth = x402::authorize(&client, &challenge, now_secs());

    let first = paid_request(&client, &auth, "first.example");
    let response = app.clone().oneshot(first).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "first purchase is served"
    );

    let second = paid_request(&client, &auth, "second.example");
    let response = app.oneshot(second).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "the same authorization must not buy a second task"
    );
}

/// Spending one nonce must not blind the company to the next payment.
#[tokio::test]
async fn a_freshly_minted_authorization_is_admitted() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let our_id = signer_for(dir.path(), &CompanyId::new("acme"))
        .await
        .unwrap()
        .agent_id();
    let app = router().with_state(state);

    let challenge = X402Challenge {
        amount: "25.00".into(),
        recipient: our_id,
        asset: "USDC".into(),
        network: "solana".into(),
    };

    for site in ["first.example", "second.example"] {
        let auth = x402::authorize(&client, &challenge, now_secs());
        let request = paid_request(&client, &auth, site);
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{site} pays its own way");
    }
}

/// A skill id the card never advertises must not slip past the pricing gate
/// on a company that prices its work.
#[tokio::test]
async fn unknown_skill_id_is_refused_on_a_pricing_card() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let app = router().with_state(state);

    let body = task_body("seo.ghost");
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
        StatusCode::NOT_FOUND,
        "an unpriced, unadvertised skill must not run for free"
    );
}

#[tokio::test]
async fn paid_skill_with_wrong_recipient_is_rechallenged() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let app = router().with_state(state.clone());

    // A well-formed, correctly-signed authorization that pays SOMEONE ELSE
    // (a self-dealing payer) must not buy priced work from this company.
    let challenge = X402Challenge {
        amount: "25.00".into(),
        recipient: client.agent_id(), // not our company's agent id
        asset: "USDC".into(),
        network: "solana".into(),
    };
    let auth = x402::authorize(&client, &challenge, now_secs());
    let rpc = JsonRpcRequest::new(
        "tasks/send",
        json!({ "skill": "seo.audit", "input": {}, "payment": auth }),
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

    // Re-challenged with a 402, not served for free.
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    // And the rejection must not have spent the nonce: on a multi-company
    // host, submitting a valid authorization against the wrong company's
    // handle would otherwise burn it here and reject the payer's retry
    // against the right company as a replay, even though it was never
    // accepted anywhere.
    assert!(
        state
            .x402_nonce()
            .check_and_insert(&auth.nonce, now_secs(), auth.timestamp)
            .expect("nonce cache must still answer")
    );
}

#[tokio::test]
async fn an_underpaid_authorization_does_not_spend_its_nonce() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let app = router().with_state(state.clone());

    let our_id = signer_for(state.home(), &CompanyId::new("acme"))
        .await
        .unwrap()
        .agent_id();
    let challenge = X402Challenge {
        amount: "1.00".into(), // below seo.audit's price
        recipient: our_id,
        asset: "USDC".into(),
        network: "solana".into(),
    };
    let auth = x402::authorize(&client, &challenge, now_secs());
    let rpc = JsonRpcRequest::new(
        "tasks/send",
        json!({ "skill": "seo.audit", "input": {}, "payment": auth }),
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

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    assert!(
        state
            .x402_nonce()
            .check_and_insert(&auth.nonce, now_secs(), auth.timestamp)
            .expect("nonce cache must still answer"),
        "an underpaid authorization must not burn its nonce — the payer \
         cannot fix the amount without re-signing, but nothing here \
         should have consumed it either"
    );
}

#[tokio::test]
async fn a_correctly_priced_authorization_in_the_wrong_asset_is_rechallenged() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let app = router().with_state(state.clone());

    let our_id = signer_for(state.home(), &CompanyId::new("acme"))
        .await
        .unwrap()
        .agent_id();
    // The payer signs a fully-priced authorization, but in an asset the
    // card never priced this skill in.
    let challenge = X402Challenge {
        amount: "25.00".into(),
        recipient: our_id,
        asset: "NOTUSDC".into(),
        network: "solana".into(),
    };
    let auth = x402::authorize(&client, &challenge, now_secs());
    let rpc = JsonRpcRequest::new(
        "tasks/send",
        json!({ "skill": "seo.audit", "input": {}, "payment": auth }),
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

    assert_eq!(
        response.status(),
        StatusCode::PAYMENT_REQUIRED,
        "a signed payment in the wrong asset must not buy work priced in a different one"
    );
    assert!(
        state
            .x402_nonce()
            .check_and_insert(&auth.nonce, now_secs(), auth.timestamp)
            .expect("nonce cache must still answer"),
        "the rejected authorization must not have spent its nonce"
    );
}

#[tokio::test]
async fn a_non_finite_amount_is_rechallenged_not_treated_as_paid() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let app = router().with_state(state.clone());

    let our_id = signer_for(state.home(), &CompanyId::new("acme"))
        .await
        .unwrap()
        .agent_id();
    // `"NaN".parse::<f64>()` succeeds and every comparison against NaN is
    // false, so a naive `paid < price` underpayment check treats this as
    // sufficient. It must not be.
    let challenge = X402Challenge {
        amount: "NaN".into(),
        recipient: our_id,
        asset: "USDC".into(),
        network: "solana".into(),
    };
    let auth = x402::authorize(&client, &challenge, now_secs());
    let rpc = JsonRpcRequest::new(
        "tasks/send",
        json!({ "skill": "seo.audit", "input": {}, "payment": auth }),
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

    assert_eq!(
        response.status(),
        StatusCode::PAYMENT_REQUIRED,
        "a non-finite claimed amount must never be treated as sufficient payment"
    );
    assert!(
        state
            .x402_nonce()
            .check_and_insert(&auth.nonce, now_secs(), auth.timestamp)
            .expect("nonce cache must still answer"),
        "the rejected authorization must not have spent its nonce"
    );
}

#[tokio::test]
async fn replayed_signature_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let app = router().with_state(state);

    let body = task_body("seo.free");
    let header = siwx_header(&client, "acme", &body, now_secs());

    let build = || {
        Request::builder()
            .method("POST")
            .uri("/a2a/acme")
            .header(AUTHORIZATION, header.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.clone()))
            .unwrap()
    };

    let first = app.clone().oneshot(build()).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    // The identical signature is rejected on replay.
    let second = app.oneshot(build()).await.unwrap();
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
}

/// The spent-nonce set is the only thing that makes an authorization
/// single-use, so a set that cannot answer must stop the sale.
#[tokio::test]
async fn an_unusable_spent_nonce_set_refuses_a_paid_task() {
    let dir = tempfile::tempdir().unwrap();
    let (state, client) = seeded_state(dir.path()).await;
    let runtime = state.registry().sole().unwrap();
    let our_id = signer_for(dir.path(), &CompanyId::new("acme"))
        .await
        .unwrap()
        .agent_id();
    state.x402_nonce().poison_for_tests();
    let app = router().with_state(state);

    let challenge = X402Challenge {
        amount: "25.00".into(),
        recipient: our_id,
        asset: "USDC".into(),
        network: "solana".into(),
    };
    let auth = x402::authorize(&client, &challenge, now_secs());
    let response = app
        .oneshot(paid_request(&client, &auth, "x.com"))
        .await
        .unwrap();

    assert_ne!(
        response.status(),
        StatusCode::OK,
        "an unreadable spent-nonce set must refuse the payment"
    );
    let stored = runtime
        .events
        .read_from(runtime.id(), EventSeq::new(0), 10)
        .await
        .unwrap();
    assert!(
        !stored
            .iter()
            .any(|e| matches!(&e.event, CompanyEvent::A2aTaskReceived { .. })),
        "no task may reach cognition when the payment was refused"
    );
}
