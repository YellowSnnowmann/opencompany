use super::*;
use crate::ports::types::EventSeq;
use crate::server::router;
use axum::http::StatusCode;
use tower::ServiceExt;

use super::operator_test_support_1::*;
use super::operator_test_support_2::*;
use super::operator_test_support_3::*;
use super::operator_test_support_4::*;

/// CONC. Two operators on the same card — or one on a double click — reach
/// this route at the same time. The approval may settle once and buy one
/// permission; the loser must be told it was already decided rather than
/// minting a second grant against the same effect.
///
/// Distinct from [`a_second_resolve_reports_already_resolved_and_mints_nothing`],
/// which sends its second request only after the first has fully settled:
/// that one passes even if the parked-set take is a non-atomic
/// check-then-remove, because there is no window for the two to overlap in.
/// These two are in flight together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_simultaneous_resolves_settle_once_and_mint_one_permission() {
    let home_dir = home();
    let c = stalled_company(home_dir.path()).await;
    c.release.notify_one();

    let approve = || {
        resolve_request(
            &c.approval_id,
            serde_json::json!({ "verdict": "approve", "detach": true }),
        )
    };
    // Spawned onto a multi-threaded runtime, so these genuinely overlap
    // rather than being polled to completion one at a time. Eight rather
    // than two because the window a lost take opens is narrow: one pair can
    // miss it by scheduling luck, and a race this test cannot lose is worth
    // more than a tidier number.
    let racers: Vec<_> = (0..8)
        .map(|_| tokio::spawn(c.app.clone().oneshot(approve())))
        .collect();

    let mut settled = Vec::new();
    for racer in racers {
        let response = racer
            .await
            .expect("the request task did not panic")
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        settled.push(
            body["alreadyResolved"]
                .as_bool()
                .unwrap_or_else(|| panic!("a receipt says whether it settled: {body}")),
        );
    }
    assert_eq!(
        settled.iter().filter(|already| !**already).count(),
        1,
        "exactly one simultaneous resolve may settle the approval, got {settled:?}"
    );

    assert!(await_continuation(&c.runtime).await);
    assert_eq!(
        c.runtime.grants.live_count(),
        1,
        "simultaneous approves must not buy more than one permission"
    );
}

/// FAIL. On the synchronous shape the operator waits for the follow-up
/// cycle, so a cycle that falls over is theirs to hear about: the request
/// answers an error rather than a success over nothing.
///
/// And the verdict is durable regardless — it is settled inline, before the
/// cycle is ever spawned. The pairing is the point. An error that also lost
/// the decision would leave the operator re-approving something already
/// approved; an error swallowed into a `200` would leave them believing work
/// resumed that never did.
#[tokio::test]
async fn a_synchronous_resolve_reports_a_failed_follow_up_and_keeps_the_verdict() {
    let home_dir = home();
    let c = multi_park_company(home_dir.path(), 1, Some("sales"), true).await;
    let approval = c.approvals[0].clone();

    let response = c
        .app
        .clone()
        .oneshot(resolve_request(
            &approval,
            serde_json::json!({ "verdict": "approve" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "a continuation that fell over must reach the operator waiting on it"
    );

    assert!(
        !c.runtime
            .pending_approvals()
            .iter()
            .any(|p| p.id == approval),
        "the verdict is settled before the cycle runs, so a failed cycle cannot un-decide it"
    );
    assert_eq!(c.runtime.grants.live_count(), 1);

    let again = c
        .app
        .clone()
        .oneshot(resolve_request(
            &approval,
            serde_json::json!({ "verdict": "approve", "detach": true }),
        ))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(
        body_json(again).await["alreadyResolved"],
        true,
        "re-deciding after the failure must say it was already decided"
    );
    assert_eq!(
        c.runtime.grants.live_count(),
        1,
        "and must not buy a second permission"
    );
}

/// AUTH (IDOR). The client sends a `node_id` only; the host re-resolves it
/// within the *addressed* company's own tree rather than trusting the
/// caller. A real node minted under the exact same id in a different
/// company must not resolve through this one — proving the lookup is
/// scoped per company, not a global id space a guessable ULID could walk.
#[tokio::test]
async fn a_chat_attachment_cannot_cross_a_company_boundary() {
    let home_dir = home();
    let state = state_with_two_companies(home_dir.path()).await;
    let acme = state.registry().get(&CompanyId::new("acme")).unwrap();
    let globex = state.registry().get(&CompanyId::new("globex")).unwrap();

    // The exact same node id, minted for real, but only in globex.
    let shared_id = "n-cross-company";
    globex
        .workspace()
        .create_binary(
            &CompanyId::new("globex"),
            &attachment_binary_node(shared_id, "globex-only.png", "image/png"),
            b"globex bytes",
        )
        .await
        .unwrap();

    let app = router(state);
    let response = app
        .oneshot(chat_with_attachments("acme", vec![shared_id.to_string()]))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "a node real in another company must not resolve through this one's chat"
    );
    assert!(
        acme.events()
            .read_from(&CompanyId::new("acme"), EventSeq::new(0), usize::MAX)
            .await
            .unwrap()
            .iter()
            .all(|stored| !matches!(stored.event, CompanyEvent::OperatorMessage { .. })),
        "a refused attachment must not journal a message with the wrong list"
    );
}

/// INPUT. An id naming nothing in this company's tree, and an id naming a
/// folder rather than a file, are both `400`s — but a genuine non-binary
/// **file** (a text note) is not: only the shape actually rejected is
/// rejected.
#[tokio::test]
async fn a_chat_attachment_refuses_an_unknown_id_and_a_folder_but_admits_a_note() {
    let home_dir = home();
    let state = state_with_company(home_dir.path(), "running").await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    let id = CompanyId::new("acme");
    runtime
        .workspace()
        .create(&id, &attachment_folder_node("f-1", "reports"), None)
        .await
        .unwrap();
    runtime
        .workspace()
        .create(
            &id,
            &attachment_note_node("n-1", "notes.md"),
            Some("just a note"),
        )
        .await
        .unwrap();

    let app = router(state);

    let unknown = app
        .clone()
        .oneshot(chat_with_attachments("acme", vec!["does-not-exist".into()]))
        .await
        .unwrap();
    assert_eq!(
        unknown.status(),
        StatusCode::BAD_REQUEST,
        "an id naming nothing in the tree must be refused"
    );

    let folder = app
        .clone()
        .oneshot(chat_with_attachments("acme", vec!["f-1".into()]))
        .await
        .unwrap();
    assert_eq!(
        folder.status(),
        StatusCode::BAD_REQUEST,
        "a folder id must be refused — it is not a file"
    );

    let admitted = app
        .oneshot(chat_with_attachments("acme", vec!["n-1".into()]))
        .await
        .unwrap();
    assert_eq!(
        admitted.status(),
        StatusCode::OK,
        "a genuine non-binary file (a note) must still resolve"
    );
    let attachments = last_operator_message_attachments(&runtime, &id).await;
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].node_id, "n-1");
}

/// LIMIT. `MAX_CHAT_ATTACHMENTS` (20) is a hard cap on one message: one
/// over is refused before any tree scan or extraction runs, and exactly
/// at the cap is still ordinary, successful traffic.
#[tokio::test]
async fn a_chat_message_may_carry_at_most_twenty_attachments() {
    let home_dir = home();
    let state = state_with_company(home_dir.path(), "running").await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    let id = CompanyId::new("acme");
    let mut ids = Vec::new();
    for n in 0..21 {
        let node_id = format!("n-{n}");
        runtime
            .workspace()
            .create_binary(
                &id,
                &attachment_binary_node(&node_id, &format!("f{n}.bin"), "application/octet-stream"),
                b"x",
            )
            .await
            .unwrap();
        ids.push(node_id);
    }

    let app = router(state);

    let over_cap = app
        .clone()
        .oneshot(chat_with_attachments("acme", ids.clone()))
        .await
        .unwrap();
    assert_eq!(
        over_cap.status(),
        StatusCode::BAD_REQUEST,
        "21 attachments must be refused before any of them are resolved"
    );

    let at_cap = ids[..20].to_vec();
    let ok = app
        .oneshot(chat_with_attachments("acme", at_cap))
        .await
        .unwrap();
    assert_eq!(
        ok.status(),
        StatusCode::OK,
        "exactly 20 attachments is still ordinary traffic, not the refused shape"
    );
    let attachments = last_operator_message_attachments(&runtime, &id).await;
    assert_eq!(attachments.len(), 20);
}

/// BOUND. A repeated id collapses to exactly one resolved attachment — and
/// the cap is measured against the *raw* list the client sent, before
/// dedup, so a client cannot smuggle an over-cap request by repeating one
/// id past the limit and relying on dedup to shrink it back down.
#[tokio::test]
async fn a_chat_attachment_id_repeated_resolves_once_and_the_cap_counts_raw_entries() {
    let home_dir = home();
    let state = state_with_company(home_dir.path(), "running").await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();
    let id = CompanyId::new("acme");
    runtime
        .workspace()
        .create_binary(
            &id,
            &attachment_binary_node("n-dup", "one.png", "image/png"),
            b"one",
        )
        .await
        .unwrap();

    let app = router(state);

    // 21 copies of the same id: one unique attachment after dedup, but the
    // raw count is still over MAX_CHAT_ATTACHMENTS.
    let over_cap_by_repetition = vec!["n-dup".to_string(); 21];
    let refused = app
        .clone()
        .oneshot(chat_with_attachments("acme", over_cap_by_repetition))
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        StatusCode::BAD_REQUEST,
        "the cap must count the raw list the client sent, not the deduplicated one"
    );

    // Comfortably under the cap, repeated three times: dedup must collapse
    // it to exactly one resolved attachment.
    let repeated = vec!["n-dup".to_string(); 3];
    let ok = app
        .oneshot(chat_with_attachments("acme", repeated))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let attachments = last_operator_message_attachments(&runtime, &id).await;
    assert_eq!(
        attachments.len(),
        1,
        "a repeated id must resolve to exactly one attachment, not one per repetition"
    );
    assert_eq!(attachments[0].node_id, "n-dup");
}
