use super::tests_core::*;

/// **Issue #1825** (Codex finding `3866158654`). A blocked agent-node's
/// gated tool call park is host-durable too — the same reasoning as
/// `only_a_workflow_gate_park_is_host_durable` above, one caller down:
/// `BlockedNodeStashed` durably carries the continuation, but nothing
/// regenerates a lost `ApprovalParked` card, so a card lost to a host
/// crash between park and approve strands the question forever with no
/// re-park coming.
///
/// Discriminated by `run_id` rather than `kind`, because a blocked node's
/// park carries the tool's own name — it varies per call, unlike
/// `WORKFLOW_APPROVE_KIND`. See `ApprovalRequestQueue::stamp_run`'s doc for
/// why `run_id` is exactly the field a workflow-dispatched call has and a
/// chat turn's own call does not.
#[test]
fn a_blocked_node_tool_call_park_is_host_durable_too() {
    let mut blocked_node_park = parked_with_kind("shell");
    let JournalRecord::ApprovalParked { effect, .. } = &mut blocked_node_park else {
        panic!("parked_with_kind always returns ApprovalParked");
    };
    effect.run_id = Some("run-1".to_string());
    assert_eq!(
        blocked_node_park.durability(),
        Durability::Host,
        "a blocked node's card is the only durable record that can put its \
         decision in front of the operator again — losing it strands the \
         stash `BlockedNodeStashed` keeps alive behind a question nobody can \
         answer"
    );

    // A chat turn's own gated call carries no `run_id` (`stamp_run` leaves
    // it `None`) and must not be swept up by the widened arm above — it
    // still re-parks on its own next attempt.
    assert_eq!(
        parked_with_kind("shell").durability(),
        Durability::Process,
        "a chat-turn park has no run_id and re-asks on its own"
    );
}

/// **Issue #392**: a host-durable record's append really does take the
/// flushing path, and a process-durable one really does not.
///
/// What this proves and what it does not. It proves the **policy** (which
/// kinds route where) and the **plumbing** (the flush was requested and its
/// syscall returned `Ok` — the append would have failed otherwise), plus
/// that the record is readable back afterwards. It does **not** prove the
/// bytes reached stable storage, and no portable unit test can: a synced and
/// an unsynced line are byte-identical on disk, and `SIGKILL` does not drop
/// the page cache, so no process-level test can simulate host loss. Proving
/// that needs a crash-consistency harness — dm-log-writes, or a VM whose
/// unsynced page cache is dropped — which this repo does not have. The OS
/// contract carries the rest.
#[tokio::test]
async fn host_records_route_through_the_durable_append() {
    use crate::store::fs::append_probe;

    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    journal.record_cycle_started("c-1", "test").await.unwrap();
    assert_eq!(
        append_probe::counts(&path),
        (1, 0),
        "CycleStarted is process-durable and must not flush"
    );

    journal.record_executed("k-1", executed(1)).await.unwrap();
    assert_eq!(
        append_probe::counts(&path),
        (1, 1),
        "EffectExecuted must flush before its side effect runs"
    );

    journal
        .record_standing_revoked(&GrantId::new("s-1"), revoker(), 6)
        .await
        .unwrap();
    assert_eq!(
        append_probe::counts(&path),
        (1, 2),
        "StandingGrantRevoked must flush"
    );

    journal
        .record_grant_consumed(&ApprovalId::new("appr-1"), None)
        .await
        .unwrap();
    assert_eq!(
        append_probe::counts(&path),
        (1, 3),
        "GrantConsumed must flush: losing it re-arms a grant whose tool ran"
    );

    journal.record_cycle_finished("c-1", None).await.unwrap();
    assert_eq!(
        append_probe::counts(&path),
        (2, 3),
        "CycleFinished is process-durable and must not flush"
    );

    // The durable path must leave a journal that still replays.
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert!(reloaded.is_executed("k-1"), "the flushed commit replays");
}

/// **Issue #392**: a host-durable record creates its journal's parent chain
/// durably too.
///
/// The wiring half of `create_dir_all_durable`. A journal's *first* append
/// is the one that creates the directories, and it is also the one most
/// likely to be an `EffectExecuted` commit. Reaching for the plain
/// `create_dir_all` there would flush the record under ancestors that were
/// not flushed, and a host crash takes the subtree — and the record with it.
#[cfg(unix)]
#[tokio::test]
async fn a_host_record_flushes_the_directories_its_journal_creates() {
    use crate::store::fs::append_probe;

    let dir = tmp_dir();
    let companies = dir.path().join("companies");
    let home = companies.join("acme");
    let journal = RuntimeJournal::new(home.join("journal.jsonl"));

    journal.record_executed("k-1", executed(1)).await.unwrap();

    for (created, holder) in [("companies", dir.path()), ("acme", &companies)] {
        assert!(
            append_probe::dir_syncs(holder) > 0,
            "the directory holding the entry naming `{created}` must be flushed \
             before a host-durable record is reported durable"
        );
    }
    assert!(
        append_probe::dir_syncs(&home) > 0,
        "the journal file's own directory entry must be flushed"
    );
}

/// **Issue #392**: a commit whose append fails must release its key, so the
/// next attempt under that key is a real retry.
///
/// Holding the key would make the failure silent in the worst way. The
/// append error aborts `execute_effect_once` before `perform_effect`, so
/// nothing external ran; a later attempt would then find the key present,
/// take the already-committed early return, and report `Ok` for an effect
/// that never happened and never will.
///
/// The fault is injected by putting a **directory** where the journal file
/// belongs: `append`'s `create_dir_all` on the parent still succeeds, and
/// the append's own `open` then fails with `EISDIR`. A real failure of the
/// real `append_line_durable`, with no production seam added to stage it.
#[tokio::test]
async fn a_failed_commit_releases_the_key_for_a_retry() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    tokio::fs::create_dir_all(&path).await.unwrap();
    let journal = RuntimeJournal::new(&path);

    let first = journal.record_executed("k-1", executed(1)).await;
    assert!(
        first.is_err(),
        "the append cannot land on a path occupied by a directory"
    );
    assert!(
        !journal.is_executed("k-1"),
        "a key whose commit never reached the file must not be held: \
         the effect did not run, so the key must not claim it did"
    );

    let retry = journal.record_executed("k-1", executed(1)).await;
    assert!(
        retry.is_err(),
        "the retry must re-attempt the commit and surface the failure again, \
         never take the already-committed early return and report success"
    );
}

/// A live release must retire a turn from `blocked_node_approvals` exactly
/// as replaying its `BlockedNodeReleased` record does — the doc-stated
/// invariant on the field itself: "a turn never lingers here past the
/// continuation it describes."
///
/// `record_blocked_node_released` used to remove only from
/// `blocked_stashes`, leaving the turn's entry in
/// `blocked_node_approvals` for the rest of the process's life — a
/// long-running tenant that parks and approves many blocked nodes
/// accumulates one stale key per release, unlike a fresh reload, which
/// (via `replay`) always retires both together. Calling the live method
/// directly, with no restart in between, isolates exactly that gap.
#[tokio::test]
async fn release_retires_the_turn_from_blocked_node_approvals_live() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    journal
        .record_blocked_node_stashed(
            "turn-1",
            "wf-1",
            &serde_json::json!({}),
            &StartedBy::Operator,
        )
        .await
        .unwrap();
    journal
        .record_blocked_node_approved("turn-1")
        .await
        .unwrap();
    assert_eq!(journal.blocked_node_approvals(), vec!["turn-1".to_string()]);

    journal
        .record_blocked_node_released("turn-1")
        .await
        .unwrap();
    assert!(
        journal.blocked_node_approvals().is_empty(),
        "a released turn must not linger in the live approval set — it \
         must be retired the moment release lands, not only on the next \
         reload's replay"
    );
}

/// The mirror image of the test above: unlike `blocked_node_approvals`,
/// a live release must **not** retire the turn from
/// `blocked_node_dispatched` (finding `3877914597`).
///
/// `resume_blocked_agent_node`'s guard against a ghost decision (issue
/// #1825, finding `3877718169`) reads only this set to tell "already
/// dispatched" apart from "genuinely nothing left on this host". A ghost
/// `ApprovalResolved` can reach that guard *after* the turn's own
/// continuation already ran to completion and its `BlockedNodeReleased`
/// already landed — a host crash loses only the process-durable
/// resolution while the host-durable park, approve, and dispatch facts
/// all survive. If release cleared this set too (as it used to, mirroring
/// `blocked_node_approvals` above), the guard would read `false` for
/// exactly that case and the ghost would fall through into the "no stash
/// on this host" branch, which tells the operator to re-run the workflow
/// by hand — manually repeating the tool call the continuation already
/// ran once. Proven live and across a reload, since the guard's only
/// caller reads a freshly-replayed journal at boot.
#[tokio::test]
async fn release_does_not_retire_the_turn_from_blocked_node_dispatched_live() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    journal
        .record_blocked_node_stashed(
            "turn-1",
            "wf-1",
            &serde_json::json!({}),
            &StartedBy::Operator,
        )
        .await
        .unwrap();
    journal
        .record_blocked_node_approved("turn-1")
        .await
        .unwrap();
    journal
        .record_blocked_node_dispatched("turn-1")
        .await
        .unwrap();
    assert!(journal.is_blocked_node_dispatched("turn-1"));

    journal
        .record_blocked_node_released("turn-1")
        .await
        .unwrap();
    assert!(
        journal.is_blocked_node_dispatched("turn-1"),
        "the dispatch tombstone must survive its own turn's release — a \
         ghost decision reaching this turn after release is exactly the \
         case issue #1825's guard exists to catch, and the guard reads \
         this set"
    );

    // A fresh reload's replay must fold the same record the same way.
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert!(
        reloaded.is_blocked_node_dispatched("turn-1"),
        "replay must agree with the live path — the boot-time \
         reconciler and the live resume guard cannot disagree about \
         whether a turn was already dispatched"
    );
}

/// Issue #1825 (P1 — found by chatgpt-codex-connector): a park-time
/// `record_blocked_node_stashed` whose first durable append fails
/// transiently must have that append retried by the settle-time fallback
/// call, not silently skipped.
///
/// # The bug this reproduces
///
/// The in-memory insert lands before the append that backs it durably, so
/// the first (failing) call still leaves `turn` in `blocked_stashes` —
/// that half is correct and load-bearing (a resolve landing before settle
/// must still find an in-memory stash to release). But the early-return
/// guard used to read that in-memory presence as proof the append had
/// already landed: the settle-time fallback's call (the *same*
/// `(turn, workflow_id, input)`, by construction) would see the entry,
/// assume it was the redundant second write, and return `Ok(())` without
/// ever appending. A restart between that skipped retry and the run
/// re-dispatching then replays no `BlockedNodeStashed` line at all — the
/// approval card is still there, clickable, but `BlockedNodeQueue::rearm`
/// has nothing to rebuild the stash from.
///
/// # Why this proves the fix
///
/// `FailOnceJournalStore` fails only the very first append, so the second
/// `record_blocked_node_stashed` call — standing in for the settle-time
/// fallback — hits a store that is willing to succeed. Pre-fix, the
/// early-return means that willingness is never tested: the assertion
/// that a fresh reload actually replays the stash fails, because nothing
/// durable was ever written. Post-fix the second call retries the append,
/// it lands, and a reload rehydrates `turn-1`.
#[tokio::test]
async fn a_stash_whose_first_durable_append_failed_is_retried_and_lands() {
    let company = CompanyId::new("acme");
    let store = Arc::new(FailOnceJournalStore::new());
    let journal = RuntimeJournal::with_store(store.clone(), company.clone());
    let input = serde_json::json!({ "request": "quarterly numbers" });

    // Park time: the first append fails transiently. The in-memory stash
    // still has to be there for a fast resolve to release, so this must
    // not be lost even though the call itself reports an error.
    let first = journal
        .record_blocked_node_stashed("turn-1", "wf-1", &input, &StartedBy::Operator)
        .await;
    assert!(
        first.is_err(),
        "the forced first-append failure must surface, not be swallowed"
    );
    assert_eq!(
        journal.blocked_stashes(),
        vec![(
            "turn-1".to_string(),
            "wf-1".to_string(),
            input.clone(),
            StartedBy::Operator,
            None,
            None,
        )],
        "the in-memory stash must still be there after a failed append — a resolve \
         landing before the next retry has to find it"
    );

    // Settle time: the fallback calls the same method with the same
    // facts. The store is willing to succeed now — the fix must actually
    // retry the append instead of taking the early return.
    let second = journal
        .record_blocked_node_stashed("turn-1", "wf-1", &input, &StartedBy::Operator)
        .await;
    assert!(
        second.is_ok(),
        "the retry must re-attempt the durable append rather than silently reporting \
         success off the in-memory presence alone: {second:?}"
    );

    // The proof that it actually landed, not merely that `Ok` came back:
    // a fresh journal over the same store replays the stash from the
    // durable line.
    let reloaded = RuntimeJournal::with_store(store, company);
    reloaded.load().await.unwrap();
    assert_eq!(
        reloaded.blocked_stashes(),
        vec![(
            "turn-1".to_string(),
            "wf-1".to_string(),
            input,
            StartedBy::Operator,
            None,
            None,
        )],
        "a restart must rehydrate this stash — the retried append is the only durable \
         record of it, and pre-fix it was never written at all"
    );
}

/// Issue #1862 prerequisite (`CodeRabbit`, comment `3879554180`; also
/// raised by `chatgpt-codex-connector`, comment `3879402310`): a
/// `BlockedNodeStashed` line written before this issue added `started_by`
/// to the record must still replay — `#[serde(default)]` is what makes
/// that true, degrading to `Operator` rather than failing the whole
/// journal load, the same fallback the pre-#1862 code path always used.
#[tokio::test]
async fn a_pre_1862_blocked_node_stashed_line_replays_as_operator() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let legacy = serde_json::json!({
        "record": "BlockedNodeStashed",
        "turn": "turn-legacy",
        "workflow_id": "wf-legacy",
        "input": { "request": "before #1862" },
        "at_millis": 1_000,
    });
    tokio::fs::write(&path, format!("{legacy}\n"))
        .await
        .unwrap();

    let journal = RuntimeJournal::new(&path);
    journal
        .load()
        .await
        .expect("a pre-#1862 line with no started_by field still replays");

    let stashes = journal.blocked_stashes();
    assert_eq!(stashes.len(), 1);
    let (turn, workflow_id, input, started_by, thread_id, workflow_fingerprint) = &stashes[0];
    assert_eq!(turn, "turn-legacy");
    assert_eq!(workflow_id, "wf-legacy");
    assert_eq!(input, &serde_json::json!({ "request": "before #1862" }));
    assert_eq!(
        started_by,
        &StartedBy::Operator,
        "a legacy line with no started_by field degrades to Operator, the coarse \
         pre-#1862 fallback — not a load failure"
    );
    assert!(thread_id.is_none());
    assert!(workflow_fingerprint.is_none());
}

#[tokio::test]
async fn a_blocked_stash_rehydrates_its_checkpoint_lineage() {
    let dir = tmp_dir();
    let journal = RuntimeJournal::new(dir.path().join("journal.jsonl"));
    journal
        .record_blocked_node_stashed_checkpointed(
            "turn-1",
            "wf-1",
            &serde_json::json!({ "request": "resume" }),
            &StartedBy::Operator,
            Some("thread-1"),
            Some("fingerprint-1"),
        )
        .await
        .expect("stash persists");

    let reloaded = RuntimeJournal::new(dir.path().join("journal.jsonl"));
    reloaded.load().await.expect("journal reloads");
    assert_eq!(reloaded.blocked_stashes()[0].4.as_deref(), Some("thread-1"));
    assert_eq!(
        reloaded.blocked_stashes()[0].5.as_deref(),
        Some("fingerprint-1")
    );
}

/// The blocked-node twin of [`a_blocked_stash_rehydrates_its_checkpoint_lineage`]:
/// a restart must rehydrate the park-time fingerprint too, on the same
/// terms as the checkpoint thread id beside it — `spawn_blocked_node_continuation`
/// reads both off `StashedBlock` to decide whether a checkpoint resume is
/// safe (PR #1991 review, `3904397452`/`3904304754`).
#[tokio::test]
async fn a_pre_fingerprint_blocked_node_stashed_line_replays_with_no_fingerprint() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let legacy = serde_json::json!({
        "record": "BlockedNodeStashed",
        "turn": "turn-legacy",
        "workflow_id": "wf-legacy",
        "input": { "request": "before the fingerprint field" },
        "thread_id": "thread-legacy",
        "at_millis": 1_000,
    });
    tokio::fs::write(&path, format!("{legacy}\n"))
        .await
        .unwrap();

    let journal = RuntimeJournal::new(&path);
    journal
        .load()
        .await
        .expect("a line with no workflow_fingerprint field still replays");

    let stashes = journal.blocked_stashes();
    assert_eq!(stashes.len(), 1);
    assert_eq!(stashes[0].4.as_deref(), Some("thread-legacy"));
    assert!(
        stashes[0].5.is_none(),
        "a legacy line with no workflow_fingerprint field degrades to None, not a load \
         failure"
    );
}
