use super::tests_core::*;

#[tokio::test]
async fn effect_key_commits_once_and_survives_reload() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    assert!(!journal.is_executed("cyc:0"));
    journal.record_executed("cyc:0", executed(0)).await.unwrap();
    assert!(journal.is_executed("cyc:0"));
    // Re-committing the same key does not append a second record.
    journal.record_executed("cyc:0", executed(0)).await.unwrap();

    // A fresh journal over the same file (a restart) replays the commit.
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert!(reloaded.is_executed("cyc:0"));

    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(raw.lines().filter(|l| !l.trim().is_empty()).count(), 1);

    // The re-commit is also what keeps the description list free of
    // duplicates: one key, one entry, however many times it is committed.
    assert_eq!(reloaded.irreversible_effects("t-1").len(), 1);
}

/// Issue #2037, the second half of it: what the gate calls irreversible is
/// the only thing that ever reaches the retry warning.
///
/// [`State::index_executed`] files nothing for a reversible effect, so a
/// gate that waves a paid engagement through does not merely skip the
/// approval — it erases the engagement from the card's history, and Retry
/// re-runs it with no warning at all. The two halves are one bug and this
/// pins the join.
///
/// The flag is **derived from a real gate** rather than hand-stamped
/// `irreversible: true` like [`executed`] does. A hand-stamped fixture
/// tests the index and stays green through exactly the regression this is
/// here for: the shape under test is an engagement of an established
/// counterparty, with no amount stated, in a company that configured no
/// cap — the shape that used to read as "under the cap".
#[tokio::test]
async fn an_engagement_the_gate_calls_irreversible_reaches_the_retry_warning() {
    use crate::company::Policy;
    use crate::policy::ManifestApprovalGate;

    let gate = ManifestApprovalGate::new(Policy {
        mode: "supervised".into(),
        always_approve: Vec::new(),
        auto_approve_under_usd: None,
        approval_ttl_hours: None,
    });

    let engagement = Effect {
        kind: "a2a.engage".into(),
        group: EffectGroup::Hire,
        ..effect()
    };
    assert!(
        gate.is_irreversible(&engagement),
        "an engagement of unstated value in a company with no cap is not \
         something a person can take back",
    );

    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);
    journal
        .record_executed(
            "cyc:0",
            ExecutedEffect {
                kind: engagement.kind.clone(),
                amount_usd: engagement.amount_usd,
                task_id: Some("t-1".into()),
                at_millis: now_millis(),
                irreversible: gate.is_irreversible(&engagement),
            },
        )
        .await
        .unwrap();

    let named = journal.irreversible_effects("t-1");
    assert_eq!(
        named.len(),
        1,
        "the card's retry dialog has nothing to warn about",
    );
    assert_eq!(named[0].kind, "a2a.engage");

    // And it survives a restart, which is where the dialog usually reads it.
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert_eq!(reloaded.irreversible_effects("t-1").len(), 1);
}

/// **Issue #726**: a journal over a non-filesystem store replays exactly
/// what the same records replay over a file — and survives the loss of the
/// bundle directory, which is the whole point.
///
/// Every semantic decision (the record enum, replay, the parked queue, the
/// grant sets) lives above the store, so this is what proves the split is
/// real rather than merely stated: the same call sequence through two
/// different sinks must produce identical state, and the sink that is not a
/// file must still hold it after `/data` is gone.
#[tokio::test]
async fn a_journal_over_a_non_filesystem_store_replays_identically() {
    use crate::ports::journal::MemoryJournalStore;

    let company = CompanyId::new("acme");
    let store = Arc::new(MemoryJournalStore::default());

    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");

    // The same call sequence through both sinks: a committed key, a parked
    // approval, a minted grant, a resolution.
    let approval = ApprovalId::new("ap-1");
    for journal in [
        RuntimeJournal::with_store(store.clone(), company.clone()),
        RuntimeJournal::new(&path),
    ] {
        journal
            .record_executed("cyc:0", executed(1_000))
            .await
            .unwrap();
        journal
            .record_parked(
                &approval,
                &effect(),
                2_000,
                TaskLink::Task { id: "t-1".into() },
                ApprovalConversation::default(),
                None,
            )
            .await
            .unwrap();
        journal.record_granted(&grant("g-1", 3_000)).await.unwrap();
    }

    // The bundle directory is gone — a container replacement on a tenant
    // whose `/data` is ephemeral scratch. The file-backed journal loses
    // everything; the ported one loses nothing.
    drop(dir);

    let over_store = RuntimeJournal::with_store(store.clone(), company.clone());
    over_store.load().await.unwrap();
    let over_file = RuntimeJournal::new(&path);
    over_file.load().await.unwrap();

    assert!(
        over_store.is_executed("cyc:0"),
        "the at-most-once key must survive the loss of the data dir"
    );
    assert!(
        !over_file.is_executed("cyc:0"),
        "the file-backed journal is exactly what this issue is about: \
         losing /data un-commits every key"
    );
    assert_eq!(
        over_store.pending().len(),
        1,
        "the parked approval must survive too"
    );
    assert_eq!(over_store.pending()[0].id, approval, "with its original id");
    assert_eq!(
        over_store.replayed_grants().len(),
        1,
        "and so must a live grant"
    );
    assert!(over_store.corruption().is_empty());

    // Isolation: another company on the same store sees none of it.
    let other = RuntimeJournal::with_store(store, CompanyId::new("globex"));
    other.load().await.unwrap();
    assert!(!other.is_executed("cyc:0"));
    assert!(other.pending().is_empty());
}

/// **Issue #351**: the executed record says what ran, for which card, and
/// whether it can be taken back — and survives a restart.
#[tokio::test]
async fn executed_effects_are_filtered_by_task_and_by_irreversibility() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    // This card's irreversible effect — the one a retry must name.
    journal
        .record_executed("cyc:0", executed(1_000))
        .await
        .unwrap();
    // The same card, but a read: it changed nothing, so it warns about
    // nothing.
    journal
        .record_executed(
            "cyc:1",
            ExecutedEffect {
                kind: "web.search".into(),
                irreversible: false,
                ..executed(1_100)
            },
        )
        .await
        .unwrap();
    // Another card's payment. Irreversible, and none of this card's
    // business.
    journal
        .record_executed(
            "cyc:2",
            ExecutedEffect {
                kind: "payment.send".into(),
                amount_usd: Some(2_400.0),
                task_id: Some("t-2".into()),
                ..executed(1_200)
            },
        )
        .await
        .unwrap();
    // A workflow delivery: no card behind it at all.
    journal
        .record_executed(
            "cyc:3",
            ExecutedEffect {
                task_id: None,
                ..executed(1_300)
            },
        )
        .await
        .unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();

    let mine = reloaded.irreversible_effects("t-1");
    assert_eq!(mine.len(), 1, "{mine:?}");
    assert_eq!(mine[0].kind, "filing.submit");
    assert_eq!(mine[0].at_millis, 1_000);

    let theirs = reloaded.irreversible_effects("t-2");
    assert_eq!(theirs.len(), 1);
    assert_eq!(theirs[0].amount_usd, Some(2_400.0));

    assert!(reloaded.irreversible_effects("t-never-ran").is_empty());
}

/// A journal line written before #351 carries a key and nothing else. It
/// must still replay as an executed key — the at-most-once guarantee is not
/// negotiable — and simply contribute no description.
#[tokio::test]
async fn a_pre_351_executed_line_still_replays_as_a_committed_key() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    tokio::fs::write(
        &path,
        "{\"record\":\"EffectExecuted\",\"key\":\"cyc-old:0\"}\n",
    )
    .await
    .unwrap();

    let journal = RuntimeJournal::new(&path);
    journal.load().await.expect("a pre-#351 line still replays");
    assert!(
        journal.is_executed("cyc-old:0"),
        "dropping the key would re-run an effect that already fired",
    );
    assert!(journal.irreversible_effects("t-1").is_empty());
    assert!(
        journal.has_undescribed_history(),
        "an empty list here is 'cannot say', not 'nothing happened'",
    );
}

/// The companion assertion: a journal whose every executed line carries a
/// description reports no gap, so an empty list stays a genuine all-clear
/// and Retry stays one click.
#[tokio::test]
async fn a_fully_described_journal_reports_no_undescribed_history() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);
    journal.record_executed("cyc:0", executed(0)).await.unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert!(!reloaded.has_undescribed_history());
}

/// **Issue #351**: an operator-approved *tool call* is settled by minting a
/// grant, never by `execute_effect_once`, so redeeming that grant is the
/// only line in the journal that can say the call fired. It must reach the
/// same per-task read the native path does, and survive a restart.
#[tokio::test]
async fn a_redeemed_grant_names_what_it_did_against_its_card() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);
    let id = ApprovalId::new("appr-tool");

    journal
        .record_parked(
            &id,
            &effect(),
            1_000,
            TaskLink::Task { id: "t-1".into() },
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    journal.record_resolved(&id).await.unwrap();
    journal
        .record_grant_consumed(
            &id,
            Some(ExecutedEffect {
                kind: "composio_execute".into(),
                amount_usd: Some(2_400.0),
                ..executed(1_200)
            }),
        )
        .await
        .unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    let named = reloaded.irreversible_effects("t-1");
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(named[0].kind, "composio_execute");
    assert_eq!(named[0].amount_usd, Some(2_400.0));
    assert!(
        reloaded.replayed_grants().is_empty(),
        "describing the redemption must not re-arm it",
    );
}

/// A redemption the runtime could not describe still journals, and simply
/// contributes no warning — the same additive degradation a pre-#351
/// `EffectExecuted` line has.
#[tokio::test]
async fn an_undescribed_redemption_still_journals_and_warns_about_nothing() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);
    journal
        .record_grant_consumed(&ApprovalId::new("appr-old"), None)
        .await
        .unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert!(reloaded.irreversible_effects("t-1").is_empty());
}

/// Issue #351: the description a redeemed grant is built from comes off the
/// park record, retained past resolution and **scrubbed of its payload** —
/// this map outlives the queue entry, and the retry read never wants a
/// recipient or a body.
#[tokio::test]
async fn an_approvals_effect_outlives_it_without_its_payload() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);
    let id = ApprovalId::new("appr-1");
    let parked = Effect {
        payload: serde_json::json!({ "to": "someone@example.com", "body": "secret" }),
        ..effect()
    };

    journal
        .record_parked(
            &id,
            &parked,
            1_000,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    journal.record_resolved(&id).await.unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();

    // Live and replayed must agree: a grant can be redeemed either side of
    // a restart.
    for from in [&journal, &reloaded] {
        let kept = from.approval_effect(&id).expect("retained past resolve");
        assert_eq!(kept.kind, "filing.submit");
        assert_eq!(kept.group, EffectGroup::Sign);
        assert_eq!(
            kept.payload,
            serde_json::Value::Null,
            "the payload must not be retained past the queue entry",
        );
    }
    assert_eq!(journal.approval_effect(&ApprovalId::new("never")), None);
}

/// An approve-with-edit supersedes the park: the grant is minted against the
/// amended arguments, so the amended amount is the one to report.
#[tokio::test]
async fn an_amendment_supersedes_the_parked_effect_as_the_description() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);
    let id = ApprovalId::new("appr-1");

    journal
        .record_parked(
            &id,
            &Effect {
                amount_usd: Some(2_400.0),
                ..effect()
            },
            1_000,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    journal
        .record_amended(
            &id,
            &Effect {
                amount_usd: Some(400.0),
                ..effect()
            },
            1_100,
        )
        .await
        .unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert_eq!(
        reloaded.approval_effect(&id).and_then(|e| e.amount_usd),
        Some(400.0),
        "reporting the pre-edit amount would name a payment nobody approved",
    );
}

#[tokio::test]
async fn parked_approvals_rehydrate_and_resolve() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);
    let id = ApprovalId::new("appr-1");
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

    // Reload from disk: the parked approval comes back.
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert_eq!(reloaded.pending().len(), 1);
    assert_eq!(reloaded.pending()[0].id, id);

    // Resolving removes it, and the removal is durable.
    reloaded.record_resolved(&id).await.unwrap();
    assert!(reloaded.pending().is_empty());

    let after = RuntimeJournal::new(&path);
    after.load().await.unwrap();
    assert!(after.pending().is_empty());
}

/// **Issue #333**: the board task an approval was parked for is carried on
/// the record, survives a restart, and outlives the resolution.
///
/// The whole point of the field is the *resolved* case — a task's Approvals
/// tab has to say which sign-offs were its own long after they left the
/// queue — so the origin assertion after `record_resolved` is the one that
/// matters, not the pending one before it.
#[tokio::test]
async fn a_parked_approval_carries_its_task_across_a_restart_and_a_resolution() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    let mine = ApprovalId::new("appr-mine");
    let theirs = ApprovalId::new("appr-theirs");
    let orphan = ApprovalId::new("appr-orphan");
    journal
        .record_parked(
            &mine,
            &effect(),
            1_000,
            TaskLink::Task { id: "t-1".into() },
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    journal
        .record_parked(
            &theirs,
            &effect(),
            1_100,
            TaskLink::Task { id: "t-2".into() },
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    // No card behind it (a workflow delivery, an operator-chat turn).
    journal
        .record_parked(
            &orphan,
            &effect(),
            1_200,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    let pending = reloaded.pending();
    assert_eq!(
        pending
            .iter()
            .filter(|p| p.task.as_ref().and_then(TaskLink::task_id) == Some("t-1"))
            .count(),
        1,
        "the parked queue must name the task, not just the effect",
    );

    // The resolution drains the queue but must not drain the link.
    reloaded.record_resolved(&mine).await.unwrap();
    assert!(reloaded.pending().iter().all(|p| p.id != mine));
    let origins = reloaded.approval_origins();
    assert_eq!(
        origins.get(&mine),
        Some(&ApprovalOrigin {
            at_millis: 1_000,
            kind: "filing.submit".into(),
            task: Some(TaskLink::Task { id: "t-1".into() }),
            run_id: None,
            thread: None,
            parent: None,
            cycle: None,
        }),
    );
    assert_eq!(
        origins.get(&theirs).and_then(|o| o.task.clone()),
        Some(TaskLink::Task { id: "t-2".into() }),
        "a second task's approval keeps its own id, so neither absorbs the other",
    );
    // Recorded as deliberately unlinked — *not* as a missing link, which is
    // what tells the read side never to fall back to the run window for it.
    assert_eq!(
        origins.get(&orphan).and_then(|o| o.task.clone()),
        Some(TaskLink::Unlinked),
    );
}

/// A journal line written before #333 has no `task` key at all. It must
/// replay with **no link** rather than failing to parse — and that absence
/// is what the read side falls back to the old run-window correlation for.
///
/// The distinction this pins is the one the whole feature rests on: a
/// missing key replays as `None`, while a park this host recorded as having
/// no card behind it replays as `Some(Unlinked)`. Both are "no task id",
/// and they must not be confused.
#[tokio::test]
async fn a_pre_333_parked_line_replays_with_no_task() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let legacy = serde_json::json!({
        "record": "ApprovalParked",
        "id": "appr-legacy",
        "effect": effect(),
        "at_millis": 4_000,
    });
    tokio::fs::write(&path, format!("{legacy}\n"))
        .await
        .unwrap();

    let journal = RuntimeJournal::new(&path);
    journal.load().await.expect("a pre-#333 line still replays");
    let id = ApprovalId::new("appr-legacy");
    assert_eq!(journal.pending().len(), 1);
    assert_eq!(journal.pending()[0].task, None, "no key means no link");
    assert_eq!(
        journal.approval_origins().get(&id).map(|o| o.at_millis),
        Some(4_000),
    );
    assert_eq!(journal.approval_task(&id), Some(None));

    // A park this host records with no card behind it is a *different*
    // fact, written explicitly, and must not read back as the legacy shape.
    let fresh = ApprovalId::new("appr-new");
    journal
        .record_parked(
            &fresh,
            &effect(),
            5_000,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        journal.approval_task(&fresh),
        Some(Some(TaskLink::Unlinked))
    );

    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    let fresh_line = raw
        .lines()
        .find(|l| l.contains("appr-new"))
        .expect("the new park was appended");
    assert!(
        fresh_line.contains(r#""link":"unlinked""#),
        "an unlinked park must say so on disk: {fresh_line}",
    );
}
