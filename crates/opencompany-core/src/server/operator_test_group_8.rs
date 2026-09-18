use super::*;
#[cfg(feature = "openhuman")]
use crate::AppConfig;
use crate::server::router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::operator_test_support_1::*;
#[cfg(feature = "openhuman")]
use super::operator_test_support_2::*;

/// The ask-which question lands in the thread that asked it.
///
/// When two blocked things share a DM and the reply names neither, the
/// runtime asks which one was meant. That question is an answer to the
/// operator's message, so it threads off it the way every other reply in
/// this handler does — otherwise the operator reads their own line in a
/// thread and the teammate's follow-up at the channel root, which is the
/// split this tier exists to close.
#[cfg(feature = "openhuman")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ask_which_question_threads_off_the_reply_that_was_ambiguous() {
    use crate::company::blocker_sender::BlockerSenderSignals;
    use crate::ports::blockers::{BlockerKind, BlockerPayload, BlockerSource, BlockerStep};

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = build_state_with_brain_and_manifest(
        &home,
        "running",
        AppConfig::default(),
        None,
        roster_manifest(),
    )
    .await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).unwrap();
    let app = router(state);

    for (task, connection) in [("t-1", "connection:slack"), ("t-2", "connection:notion")] {
        runtime
            .park_blocker(
                &BlockerPayload {
                    kind: BlockerKind::Infrastructure,
                    source: BlockerSource::Provider,
                    step: Some(BlockerStep::Task {
                        task_id: task.to_string(),
                    }),
                    reason: format!("{connection} refused the call"),
                    needed: "a working connection".to_string(),
                    group_key: Some(connection.to_string()),
                },
                task,
                BlockerSenderSignals {
                    started_by: None,
                    owner_desk: None,
                    assignee: Some("backend_engineer".to_string()),
                },
            )
            .await
            .expect("parks the blocker into the teammate's DM");
    }

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/companies/acme/chat")
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"chat":"dm:backend_engineer","text":"retry it"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let stored = runtime
        .events
        .read_from(
            runtime.id(),
            crate::ports::types::EventSeq::new(0),
            usize::MAX,
        )
        .await
        .expect("read events");
    let asked = stored
        .iter()
        .find_map(|s| match &s.event {
            crate::ports::types::CompanyEvent::OperatorMessage { chat, text, .. }
                if chat.as_deref() == Some("dm:backend_engineer") && text == "retry it" =>
            {
                Some(s.seq)
            }
            _ => None,
        })
        .expect("the operator's ambiguous reply is journalled");
    let prompt = stored
        .iter()
        .find_map(|s| match &s.event {
            crate::ports::types::CompanyEvent::AgentReply {
                chat_id,
                text,
                parent,
                ..
            } if chat_id == "dm:backend_engineer" && text.contains("Which") => {
                Some((text.clone(), *parent))
            }
            _ => None,
        })
        .expect("the runtime asks which of the two was meant");
    assert_eq!(
        prompt.1,
        Some(asked),
        "the ask-which question must hang off the reply that was ambiguous, not the \
         channel root; prompt was {:?}",
        prompt.0
    );
}

#[tokio::test]
async fn chat_by_id_matches_registered_company() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home, "running").await;
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/companies/acme/chat")
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"yo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn unknown_company_is_404() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home, "running").await;
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/companies/ghost/chat")
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"hi"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    // 401, not 404: the caller holds no credential for `ghost`, and
    // authentication precedes existence. Answering "no such company" to an
    // unauthenticated caller would let anyone enumerate which companies a
    // host runs. A user of `ghost` gets a real 404.
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn paused_company_chat_is_409() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home, "paused").await;
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/company/chat")
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"hi"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn list_and_status_routes_report_the_company() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home, "running").await;
    let app = router(state);

    let list = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/companies")
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let bytes = to_bytes(list.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 1);
    assert_eq!(value[0]["id"], "acme");

    let status = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/companies/acme")
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let bytes = to_bytes(status.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["id"], "acme");
}

#[tokio::test]
async fn approvals_list_is_empty_before_any_park() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home, "running").await;
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/company/approvals")
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn amended_approve_resolves_and_returns_responses() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home, "running").await;
    let app = router(state);

    // An `approve` verdict carrying an amended payload routes to the
    // approve-with-edit path. Even against an unknown id it resolves
    // cleanly (nothing to execute) and the follow-up cycle replies.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/company/approvals/missing")
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"verdict":"approve","amended_payload":{"text":"edited"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value["responses"].is_array());
}

/// Issue #618: membership gets you the approval, role gets you its
/// contents.
///
/// Issue #561: the receipt says whether this decision actually released the
/// turn.
///
/// A turn that parked two calls is blocked on two decisions (issue #469
/// continues it once, on the last one). The console used to tell the
/// operator "the agent is completing the action" on the first click, which
/// is false — nothing runs until the second. This is the count it now words
/// that sentence from: one still owed after the first decision, none after
/// the second.
#[tokio::test]
async fn a_receipt_says_how_many_decisions_the_turn_is_still_blocked_on() {
    use crate::runtime::journal::{ApprovalConversation, TaskLink};

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home, "running").await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();

    let effect = |memo: &str| crate::ports::types::Effect {
        kind: "payment.send".into(),
        group: crate::ports::types::EffectGroup::Spend,
        amount_usd: Some(10.0),
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::json!({ "to": "board@example.test", "memo": memo }),
        agent: Some("ceo".into()),
        run_id: None,
    };

    // One turn, two parked calls — the shape an operator meets whenever an
    // agent gates more than once in a turn.
    for (id, memo) in [("appr-561-a", "first"), ("appr-561-b", "second")] {
        runtime
            .journal
            .record_parked(
                &crate::ports::types::ApprovalId::new(id),
                &effect(memo),
                1_000,
                TaskLink::Unlinked,
                ApprovalConversation::default(),
                Some("cycle-561".to_string()),
            )
            .await
            .unwrap();
        runtime.continuations.arm("cycle-561");
    }

    let app = router(state);
    let resolve = |app: axum::Router, id: &'static str| async move {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/company/approvals/{id}"))
                    .header("content-type", "application/json")
                    .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                    .body(Body::from(
                        serde_json::json!({ "verdict": "approve", "detach": true }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
    };

    let first = resolve(app.clone(), "appr-561-a").await;
    assert_eq!(
        first["stillAwaiting"], 1,
        "the first decision releases nothing — the turn is still blocked on the second: {first}"
    );

    // The count is read per decision, at the moment the verdict lands. What
    // happens to the sibling afterwards is issue #848's business — a turn's
    // gated calls may be consolidated and settle together — and this test
    // deliberately asserts only the half the operator's confirmation is
    // worded from: this click did not release the turn.
    //
    // The other half — the last decision reporting nothing outstanding —
    // is pinned on the queue itself in
    // `runtime::continuation::test::outstanding_counts_the_decision_being_made`,
    // where it is deterministic rather than racing a spawned follow-up.
}

/// **The two-account part is the point.** The harness signs every request
/// in as an admin, so a redaction verified only as an admin passes
/// identically against no redaction at all — the test would prove nothing
/// while looking like coverage. This seeds a second, Member-role account
/// and drives the same route with both.
#[tokio::test]
async fn a_member_sees_the_approval_but_not_its_payload_or_amount() {
    use crate::runtime::journal::{ApprovalConversation, TaskLink};

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home, "running").await;
    crate::server::test_support::seed_fixed_member(&state, "acme").await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();

    let effect = crate::ports::types::Effect {
        kind: "payment.send".into(),
        group: crate::ports::types::EffectGroup::Spend,
        amount_usd: Some(2400.0),
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::json!({ "to": "board@example.test", "memo": "Q3 retainer" }),
        agent: Some("ceo".into()),
        run_id: None,
    };
    runtime
        .journal
        .record_parked(
            &crate::ports::types::ApprovalId::new("appr-618"),
            &effect,
            1_000,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();

    let app = router(state);

    async fn approvals_as(app: &axum::Router, cookie: String) -> serde_json::Value {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/company/approvals")
                    .header("cookie", cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    // The admin decides the sign-off, so the admin sees what it will do.
    let as_admin = approvals_as(&app, crate::server::test_support::fixed_cookie("acme")).await;
    let admin_row = &as_admin.as_array().unwrap()[0];
    assert_eq!(admin_row["amount_usd"].as_f64(), Some(2400.0));
    assert_eq!(admin_row["payload"]["to"], "board@example.test");
    assert!(
        admin_row.get("contents_hidden").is_none(),
        "an admin is not told anything was hidden: {admin_row}"
    );

    let as_member = approvals_as(&app, crate::server::test_support::member_cookie("acme")).await;
    let member_row = &as_member.as_array().unwrap()[0];

    // Still visible: everything that makes stalled work legible. This half
    // is what #468 depends on — a member must keep seeing that work is
    // waiting and what kind of call it is.
    assert_eq!(member_row["id"], "appr-618");
    assert_eq!(member_row["kind"], "payment.send");
    assert_eq!(member_row["agent"], "ceo");
    assert_eq!(member_row["at_millis"].as_u64(), Some(1_000));

    // Withheld: the recipient and the money.
    assert!(
        member_row.get("payload").is_none(),
        "the recipient must not reach a member: {member_row}"
    );
    // `null`, not absent: unlike `payload`, `amount_usd` carries no
    // `skip_serializing_if`, so it stays on the wire as an explicit null.
    // Both read as "no value" to the console (`a.amount_usd != null`
    // covers either), and changing the wire shape as a side effect of a
    // redaction would be a worse trade than asserting the shape that is
    // actually there.
    assert!(
        member_row["amount_usd"].is_null(),
        "nor the amount: {member_row}"
    );
    assert_eq!(
        member_row["contents_hidden"], true,
        "and the console must be able to say so rather than render an empty card: {member_row}"
    );

    // Belt and braces: the recipient string must appear nowhere in the
    // member's response, however the shape changes later.
    let raw = serde_json::to_string(&as_member).unwrap();
    assert!(
        !raw.contains("board@example.test") && !raw.contains("Q3 retainer"),
        "payload content leaked to a member: {raw}"
    );
}

/// **Issue #2028 — the bug.** An Approvals click that says `skip` banks a
/// skip. Before the route arm existed the same request banked a `retry`,
/// because `verdict: approve` was the only thing the host read.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_skip_from_the_approvals_page_banks_a_skip() {
    let home_dir = home();
    let c = blocked_company(home_dir.path()).await;

    let (status, answer) = post_resolve(
        &c.app,
        &c.approval_id,
        serde_json::json!({ "verdict": "approve", "blocker_verdict": "skip", "detach": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    let banked = banked_resolutions(&c.home, &c.company).await;
    assert_eq!(banked.len(), 1, "one answer, one banked resolution");
    assert_eq!(
        banked[0]["resolution"]["verdict"], "skip",
        "the operator asked to skip the node, not to run it again"
    );
}

/// The amend twin: the words the operator typed reach the banked resolution
/// verbatim, which is what the re-entered step reads.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_amend_from_the_approvals_page_carries_the_answer_verbatim() {
    let home_dir = home();
    let c = blocked_company(home_dir.path()).await;

    let (status, answer) = post_resolve(
        &c.app,
        &c.approval_id,
        serde_json::json!({
            "verdict": "approve",
            "blocker_verdict": "amend",
            "blocker_answer": "use gpt-4o-mini instead",
            "detach": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    let banked = banked_resolutions(&c.home, &c.company).await;
    assert_eq!(banked.len(), 1);
    assert_eq!(banked[0]["resolution"]["verdict"], "amend");
    assert_eq!(
        banked[0]["resolution"]["answer"], "use gpt-4o-mini instead",
        "the correction must reach the step, or the re-run repeats the failure"
    );
}

/// A cancel still denies, and is still the only verdict that does.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_cancel_from_the_approvals_page_banks_a_cancel() {
    let home_dir = home();
    let c = blocked_company(home_dir.path()).await;

    let (status, answer) = post_resolve(
        &c.app,
        &c.approval_id,
        serde_json::json!({ "verdict": "deny", "blocker_verdict": "cancel", "detach": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    let banked = banked_resolutions(&c.home, &c.company).await;
    assert_eq!(banked.len(), 1);
    assert_eq!(banked[0]["resolution"]["verdict"], "cancel");
}

/// Answering one member of a root-cause group answers all of them — the
/// same fan-out a DM answer performs — and the receipt names every id it
/// settled so the console can drop the siblings' cards too.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_group_settles_together_and_the_receipt_names_every_member() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home, "running").await;
    let company = CompanyId::new("acme");
    let runtime = state.registry().get(&company).unwrap();
    let app = router(state);
    let first = park_node_blocker(&runtime, "grouped-1", Some("connection:slack")).await;
    let second = park_node_blocker(&runtime, "grouped-2", Some("connection:slack")).await;

    let (status, answer) = post_resolve(
        &app,
        &first,
        serde_json::json!({ "verdict": "approve", "blocker_verdict": "skip", "detach": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(
        answer["settledIds"],
        serde_json::json!(["grouped-1", "grouped-2"]),
        "the receipt must name the siblings the answer settled: {answer}"
    );
    assert!(
        runtime.pending_approvals().is_empty(),
        "one answer to a root-cause group retires every member of it"
    );
    let banked = banked_resolutions(&home, &company).await;
    assert_eq!(banked.len(), 2, "both members banked the same verdict");
    for line in &banked {
        assert_eq!(line["resolution"]["verdict"], "skip");
    }
    let _ = second;
}

/// An ordinary resolve is unchanged: no `settledIds` key at all, so a
/// console predating the field reads the same body it always did.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_ordinary_resolve_names_no_settled_ids() {
    let home_dir = home();
    let c = blocked_company(home_dir.path()).await;

    let (status, answer) = post_resolve(
        &c.app,
        &c.approval_id,
        serde_json::json!({ "verdict": "approve", "detach": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(
        answer.get("settledIds").is_none(),
        "a resolve that fanned to nothing must carry no list: {answer}"
    );
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_disagreeing_verdict_pair_is_refused() {
    assert_refused(
        serde_json::json!({ "verdict": "deny", "blocker_verdict": "skip" }),
        "cannot accompany verdict",
    )
    .await;
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_blank_amend_is_refused_rather_than_downgraded() {
    assert_refused(
        serde_json::json!({
            "verdict": "approve",
            "blocker_verdict": "amend",
            "blocker_answer": "   \n\t ",
        }),
        "needs a non-empty blocker_answer",
    )
    .await;
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_amend_with_no_answer_at_all_is_refused() {
    assert_refused(
        serde_json::json!({ "verdict": "approve", "blocker_verdict": "amend" }),
        "needs a non-empty blocker_answer",
    )
    .await;
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_answer_with_no_verdict_is_refused() {
    assert_refused(
        serde_json::json!({ "verdict": "approve", "blocker_answer": "use gpt-4o-mini" }),
        "blocker_answer needs a blocker_verdict",
    )
    .await;
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_answer_on_a_wordless_verdict_is_refused() {
    for verdict in ["retry", "skip", "cancel"] {
        let event = if verdict == "cancel" {
            "deny"
        } else {
            "approve"
        };
        assert_refused(
            serde_json::json!({
                "verdict": event,
                "blocker_verdict": verdict,
                "blocker_answer": "words this verdict cannot carry",
            }),
            "only accompanies blocker_verdict",
        )
        .await;
    }
}

#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_unknown_blocker_verdict_is_refused_by_name() {
    assert_refused(
        serde_json::json!({ "verdict": "approve", "blocker_verdict": "ignore" }),
        "unknown blocker_verdict",
    )
    .await;
}
