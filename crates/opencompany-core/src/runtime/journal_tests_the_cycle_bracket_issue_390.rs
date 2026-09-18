use super::tests_core::*;

/// A park line written before #379 has no `thread` key. It must replay as
/// "no thread" rather than failing to parse — which is what leaves every
/// already-parked approval on the Approvals page and in no channel, exactly
/// as it was before this shipped.
///
/// The second half is the one that has to keep working after the resolution:
/// the thread is read off the retained origin, so it survives the queue
/// removal. That is what lets a follow-up cycle's own re-park stay in the
/// channel the first sign-off was asked in.
#[tokio::test]
async fn a_pre_379_parked_line_replays_with_no_thread_and_a_stamped_one_survives_resolution() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let legacy = serde_json::json!({
        "record": "ApprovalParked",
        "id": "appr-legacy",
        "effect": effect(),
        "at_millis": 4_000,
        "task": { "link": "unlinked" },
    });
    tokio::fs::write(&path, format!("{legacy}\n"))
        .await
        .unwrap();

    let journal = RuntimeJournal::new(&path);
    journal.load().await.expect("a pre-#379 line still replays");
    let legacy_id = ApprovalId::new("appr-legacy");
    assert_eq!(journal.pending().len(), 1);
    assert_eq!(
        journal.pending()[0].thread,
        None,
        "no key means no conversation owns it",
    );
    assert_eq!(journal.approval_thread(&legacy_id), Some(None));
    assert_eq!(
        journal.approval_task(&legacy_id),
        Some(Some(TaskLink::Unlinked)),
        "the #333 link is untouched by the new field",
    );

    // A park stamped with the desk channel that produced it.
    let stamped = ApprovalId::new("appr-desk");
    journal
        .record_parked(
            &stamped,
            &effect(),
            5_000,
            TaskLink::Unlinked,
            ApprovalConversation {
                thread: Some("desk-finance".to_string()),
                parent: None,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        journal
            .pending()
            .iter()
            .find(|p| p.id == stamped)
            .unwrap()
            .thread,
        Some("desk-finance".to_string()),
    );

    // Resolving drains the queue but must not drain the origin thread —
    // the follow-up cycle reads it back from here.
    journal.record_resolved(&stamped).await.unwrap();
    assert!(journal.pending().iter().all(|p| p.id != stamped));
    assert_eq!(
        journal.approval_thread(&stamped),
        Some(Some("desk-finance".to_string())),
    );

    // And it round-trips through a reload, from the raw line.
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    let stamped_line = raw
        .lines()
        .find(|l| l.contains("appr-desk"))
        .expect("the stamped park was appended");
    assert!(
        stamped_line.contains(r#""thread":"desk-finance""#),
        "a thread-stamped park must say so on disk: {stamped_line}",
    );
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert_eq!(
        reloaded.approval_thread(&stamped),
        Some(Some("desk-finance".to_string())),
    );
    assert_eq!(reloaded.approval_thread(&legacy_id), Some(None));
}

/// Issue #435: the thread root survives a resolution and a reload on
/// exactly the terms the channel does, and a line written before the field
/// existed replays as "no thread" rather than failing to parse.
///
/// The reload half is the one that matters. The continuation is journaled
/// *after* the operator decides, so a restart between the two is an
/// ordinary case, not an exotic one — and a root that did not survive it
/// would silently drop the answer back into the channel.
#[tokio::test]
async fn a_thread_root_survives_resolution_and_reload_and_a_pre_435_line_replays() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");

    // A pre-#435 line: stamped with a channel, no `parent` key at all.
    let legacy = serde_json::json!({
        "record": "ApprovalParked",
        "id": "appr-pre435",
        "effect": effect(),
        "at_millis": 4_000,
        "task": { "link": "unlinked" },
        "thread": "desk-finance",
    });
    tokio::fs::write(&path, format!("{legacy}\n"))
        .await
        .unwrap();
    let journal = RuntimeJournal::new(&path);
    journal.load().await.expect("a pre-#435 line still replays");
    let legacy_id = ApprovalId::new("appr-pre435");
    assert_eq!(
        journal.approval_conversation(&legacy_id),
        Some(ApprovalConversation {
            thread: Some("desk-finance".to_string()),
            parent: None,
        }),
        "the channel is untouched and the missing root reads as no thread",
    );

    // A park raised inside thread 7 of that channel.
    let threaded = ApprovalId::new("appr-threaded");
    journal
        .record_parked(
            &threaded,
            &effect(),
            5_000,
            TaskLink::Unlinked,
            ApprovalConversation {
                thread: Some("desk-finance".to_string()),
                parent: Some(EventSeq::new(7)),
            },
            None,
        )
        .await
        .unwrap();

    // Resolving drains the queue but must not drain the origin: the
    // continuation reads the root back from here, after the fact.
    journal.record_resolved(&threaded).await.unwrap();
    assert!(journal.pending().iter().all(|p| p.id != threaded));
    assert_eq!(
        journal.approval_conversation(&threaded),
        Some(ApprovalConversation {
            thread: Some("desk-finance".to_string()),
            parent: Some(EventSeq::new(7)),
        }),
    );

    // It is on disk, and it comes back from the raw line.
    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    let line = raw
        .lines()
        .find(|l| l.contains("appr-threaded"))
        .expect("the threaded park was appended");
    assert!(
        line.contains(r#""parent":7"#),
        "a thread-rooted park must say so on disk: {line}",
    );
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert_eq!(
        reloaded.approval_conversation(&threaded),
        Some(ApprovalConversation {
            thread: Some("desk-finance".to_string()),
            parent: Some(EventSeq::new(7)),
        }),
    );
    assert_eq!(
        reloaded.approval_conversation(&legacy_id),
        Some(ApprovalConversation {
            thread: Some("desk-finance".to_string()),
            parent: None,
        }),
    );
    // And the #379 accessor still answers the question it always did.
    assert_eq!(
        reloaded.approval_thread(&threaded),
        Some(Some("desk-finance".to_string())),
    );
}

#[tokio::test]
async fn expired_record_removes_parked_and_survives_reload() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);
    let id = ApprovalId::new("appr-exp");
    journal
        .record_parked(
            &id,
            &effect(),
            now_millis(),
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(journal.pending().len(), 1);

    journal
        .record_expired(&id, now_millis(), ExpiryReason::Ttl)
        .await
        .unwrap();
    assert!(journal.pending().is_empty());

    // A restart replays the expiry: the approval stays gone.
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert!(reloaded.pending().is_empty());

    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(raw.contains("ApprovalExpired"));
}

#[tokio::test]
async fn amended_record_is_audit_only_and_round_trips() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);
    let id = ApprovalId::new("appr-amend");
    journal
        .record_parked(
            &id,
            &effect(),
            now_millis(),
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();

    let mut amended = effect();
    amended.payload = serde_json::json!({ "edited": true });
    journal
        .record_amended(&id, &amended, now_millis())
        .await
        .unwrap();
    // The audit record alone does not drain the queue.
    assert_eq!(journal.pending().len(), 1);
    // The paired resolution removes it.
    journal.record_resolved(&id).await.unwrap();
    assert!(journal.pending().is_empty());

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert!(reloaded.pending().is_empty());

    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(raw.contains("ApprovalAmended"));
    assert!(raw.contains("\"edited\":true"));
}

/// Issue #305: the park instant outlives the parked entry.
///
/// Waiting time is only recoverable by joining a resolved approval back to
/// when it parked, and the event log carries no park time. If the index were
/// cleared alongside `parked` on resolve — the obvious symmetry — every
/// *finished* wait would be unreadable, which is exactly the case the header
/// needs. Expiry (the default-deny path) must retain it for the same reason.
#[tokio::test]
async fn approval_origins_outlive_resolution_and_expiry_and_survive_reload() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    let resolved = ApprovalId::new("appr-resolved");
    let expired = ApprovalId::new("appr-expired");
    journal
        .record_parked(
            &resolved,
            &effect(),
            1_000,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    journal
        .record_parked(
            &expired,
            &effect(),
            2_000,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();

    journal.record_resolved(&resolved).await.unwrap();
    journal
        .record_expired(&expired, 9_000, ExpiryReason::Ttl)
        .await
        .unwrap();

    // Both left the queue...
    assert!(journal.pending().is_empty());
    // ...but their park instants are still joinable.
    let origins = journal.approval_origins();
    assert_eq!(origins.get(&resolved).map(|o| o.at_millis), Some(1_000));
    assert_eq!(origins.get(&expired).map(|o| o.at_millis), Some(2_000));

    // And a restart replays them out of the file, so history predating this
    // process is readable too.
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    let origins = reloaded.approval_origins();
    assert!(reloaded.pending().is_empty());
    assert_eq!(origins.get(&resolved).map(|o| o.at_millis), Some(1_000));
    assert_eq!(origins.get(&expired).map(|o| o.at_millis), Some(2_000));
}

// --- The cycle bracket (issue #390) ----------------------------------

/// A cycle's bracket survives a restart as an *open* one, which is what the
/// boot sweep then settles. Both halves in one test because neither is
/// meaningful alone: an open bracket nobody settles is worse than no
/// bracket — it reports a dead cycle as live work, forever.
#[tokio::test]
async fn an_interrupted_cycle_replays_as_open_and_is_swept_at_boot() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");

    let journal = RuntimeJournal::new(&path);
    journal
        .record_cycle_started("cycle-1", "approval-continuation")
        .await
        .unwrap();
    journal
        .record_cycle_started("cycle-2", "operator-message")
        .await
        .unwrap();
    journal
        .record_cycle_finished("cycle-2", None)
        .await
        .unwrap();

    // A fresh journal over the same file: the host died with `cycle-1` open.
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    let open = reloaded.open_cycles();
    assert_eq!(open.len(), 1, "only the unfinished one replays as open");
    assert_eq!(open[0].cycle_id, "cycle-1");
    assert_eq!(open[0].trigger, "approval-continuation");

    // The sweep settles it, and a second boot finds nothing left to settle —
    // so a swept cycle cannot be settled twice into two contradictory
    // outcomes for one id.
    assert_eq!(reloaded.sweep_interrupted_cycles().await, 1);
    assert!(reloaded.open_cycles().is_empty());

    let after = RuntimeJournal::new(&path);
    after.load().await.unwrap();
    assert!(after.open_cycles().is_empty());
    assert_eq!(
        after.sweep_interrupted_cycles().await,
        0,
        "a second boot has nothing to settle"
    );
}

/// A cycle that fails still closes its bracket, carrying the reason — a
/// failed cycle is not an open one, and an operator reading the bracket
/// needs to tell "it broke" from "it never came back".
#[tokio::test]
async fn a_failed_cycle_closes_its_bracket_with_the_error() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    journal
        .record_cycle_started("cycle-1", "approval-continuation")
        .await
        .unwrap();
    assert_eq!(journal.open_cycles().len(), 1);
    journal
        .record_cycle_finished("cycle-1", Some("the brain fell over".into()))
        .await
        .unwrap();
    assert!(
        journal.open_cycles().is_empty(),
        "a failure closes the bracket; only a host that never came back leaves it open"
    );

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert!(
        reloaded.open_cycles().is_empty(),
        "and it stays closed on replay"
    );
}

/// Issue #243: a grant minted before a restart is still redeemable after it.
///
/// The window between "operator approved" and "agent re-issued the call"
/// spans a model turn, so a deploy or crash inside it is ordinary. Without
/// replay the operator's approval would evaporate and the agent would come
/// back asking for the same permission it had just been given.
#[tokio::test]
async fn a_live_grant_replays_across_a_restart() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    journal
        .record_granted(&grant("appr-1", 1_000))
        .await
        .unwrap();
    assert_eq!(journal.replayed_grants().len(), 1);

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    let replayed = reloaded.replayed_grants();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].approval_id, ApprovalId::new("appr-1"));
    assert_eq!(replayed[0].agent, "finance");
    assert_eq!(replayed[0].tool, "composio_execute");
    assert_eq!(
        replayed[0].args,
        crate::policy::test_support::composio_send_args(),
        "the exact arguments the operator approved survive the restart"
    );
}

/// A single-use grant claimed for a follow-up turn must not re-arm when the
/// tool consumes it but the cycle has not drained that consumption yet.
#[tokio::test]
async fn a_grant_consumed_but_not_yet_drained_does_not_replay_after_a_restart() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    journal
        .record_granted(&grant("appr-crash", 1_000))
        .await
        .unwrap();
    let live = crate::runtime::grants::GrantSet::default();
    live.rehydrate(journal.replayed_grants());
    journal
        .record_grant_dispatched(&ApprovalId::new("appr-crash"), 1_500)
        .await
        .unwrap();
    assert!(
        live.consume(
            "finance",
            "composio_execute",
            &crate::policy::test_support::composio_send_args(),
        )
        .is_some()
    );
    assert_eq!(live.live_count(), 0);

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    let replayed = reloaded.replayed_grants();
    assert_eq!(
        replayed.len(),
        0,
        "a grant already consumed by its claimed turn must not re-arm before the drain: \
         {replayed:?}"
    );
    assert_eq!(
        live.drain_consumed(),
        vec![ApprovalId::new("appr-crash")],
        "the terminal consumption is still waiting for the ordinary cycle drain"
    );
}

#[tokio::test]
async fn a_denied_explicit_request_replays_only_as_a_verdict_continuation() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);
    let continuation = ApprovalContinuation {
        call: GrantedCall {
            tool: crate::ports::types::REQUEST_APPROVAL_EFFECT_KIND.into(),
            ..grant("appr-denied", 1_000)
        },
        verdict: crate::ports::types::Verdict::Deny,
        by: Actor {
            kind: crate::ports::types::ActorKind::User,
            id: "operator".into(),
        },
    };

    journal
        .record_approval_continuation(&continuation)
        .await
        .unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert!(
        reloaded.replayed_grants().is_empty(),
        "a denial must never replay as executable authority"
    );
    assert_eq!(
        reloaded.replayed_approval_continuations(),
        vec![continuation.clone()]
    );

    reloaded
        .record_approval_continuation_dispatched(&continuation.call.approval_id, 2_000)
        .await
        .unwrap();
    let after_dispatch = RuntimeJournal::new(&path);
    after_dispatch.load().await.unwrap();
    assert!(
        after_dispatch.replayed_approval_continuations().is_empty(),
        "a host-durable dispatch claim prevents restart from repeating the turn"
    );

    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(raw.contains("ApprovalContinuationQueued"));
    assert!(raw.contains("ApprovalContinuationDispatched"));
    assert!(!raw.contains("ApprovalGranted"));
}

/// The other half, and the one that actually matters for safety: a grant
/// that already fired — or that lapsed and was announced as lapsed — must
/// NOT come back on replay.
///
/// A single-use grant resurrected by a restart is no longer single-use. The
/// fold therefore *removes* on both terminal records, unlike `origins`
/// (#305) which deliberately retains.
#[tokio::test]
async fn consumed_and_expired_grants_are_not_rehydrated() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    journal
        .record_granted(&grant("consumed", 1_000))
        .await
        .unwrap();
    journal
        .record_granted(&grant("expired", 2_000))
        .await
        .unwrap();
    journal.record_granted(&grant("live", 3_000)).await.unwrap();
    assert_eq!(journal.replayed_grants().len(), 3);

    journal
        .record_grant_consumed(&ApprovalId::new("consumed"), None)
        .await
        .unwrap();
    journal
        .record_grant_expired(&ApprovalId::new("expired"), 9_000)
        .await
        .unwrap();

    let still_live: Vec<_> = journal.replayed_grants();
    assert_eq!(still_live.len(), 1);
    assert_eq!(still_live[0].approval_id, ApprovalId::new("live"));

    // And the removal is durable, not just in-memory.
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    let replayed = reloaded.replayed_grants();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].approval_id, ApprovalId::new("live"));

    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(raw.contains("ApprovalGranted"));
    assert!(raw.contains("GrantConsumed"));
    assert!(raw.contains("GrantExpired"));
}

/// Issue #374: a standing grant survives a restart, with its expiry and the
/// operator who granted it intact.
#[tokio::test]
async fn a_standing_grant_replays_across_a_restart() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    journal
        .record_standing_granted(&standing("g1", "shell", 100_000))
        .await
        .unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    let replayed = reloaded.replayed_standing_grants(2_000);
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].id, GrantId::new("g1"));
    assert_eq!(replayed[0].tool, "shell");
    assert_eq!(replayed[0].expires_at_millis, 100_000);
    assert_eq!(
        replayed[0].granted_by.id, "user-42",
        "who opened this tool up is the point of the record"
    );
}
