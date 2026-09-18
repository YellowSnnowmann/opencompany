//! Runtime blocker DMs: idempotent late replies, banked verdicts, and concurrent-resolve races.

use crate::company::blocker_sender::BlockerSenderSignals;
use crate::company::runtime::CompanyRuntime;
use crate::company::task_intent::BlockerReplyIntent;
use crate::ports::blockers::{BlockerKind, BlockerPayload, BlockerSource, BlockerStep};
use crate::ports::types::CompanyId;
use std::sync::Arc;
use tempfile::TempDir;

async fn runtime() -> (Arc<CompanyRuntime>, TempDir) {
    let home = tempfile::Builder::new()
        .prefix("opencompany-blocker-dms-")
        .tempdir()
        .expect("tempdir");
    let manifest: crate::company::CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n\
         [[agent]]\nid = \"eng\"\nrole = \"Engineer\"\n",
    )
    .expect("manifest");
    let runtime = Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
            .with_id(CompanyId::new("acme"))
            .build()
            .await
            .expect("runtime"),
    );
    (runtime, home)
}

fn blocker(task_id: &str, group_key: Option<&str>) -> BlockerPayload {
    BlockerPayload {
        kind: BlockerKind::Infrastructure,
        source: BlockerSource::Tool,
        step: Some(BlockerStep::Task {
            task_id: task_id.to_string(),
        }),
        reason: format!("could not connect to mcp server for {task_id}"),
        needed: "the integration reconnected from Apps".to_string(),
        group_key: group_key.map(str::to_string),
    }
}

fn assignee(id: &str) -> BlockerSenderSignals {
    BlockerSenderSignals {
        started_by: None,
        owner_desk: None,
        assignee: Some(id.to_string()),
    }
}

/// Manually parks a blocker with an arbitrary `at_millis` (and
/// therefore an arbitrary deadline), bypassing `park_blocker`'s
/// always-now stamp — the same technique `seed_parked` uses elsewhere
/// in this file, adapted to a real blocker payload so the group it
/// joins is genuine.
async fn park_blocker_at(
    runtime: &Arc<CompanyRuntime>,
    id: &str,
    payload: &BlockerPayload,
    at_millis: u64,
) -> crate::ports::types::ApprovalId {
    use crate::runtime::journal::{ApprovalConversation, TaskLink};
    let approval = crate::ports::types::ApprovalId::new(id);
    let effect = crate::ports::types::Effect {
        kind: payload.effect_kind(),
        group: crate::ports::types::EffectGroup::Other,
        amount_usd: None,
        established_thread: false,
        first_time_counterparty: false,
        payload: serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
        agent: None,
        run_id: None,
    };
    runtime
        .approval_gate
        .rehydrate(approval.clone(), effect.clone(), at_millis);
    runtime
        .journal
        .record_parked(
            &approval,
            &effect,
            at_millis,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .expect("seed parked blocker");
    approval
}

/// **Issue #2028 (P2 review finding).** `blocker_group_members` is
/// oldest-first, so the group's first receipt need not belong to the
/// id the request addressed. An older sibling can expire mid-loop
/// while the addressed blocker settles the requested verdict just
/// fine; the returned receipt must describe the ADDRESSED blocker,
/// not whichever member happens to be oldest.
#[tokio::test]
async fn the_addressed_members_own_outcome_is_reported_not_the_oldest_siblings() {
    let (runtime, _home) = runtime().await;
    let group = Some("connection:slack");
    // Ancient: already past its deadline against real wall-clock time.
    let old = park_blocker_at(&runtime, "old", &blocker("t-old", group), 1).await;
    // Fresh: parked now, nowhere near its deadline.
    let addressed = runtime
        .park_blocker(&blocker("t-new", group), "t-new", assignee("eng"))
        .await
        .expect("parks the addressed blocker");

    let (receipt, follow_up) = runtime
        .apply_blocker_reply_spawned(
            &[old.clone(), addressed.clone()],
            &addressed,
            crate::ports::blockers::BlockerVerdict::Retry,
            "",
            None,
        )
        .await
        .expect("resolves the group");
    crate::company::runtime::join_follow_up(follow_up)
        .await
        .expect("follow-ups run");

    assert_eq!(
        receipt.outcome(),
        "settled",
        "the addressed blocker settled the requested verdict just fine — reporting \
         anything else (e.g. the oldest sibling's \"expired\") tells the operator \
         their own decision failed when it did not: {receipt:?}"
    );

    // Sanity on the test's own premise: the older sibling really did
    // expire in this same call, so a naive "receipts[0]" implementation
    // would have reported exactly that outcome instead.
    assert!(
        runtime.pending_approvals().is_empty(),
        "both members left the pending queue — one settled, one expired"
    );
}

/// **Issue #2028 (P2 review finding).** `parked_blocker_group` returns
/// `None` for BOTH "never a blocker" and "was a blocker, already
/// resolved" — `resolve_blocker` used to 400 either way. A blocker
/// that just settled (another tab, a double-click, a sibling's fan-out
/// beating this request) must answer the same idempotent
/// `AlreadyResolved` an ordinary approval's double-submit gets, not a
/// refusal that tells the operator their successful decision failed.
#[tokio::test]
async fn a_settled_blockers_late_request_is_already_resolved_not_refused() {
    let (runtime, _home) = runtime().await;
    let id = runtime
        .park_blocker(&blocker("t-1", None), "t-1", assignee("eng"))
        .await
        .expect("parks");
    runtime
        .apply_blocker_reply(
            std::slice::from_ref(&id),
            BlockerReplyIntent::Retry,
            "go ahead",
            None,
        )
        .await
        .expect("resolves");
    assert!(
        runtime.parked_blocker_group(&id).is_none(),
        "test setup: the blocker is no longer parked"
    );

    let (receipt, follow_up) = runtime.already_resolved_blocker_receipt(&id).expect(
        "an id that WAS a blocker must get an idempotent answer once it has \
             resolved, not None (which the caller reads as \"never a blocker\" and \
             refuses)",
    );
    assert!(
        matches!(
            receipt,
            crate::runtime::cycle::ResolveReceipt::AlreadyResolved
        ),
        "a settled blocker's late request is AlreadyResolved, not an error: {receipt:?}"
    );
    crate::company::runtime::join_follow_up(follow_up)
        .await
        .expect("the synthetic already-resolved follow-up completes cleanly");
}

/// The other half of the same guard: an id that was never a blocker at
/// all — an unknown id, or an ordinary (non-blocker) approval — must
/// still be refused. Only "was a blocker, now resolved" gets the
/// idempotent answer.
#[tokio::test]
async fn an_id_that_was_never_a_blocker_gets_no_idempotent_answer() {
    let (runtime, _home) = runtime().await;
    assert!(
        runtime
            .already_resolved_blocker_receipt(&crate::ports::types::ApprovalId::new(
                "never-existed"
            ))
            .is_none(),
        "an unknown id must not be answered as a settled blocker"
    );
}

/// The paused card a parked blocker's approval links to.
async fn seed_paused_card(runtime: &Arc<CompanyRuntime>, id: &str) {
    use crate::ports::tasks::{COLUMN_PAUSED, TaskDeliverable, TaskRecord, TaskTitle};

    runtime
        .ops
        .tasks
        .upsert(
            &runtime.id,
            &TaskRecord {
                id: id.to_string(),
                title: TaskTitle::authored("Draft the launch note"),
                note: None,
                column: COLUMN_PAUSED.to_string(),
                priority: "medium".to_string(),
                assignee: "eng".to_string(),
                updated_at_millis: 1,
                origin: crate::ports::TaskOrigin::new(Some("dm:eng".to_string()), None),
                parent_task_id: None,
                output: None,
                plan: None,
                planning_attempts: Vec::new(),
                deliverable: TaskDeliverable::Once,
                workflow_proposal: None,
                origin_run_id: None,
                origin_workflow_id: None,
                origin_message_seq: None,
                bounced: None,
            },
        )
        .await
        .expect("seed card");
}

/// A bare agent question: `step: None`, so the resume has only the
/// approval's task link to work from.
fn question() -> BlockerPayload {
    BlockerPayload {
        kind: BlockerKind::Information,
        source: BlockerSource::AgentQuestion,
        step: None,
        reason: "which cluster should this deploy to?".to_string(),
        needed: "the cluster name".to_string(),
        group_key: None,
    }
}

/// Every verdict the durable journal banked for `id`, in append order —
/// read off disk, not off the in-memory map a resume consumes and
/// clears. What an operator's answer actually recorded.
async fn banked_verdicts(
    home: &std::path::Path,
    company: &CompanyId,
    id: &crate::ports::types::ApprovalId,
) -> Vec<String> {
    let path = crate::store::paths::Bundle::new(home, company).journal_jsonl();
    let raw = tokio::fs::read_to_string(path).await.unwrap_or_default();
    raw.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|line| line["record"] == "BlockerResolved" && line["id"] == id.to_string())
        .filter_map(|line| line["resolution"]["verdict"].as_str().map(str::to_string))
        .collect()
}

/// **Issue #2028 — a late second verdict must not overwrite the answer
/// that already won, deterministically.** No threads: the first request
/// is resolved and resumed to completion, and only then does a second
/// arrive carrying a group list captured before any of it ran — exactly
/// what a second browser tab holds, and what every caller passes
/// (`parked_blocker_group` snapshots outside the lock).
///
/// The loser must write **nothing**. Before the fix it banked its own
/// `record_blocker_resolution` and armed its own answer before
/// `settle_approval` told it that it had lost, so the durable journal
/// gained a Cancel line for an approval the host had settled as Retry,
/// and the armed Cancel was left in the side-channel with no resume left
/// to consume it — for the next boot to re-arm and act on.
#[tokio::test]
async fn a_late_second_verdict_banks_nothing_over_the_answer_that_won() {
    use crate::ports::blockers::BlockerVerdict;

    let (runtime, home) = runtime().await;
    let id = runtime
        .park_blocker(&question(), "t-1", assignee("eng"))
        .await
        .expect("parks");
    // Captured BEFORE the first request runs, and reused afterwards —
    // the stale snapshot every caller holds.
    let group = runtime
        .parked_blocker_group(&id)
        .expect("the blocker is parked");

    let (winner, follow_up) = runtime
        .apply_blocker_reply_spawned(&group, &id, BlockerVerdict::Retry, "", None)
        .await
        .expect("the first request resolves");
    assert_eq!(winner.outcome(), "settled", "the first request wins");
    crate::company::runtime::join_follow_up(follow_up)
        .await
        .expect("its resume runs to completion");

    let (loser, follow_up) = runtime
        .apply_blocker_reply_spawned(&group, &id, BlockerVerdict::Cancel, "", None)
        .await
        .expect("the late request is answered, not refused");
    crate::company::runtime::join_follow_up(follow_up)
        .await
        .expect("it owes no resume");
    assert_eq!(
        loser.outcome(),
        "already_resolved",
        "the late request settled nothing: {loser:?}"
    );

    let banked = banked_verdicts(home.path(), runtime.id(), &id).await;
    assert_eq!(
        banked,
        vec!["retry".to_string()],
        "the durable journal must hold only the verdict that actually settled; a \
         losing request that banks its own is the record disagreeing with the \
         approval event about what the operator decided: {banked:?}"
    );
    assert!(
        runtime.grants.peek_blocker_resolution(&id).is_none(),
        "a losing request must leave nothing armed — an answer banked with no resume \
         left to consume it is what the next boot re-arms and carries out"
    );
}

/// **Issue #2028 (P1 review finding) — the same race, run as a race.**
/// Two operators resolve one blocker with different verdicts
/// concurrently, on a multi-thread runtime so the two really interleave.
/// Whichever verdict the durable approval event names must be the one
/// the resume acts on, the only one banked, and the only one left armed.
///
/// Repeated over fresh runtimes because the losing order is what varies:
/// a single round can have the loser arrive after the winner's resume
/// has already consumed the entry, which is the benign interleaving.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_resolves_cannot_desync_the_armed_verdict_from_the_settled_one() {
    for round in 0..15 {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            one_concurrent_round(round),
        )
        .await
        .expect("a resolve round must not hang");
    }
}

async fn one_concurrent_round(round: usize) {
    use crate::ports::blockers::BlockerVerdict;

    let (runtime, home) = runtime().await;
    let payload = question();
    seed_paused_card(&runtime, "t-1").await;
    let id = runtime
        .park_blocker(&payload, "t-1", assignee("eng"))
        .await
        .expect("parks");

    // Two concurrent requests naming different verdicts for the SAME
    // id. `apply_blocker_reply_spawned` serializes internally, so this
    // is a real race on the lock, not a hand-arranged interleaving.
    let a = {
        let rt = Arc::clone(&runtime);
        let id = id.clone();
        tokio::spawn(async move {
            rt.apply_blocker_reply_spawned(
                std::slice::from_ref(&id),
                &id,
                BlockerVerdict::Retry,
                "",
                None,
            )
            .await
        })
    };
    let b = {
        let rt = Arc::clone(&runtime);
        let id = id.clone();
        tokio::spawn(async move {
            rt.apply_blocker_reply_spawned(
                std::slice::from_ref(&id),
                &id,
                BlockerVerdict::Cancel,
                "",
                None,
            )
            .await
        })
    };
    let (a, b) = tokio::join!(a, b);
    let a = a.expect("task a joins");
    let b = b.expect("task b joins");

    // Exactly one of the two racing requests actually claims the
    // approval (`settle_approval`'s atomic `resolve_outcome`); the
    // loser reads `AlreadyResolved`. Whichever wins, its verdict is
    // what both the durable event AND the armed resume must agree on.
    #[allow(clippy::type_complexity)]
    let settled = |r: &crate::Result<(
        crate::runtime::cycle::ResolveReceipt,
        tokio::task::JoinHandle<crate::Result<crate::runtime::types::CycleReport>>,
    )>| {
        matches!(
            r,
            Ok((crate::runtime::cycle::ResolveReceipt::Settled(_), _))
        )
    };
    let winner_verdict = match (settled(&a), settled(&b)) {
        (true, false) => BlockerVerdict::Retry,
        (false, true) => BlockerVerdict::Cancel,
        (won_a, won_b) => panic!(
            "exactly one request must settle the approval: a settled={won_a} \
             b settled={won_b}"
        ),
    };

    for outcome in [a, b] {
        let (_, follow_up) = outcome.expect("resolves or is already-resolved");
        crate::company::runtime::join_follow_up(follow_up)
            .await
            .expect("follow-up runs");
    }

    // Retry and cancel post different notes into the DM, and exactly
    // one resume runs, so the note that landed must match the winner.
    let notes: Vec<String> = runtime
        .events
        .read_from(
            runtime.id(),
            crate::ports::types::EventSeq::new(0),
            usize::MAX,
        )
        .await
        .expect("read events")
        .into_iter()
        .filter_map(|stored| match stored.event {
            crate::ports::types::CompanyEvent::AgentReply { chat_id, text, .. }
                if chat_id == "dm:eng" =>
            {
                Some(text)
            }
            _ => None,
        })
        .collect();

    let (expected, contradicting) = match winner_verdict {
        BlockerVerdict::Retry => (
            "Got it — picking that back up now.",
            "Okay — I've cancelled that. It's back in To-do if you want to pick it up \
             later.",
        ),
        BlockerVerdict::Cancel => (
            "Okay — I've cancelled that. It's back in To-do if you want to pick it up \
             later.",
            "Got it — picking that back up now.",
        ),
        _ => unreachable!(),
    };
    assert!(
        notes.iter().any(|n| n.as_str() == expected),
        "round {round}: the resume must post the WINNING verdict's note \
         ({expected:?}); posted: {notes:?}"
    );
    assert!(
        !notes.iter().any(|n| n.as_str() == contradicting),
        "round {round}: the resume must never carry out the LOSING request's verdict \
         — found its note ({contradicting:?}) even though the durable event named \
         {winner_verdict:?}: {notes:?}"
    );

    // The note only catches the loser when it overwrote the arming
    // *before* the winner's resume consumed it, which is the narrow
    // window. The journal catches it every time: a losing request that
    // banks at all leaves a second verdict on the record for an
    // approval only one verdict ever settled.
    let banked = banked_verdicts(home.path(), runtime.id(), &id).await;
    assert_eq!(
        banked,
        vec![winner_verdict.as_str().to_string()],
        "round {round}: only the verdict that settled may be banked; the durable \
         record must not disagree with the approval event: {banked:?}"
    );
    assert!(
        runtime.grants.peek_blocker_resolution(&id).is_none(),
        "round {round}: nothing may stay armed once the one resume this approval \
         owed has run — a leftover answer is what the next boot re-arms and acts on"
    );
}
