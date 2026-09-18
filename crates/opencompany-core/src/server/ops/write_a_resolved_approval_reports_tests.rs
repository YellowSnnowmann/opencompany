//! Integration tests for the `ops` write plane: tasks, memory, workspace,
//! skills, team, inbox-read, and desk chat — exercised end-to-end over the
//! router against a real fs-backed company.

use axum::http::StatusCode;
use serde_json::json;

use super::write_test_support::*;
use crate::ports::types::CompanyId;
use crate::runtime::journal::{ApprovalConversation, TaskLink};

/// A resolved approval keeps its row on the tab, carrying the verdict and the
/// wait it caused. The same resolution the main Approvals page performed, seen
/// from the card: approving on either surface reflects on both.
#[tokio::test]
async fn a_resolved_approval_reports_its_verdict_and_wait_on_the_tab() {
    use crate::ports::types::{Actor, ActorKind, ApprovalId, CompanyEvent, Verdict};

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = CompanyId::new("acme");
    let (runtime, dispatched_at) = dispatched_task(&state, &company).await;

    let id = ApprovalId::new("appr-done");
    let parked_at = dispatched_at + 20;
    runtime
        .journal
        .record_parked(
            &id,
            &parked_effect(),
            parked_at,
            TaskLink::Task { id: "t-1".into() },
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    runtime.journal.record_resolved(&id).await.unwrap();
    runtime
        .events()
        .append(
            &company,
            CompanyEvent::ApprovalResolved {
                approval_id: id,
                verdict: Verdict::Deny,
                by: Actor {
                    kind: ActorKind::Operator,
                    id: "owner".into(),
                },
            },
        )
        .await
        .unwrap();

    let (_, body) = send(&state, "GET", "/api/v1/company/tasks/t-1", None).await;
    let approvals = body["approvals"].as_array().unwrap();
    assert_eq!(approvals.len(), 1, "{approvals:?}");
    let row = &approvals[0];
    assert_eq!(row["status"], "denied");
    // The row is anchored at the *park*, so approvals read in the order things
    // were asked rather than the order they were answered.
    assert_eq!(row["atMillis"].as_u64().unwrap(), parked_at);
    // The park→resolve span moved off this row with the Approvals tab (#468).
    // It is unchanged, and still asserted here — on the `approval` timeline
    // entry, which is where it now lives. Dropping the assertion along with the
    // field would have quietly retired the arithmetic's only coverage.
    let entry = body["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "approval")
        .expect("a resolved approval reaches the timeline");
    let resolved_at = entry["atMillis"].as_u64().unwrap();
    assert!(resolved_at > parked_at);
    assert_eq!(
        entry["waitedMillis"].as_u64().unwrap(),
        resolved_at - parked_at,
    );
    // Nothing is parked any more, so the card is not still waiting.
    assert!(body.get("waitingSince").is_none());
    // The join must not become a new identity leak.
    let raw = serde_json::to_string(&body["approvals"]).unwrap();
    assert!(!raw.contains("owner"), "operator identity leaked: {raw}");
}

/// A task with no approvals reports an empty list, not a fabricated one — the
/// honest empty state the console renders.
///
/// Covers *another card's* approval parked mid-window. The case where the
/// approval belongs to nothing at all is
/// [`an_unlinked_approval_is_not_absorbed_by_the_running_card`], which is a
/// different fact and was the one the old window got wrong.
#[tokio::test]
async fn a_task_with_no_approvals_of_its_own_reports_an_empty_list() {
    use crate::ports::types::ApprovalId;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = CompanyId::new("acme");
    let (runtime, dispatched_at) = dispatched_task(&state, &company).await;

    // Parked for a different card entirely, while this one is mid-run.
    runtime
        .journal
        .record_parked(
            &ApprovalId::new("appr-elsewhere"),
            &parked_effect(),
            dispatched_at + 5,
            TaskLink::Task {
                id: "t-other".into(),
            },
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();

    let (status, body) = send(&state, "GET", "/api/v1/company/tasks/t-1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["approvals"].as_array().unwrap().is_empty(),
        "{:?}",
        body["approvals"],
    );
    assert!(
        body.get("waitingSince").is_none(),
        "another card's approval must not make this one read as waiting",
    );
}

/// An approval that belongs to **no** card — a workflow delivery, a chat turn,
/// a scheduler tick — parked while a card is mid-run must not be absorbed by
/// that card (#333 review follow-up).
///
/// This is the case the first cut of the ownership test got wrong. It tested
/// `origins.get(id).and_then(|o| o.task_id)`, which is `None` both for a park
/// that recorded no card *and* for a pre-#333 park that could not record one —
/// so every unlinked park since #333 fell through to the run window and landed
/// on whatever happened to be running, dragging `waitingSince` with it.
#[tokio::test]
async fn an_unlinked_approval_is_not_absorbed_by_the_running_card() {
    use crate::ports::types::ApprovalId;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = CompanyId::new("acme");
    let (runtime, dispatched_at) = dispatched_task(&state, &company).await;

    // The shape `workflows::delivery` writes: parked mid-window, owned by
    // nothing, and recorded as such.
    runtime
        .journal
        .record_parked(
            &ApprovalId::new("appr-delivery"),
            &parked_effect(),
            dispatched_at + 5,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();

    let (status, body) = send(&state, "GET", "/api/v1/company/tasks/t-1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["approvals"].as_array().unwrap().is_empty(),
        "an approval owned by no card must not appear on a card: {:?}",
        body["approvals"],
    );
    assert!(
        body.get("waitingSince").is_none(),
        "nor may it make that card read as waiting on the operator",
    );
}

/// The two correlation keys, in the order the read side resolves them: the
/// attempt (`run_id`, #242) is authoritative wherever it is present, and the
/// parked card link (#333) is the fallback for every park with no attempt
/// behind it.
///
/// Neither key is a superset of the other, which is why both are kept. A
/// `RunRecord` names its card, so a run id resolves to a task — but a task id
/// can never say which *attempt* parked an approval, and #183 settled that
/// repeat trips through review are normal. Meanwhile `run_id` is `None` by
/// design for a chat turn, a workflow delivery, or the hosted brain's gate, so
/// it cannot be the only key either.
#[tokio::test]
async fn the_attempt_id_outranks_the_card_link_when_both_are_present() {
    use crate::ports::runs::NewRun;
    use crate::ports::types::ApprovalId;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = CompanyId::new("acme");
    let (runtime, dispatched_at) = dispatched_task(&state, &company).await;

    // Two attempts at this card, and one at another — the case a card-level key
    // alone cannot tell apart.
    for (id, task) in [("run-a", "t-1"), ("run-b", "t-1"), ("run-c", "t-other")] {
        runtime
            .runs()
            .create_run(&company, NewRun::for_task(id, task, "ceo"))
            .await
            .unwrap();
    }

    let under_run = |run: &str| {
        let mut effect = parked_effect();
        effect.run_id = Some(run.to_string());
        effect
    };

    // Parked under this card's *second* attempt, and stamped Unlinked at the
    // card level. The run id is authoritative, so it still lands here.
    runtime
        .journal
        .record_parked(
            &ApprovalId::new("appr-attempt-2"),
            &under_run("run-b"),
            dispatched_at + 5,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    // Parked under another card's attempt, but stamped with *our* card. The run
    // id outranks the link, so it must not appear.
    runtime
        .journal
        .record_parked(
            &ApprovalId::new("appr-elsewhere"),
            &under_run("run-c"),
            dispatched_at + 6,
            TaskLink::Task { id: "t-1".into() },
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();

    let (status, body) = send(&state, "GET", "/api/v1/company/tasks/t-1", None).await;
    assert_eq!(status, StatusCode::OK);
    let approvals = body["approvals"].as_array().unwrap();
    assert_eq!(approvals.len(), 1, "{approvals:?}");
    assert_eq!(
        approvals[0]["id"], "appr-attempt-2",
        "the attempt id decides ownership, not the card link",
    );
}

/// The **queue** answers ownership the same way the card does (#1891).
///
/// [`the_attempt_id_outranks_the_card_link_when_both_are_present`] pins the task
/// detail read. `GET …/approvals` projected the raw park stamp instead, so the
/// two surfaces disagreed about the same approval: the card refused to show
/// `appr-elsewhere` and the queue handed it out labelled `t-1`. Every console
/// join on that link — the board's blocked row, the Approvals page's per-card
/// filter — inherited the disagreement.
///
/// Read-only that was a wrong label. Once the board card grew Approve and
/// Decline it became an operator resolving another card's request, so the two
/// reads are pinned against each other here rather than left to agree by
/// convention.
#[tokio::test]
async fn the_queue_resolves_ownership_the_same_way_the_card_does() {
    use crate::ports::runs::NewRun;
    use crate::ports::types::ApprovalId;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = CompanyId::new("acme");
    let (runtime, dispatched_at) = dispatched_task(&state, &company).await;

    for (id, task) in [("run-b", "t-1"), ("run-c", "t-other")] {
        runtime
            .runs()
            .create_run(&company, NewRun::for_task(id, task, "ceo"))
            .await
            .unwrap();
    }

    let under_run = |run: &str| {
        let mut effect = parked_effect();
        effect.run_id = Some(run.to_string());
        effect
    };

    // Stamped with this card, parked under another card's attempt. The card
    // read refuses it; the queue must not label it `t-1` either.
    runtime
        .journal
        .record_parked(
            &ApprovalId::new("appr-elsewhere"),
            &under_run("run-c"),
            dispatched_at + 5,
            TaskLink::Task { id: "t-1".into() },
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    // Stamped Unlinked *and* carrying a run id — which `workflow_run_of` reads
    // as a workflow park, because the two id spaces are indistinguishable by
    // value. The card read claims it (it checks membership in this card's own
    // attempt ids, which the queue has no way to do); the queue leaves it
    // alone rather than risk relabelling a workflow approval onto a card.
    runtime
        .journal
        .record_parked(
            &ApprovalId::new("appr-attempt-2"),
            &under_run("run-b"),
            dispatched_at + 6,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    // No attempt at all: the stamp is the whole answer, unchanged.
    runtime
        .journal
        .record_parked(
            &ApprovalId::new("appr-stamped"),
            &parked_effect(),
            dispatched_at + 7,
            TaskLink::Task { id: "t-1".into() },
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();

    let (status, body) = send(&state, "GET", "/api/v1/company/approvals", None).await;
    assert_eq!(status, StatusCode::OK);
    let queue = body.as_array().unwrap();
    let owner_of = |id: &str| {
        queue
            .iter()
            .find(|row| row["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from the queue: {queue:?}"))["task"]
            .clone()
    };

    assert_eq!(
        owner_of("appr-elsewhere"),
        json!({ "link": "task", "id": "t-other" }),
        "the attempt outranks the stamp on the queue, exactly as on the card",
    );
    assert_eq!(
        owner_of("appr-attempt-2"),
        json!({ "link": "unlinked" }),
        "an Unlinked park carrying a run id is a workflow park by `workflow_run_of`'s \
         rule, and the queue must not claim it for a card on the strength of an id \
         whose space it cannot identify",
    );
    assert_eq!(
        owner_of("appr-stamped"),
        json!({ "link": "task", "id": "t-1" }),
        "a park with no attempt keeps the link it was stamped with",
    );

    // The pinning half, and it is a **subset** rather than an equality, which is
    // the honest shape of the guarantee.
    //
    // The queue may never claim an approval the card does not — that direction
    // is the defect, and it is what puts a decision the operator should not
    // have in front of them. It may fall short: `approval_owner` asks whether a
    // run is among *this card's* attempts, which the queue cannot ask without
    // per-card state, so where the id space is ambiguous the queue abstains.
    // The cost of abstaining is a blocked row the board does not draw; the cost
    // of the other direction is deciding somebody else's request.
    let (_, card) = send(&state, "GET", "/api/v1/company/tasks/t-1", None).await;
    let on_card: std::collections::HashSet<&str> = card["approvals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    let from_queue: std::collections::HashSet<&str> = queue
        .iter()
        .filter(|row| row["task"] == json!({ "link": "task", "id": "t-1" }))
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert!(
        from_queue.is_subset(&on_card),
        "the queue must never put an approval on a card the card itself disowns: \
         queue={from_queue:?} card={on_card:?}",
    );
    assert!(
        from_queue.contains("appr-stamped"),
        "and must still carry the unambiguous ones: {from_queue:?}",
    );
    assert!(
        !from_queue.contains("appr-elsewhere"),
        "least of all the one parked under another card's attempt: {from_queue:?}",
    );
}

/// An approval parked by a build older than #333 carries no link at all. It
/// keeps the pre-#333 run-window correlation rather than vanishing, so existing
/// history still renders.
///
/// The legacy line is written **raw and replayed**, not produced by
/// `record_parked` — which is the point. Since #333 there is no way to record a
/// park without a link, so the only source of a missing one is a file written
/// by an older host, and that is exactly what this pins. Contrast
/// [`an_unlinked_approval_is_not_absorbed_by_the_running_card`]: same "no task
/// id", opposite outcome, because one is unrecorded and the other is recorded.
#[tokio::test]
async fn a_pre_333_approval_falls_back_to_the_run_window() {
    use crate::ports::types::{Actor, ActorKind, ApprovalId, CompanyEvent, Verdict};
    use crate::store::paths::Bundle;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = CompanyId::new("acme");
    let (runtime, dispatched_at) = dispatched_task(&state, &company).await;

    // A journal line as an older host wrote it: no `task` key whatsoever.
    let legacy = json!({
        "record": "ApprovalParked",
        "id": "appr-legacy",
        "effect": parked_effect(),
        "at_millis": dispatched_at + 5,
    });
    let path = Bundle::new(&home, runtime.id()).journal_jsonl();
    tokio::fs::write(&path, format!("{legacy}\n"))
        .await
        .unwrap();
    runtime.journal.load().await.unwrap();

    let (_, body) = send(&state, "GET", "/api/v1/company/tasks/t-1", None).await;
    let approvals = body["approvals"].as_array().unwrap();
    assert_eq!(approvals.len(), 1, "{approvals:?}");
    assert_eq!(approvals[0]["id"], "appr-legacy");
    assert_eq!(
        body["waitingSince"].as_u64().unwrap(),
        dispatched_at + 5,
        "the legacy live-wait behaviour is unchanged",
    );

    let id = ApprovalId::new("appr-legacy");
    runtime.journal.record_resolved(&id).await.unwrap();
    runtime
        .events()
        .append(
            &company,
            CompanyEvent::ApprovalResolved {
                approval_id: id,
                verdict: Verdict::Approve,
                by: Actor {
                    kind: ActorKind::User,
                    id: "u-1".into(),
                },
            },
        )
        .await
        .unwrap();

    let (_, body) = send(&state, "GET", "/api/v1/company/tasks/t-1", None).await;
    let approvals = body["approvals"].as_array().unwrap();
    assert_eq!(approvals.len(), 1, "{approvals:?}");
    assert_eq!(approvals[0]["id"], "appr-legacy");
    assert_eq!(approvals[0]["status"], "approved");
    // As above (#468): the span lives on the timeline entry now, and the legacy
    // clamping behaviour is asserted there rather than dropped.
    let entry = body["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "approval")
        .expect("a resolved approval reaches the timeline");
    let resolved_at = entry["atMillis"].as_u64().unwrap();
    assert_eq!(
        entry["waitedMillis"].as_u64().unwrap(),
        resolved_at.saturating_sub(dispatched_at + 5),
        "the resolved legacy row keeps the original park-to-resolve wait",
    );
}
