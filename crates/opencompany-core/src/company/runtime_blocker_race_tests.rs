//! Runtime tests: console/DM blocker verdict races and pruning a wholly-refused blocked node's checkpoint lineage.

use std::sync::Arc;

#[cfg(feature = "openhuman")]
use super::tests_core::{RacingJournalStore, RefusingJournalStore};

/// **Major review finding (CodeRabbit) on PR #2038.** The claim-release
/// fix above only covers `record_blocker_resolution`'s own failure.
/// `settle_approval` banks its own journal record right after
/// (`record_resolved`), and a volume that dies between the two fails
/// there instead — after the blocker's resolution is already durable,
/// but before the approval itself settles. That path returned via `?`
/// with the claim still taken.
///
/// This asserts the claim itself (`peek_blocker_resolution`) rather than
/// a full successful retry, because `record_resolved` (like
/// `resolve_outcome` on the gate) removes the approval from
/// `journal.pending()` *before* its own append can fail — so a same-
/// process retry hits `claim_and_settle_blocker`'s independent
/// `still_parked` guard and reports `AlreadyResolved` regardless of
/// whether the claim was released. Releasing it here is still owed: an
/// orphaned entry in `grants.blocker_resolutions` for an id no live
/// resume will ever consume is exactly the state
/// `take_blocker_resolution` exists to prevent.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_journal_failure_inside_settle_also_releases_the_blocker_claim() {
    let home = tempfile::tempdir().expect("home");
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [policy]
        mode = "supervised"
        "#,
    )
    .expect("manifest");
    let journal = std::sync::Arc::new(RefusingJournalStore::default());
    let runtime = std::sync::Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
            .with_id(crate::ports::types::CompanyId::new("acme"))
            .with_journal_store(journal.clone())
            .build()
            .await
            .expect("runtime"),
    );

    let payload = crate::ports::blockers::BlockerPayload {
        kind: crate::ports::blockers::BlockerKind::Infrastructure,
        source: crate::ports::blockers::BlockerSource::Provider,
        step: Some(crate::ports::blockers::BlockerStep::Task {
            task_id: "t-1".to_string(),
        }),
        reason: "the model `gpt-nonexistent` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    };
    let id = runtime
        .park_blocker(
            &payload,
            "t-1",
            crate::company::blocker_sender::BlockerSenderSignals::default(),
        )
        .await
        .expect("parks before the volume goes away");

    // The volume dies after exactly one more append lands: that append
    // is `record_blocker_resolution`, so the claim's own bank succeeds
    // and the very next journal write --- `settle_approval`'s
    // `record_resolved` --- is the one that fails.
    journal.arm();
    journal.allow_next(1);
    let failed = runtime
        .apply_blocker_reply_spawned(
            std::slice::from_ref(&id),
            &id,
            crate::ports::blockers::BlockerVerdict::Retry,
            "",
            None,
        )
        .await;
    assert!(
        failed.is_err(),
        "the armed journal store must fail settle_approval's own record and surface \
         the error: {failed:?}"
    );

    assert!(
        runtime.grants.peek_blocker_resolution(&id).is_none(),
        "a settle_approval failure after the claim was banked must release it too, \
         not just a record_blocker_resolution failure — otherwise \
         grants.blocker_resolutions keeps an orphaned entry for an id no live \
         resume will ever consume"
    );
}
/// **P1 review finding (Codex) on PR #2038.** Releasing the *live* claim
/// when `settle_approval` fails is only half the compensation: the durable
/// `BlockerResolved` record is already banked and survives. A boot
/// rehydrates it onto the grant set, the approval itself is still parked —
/// `record_resolved` never landed, so replay never saw it resolve — and
/// nothing drives the pair. Every later answer then loses
/// `claim_blocker_resolution` to the rehydrated entry and returns
/// `AlreadyResolved` without settling or resuming, so the blocker is
/// permanently unanswerable and stays that way across further restarts.
///
/// `claim_and_settle_blocker`'s own doc already promised the opposite —
/// "a crash between the two still replays as still armed and re-resumes" —
/// and no code made that true. This is that promise, asserted: after the
/// restart the blocker must actually leave the pending set rather than sit
/// banked forever.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_banked_blocker_whose_settle_failed_is_driven_on_the_next_boot() {
    let home = tempfile::tempdir().expect("home");
    let manifest = || {
        toml::from_str::<crate::company::CompanyManifest>(
            r#"
            [company]
            name = "Acme"

            [[agent]]
            id = "ceo"
            role = "Chief"

            [policy]
            mode = "supervised"
            "#,
        )
        .expect("manifest")
    };
    let journal = std::sync::Arc::new(RefusingJournalStore::default());
    let booted = std::sync::Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
            .with_id(crate::ports::types::CompanyId::new("acme"))
            .with_journal_store(journal.clone())
            .build()
            .await
            .expect("runtime"),
    );

    let payload = crate::ports::blockers::BlockerPayload {
        kind: crate::ports::blockers::BlockerKind::Infrastructure,
        source: crate::ports::blockers::BlockerSource::Provider,
        step: Some(crate::ports::blockers::BlockerStep::Task {
            task_id: "t-1".to_string(),
        }),
        reason: "the model `gpt-nonexistent` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    };
    let id = booted
        .park_blocker(
            &payload,
            "t-1",
            crate::company::blocker_sender::BlockerSenderSignals::default(),
        )
        .await
        .expect("parks");

    // The volume dies after exactly one more append: `record_blocker_resolution`
    // lands, so the answer is durable, and `settle_approval`'s own
    // `record_resolved` is the write that fails.
    journal.arm();
    journal.allow_next(1);
    assert!(
        booted
            .apply_blocker_reply_spawned(
                std::slice::from_ref(&id),
                &id,
                crate::ports::blockers::BlockerVerdict::Retry,
                "",
                None,
            )
            .await
            .is_err(),
        "the armed journal store must fail settle_approval's own record"
    );
    // The volume comes back, as it would have by the time anyone restarts.
    journal.disarm();
    drop(booted);

    // The next boot: same home, same journal, replayed from scratch.
    let rebooted = std::sync::Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest())
            .with_id(crate::ports::types::CompanyId::new("acme"))
            .with_journal_store(journal.clone())
            .build()
            .await
            .expect("runtime"),
    );
    assert!(
        rebooted.grants.peek_blocker_resolution(&id).is_some(),
        "the boot must rehydrate the banked answer — without that there is \
         nothing for this test to drive"
    );
    assert!(
        rebooted.journal.pending().iter().any(|p| p.id == id),
        "and the approval must still be parked, since record_resolved never landed"
    );

    rebooted.recover().await.expect("replay");

    // The resume runs on a spawned task, so give it room to land.
    let mut settled = false;
    for _ in 0..200 {
        if !rebooted.journal.pending().iter().any(|p| p.id == id) {
            settled = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        settled,
        "a banked-but-unsettled blocker answer must be driven on the next boot; it is \
         still parked with its resolution rehydrated, so claim_blocker_resolution will \
         refuse every later answer and this blocker can never be resolved by anyone"
    );
}
/// A blocker parked far enough in the past to be past any TTL, seeded the
/// way `seed_parked` does so the deadline is arbitrary rather than "now".
#[cfg(feature = "openhuman")]
async fn park_expired_blocker(
    runtime: &Arc<super::CompanyRuntime>,
    id: &str,
    payload: &crate::ports::blockers::BlockerPayload,
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
        .rehydrate(approval.clone(), effect.clone(), 0);
    runtime
        .journal
        .record_parked(
            &approval,
            &effect,
            0,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .expect("seed parked blocker");
    approval
}

#[cfg(feature = "openhuman")]
fn stuck_payload() -> crate::ports::blockers::BlockerPayload {
    crate::ports::blockers::BlockerPayload {
        kind: crate::ports::blockers::BlockerKind::Infrastructure,
        source: crate::ports::blockers::BlockerSource::Provider,
        step: Some(crate::ports::blockers::BlockerStep::Task {
            task_id: "t-1".to_string(),
        }),
        reason: "the model `gpt-nonexistent` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    }
}

#[cfg(feature = "openhuman")]
async fn blocker_runtime() -> (Arc<super::CompanyRuntime>, tempfile::TempDir) {
    let home = tempfile::tempdir().expect("home");
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [policy]
        mode = "supervised"
        "#,
    )
    .expect("manifest");
    let runtime = Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
            .with_id(crate::ports::types::CompanyId::new("acme"))
            .build()
            .await
            .expect("runtime"),
    );
    (runtime, home)
}

/// **Major review finding (CodeRabbit) on PR #2038.** The console fallback
/// claims the slot before it settles, and `settle_approval` can answer
/// `Expired` or `AlreadyResolved` rather than failing outright. Neither
/// receipt gets a resume — `spawn_follow_up` returns early for both — so
/// the claim it armed is left in `grants.blocker_resolutions` with nothing
/// that will ever consume it. The four-way path already compensates this;
/// the fallback did not.
///
/// An epoch-0 park is unambiguously past any TTL, which makes the
/// `Expired` arm reachable without racing a deadline.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_console_expiry_releases_the_blocker_claim_it_armed() {
    let (runtime, _home) = blocker_runtime().await;
    let payload = stuck_payload();
    let id = park_expired_blocker(&runtime, "appr-expired-blocker", &payload).await;

    let (receipt, _follow_up) = runtime
        .resolve_approval_spawned(
            &id,
            crate::ports::types::Verdict::Approve,
            crate::ports::types::Actor {
                kind: crate::ports::types::ActorKind::Operator,
                id: "owner".to_string(),
            },
            crate::runtime::grants::GrantScope::Once,
        )
        .await
        .expect("a late console click resolves as an expiry, not an error");
    assert!(
        receipt.expired(),
        "an epoch-0 park must read as expired: {receipt:?}"
    );

    assert!(
        runtime.grants.peek_blocker_resolution(&id).is_none(),
        "the console fallback armed a claim and then settled to a receipt with no \
         resume behind it; leaving the claim armed strands an entry no follow-up \
         will ever take, and blocks every later answer to the same id"
    );
}

/// The same finding's other half: a `settle_approval` that *fails* after the
/// fallback has claimed and banked must release the live claim too, exactly
/// as `settle_claimed_blocker` does for the four-way path.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_console_settle_failure_releases_the_blocker_claim_it_armed() {
    let home = tempfile::tempdir().expect("home");
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [policy]
        mode = "supervised"
        "#,
    )
    .expect("manifest");
    let journal = std::sync::Arc::new(RefusingJournalStore::default());
    let runtime = Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
            .with_id(crate::ports::types::CompanyId::new("acme"))
            .with_journal_store(journal.clone())
            .build()
            .await
            .expect("runtime"),
    );
    let payload = stuck_payload();
    let id = runtime
        .park_blocker(
            &payload,
            "t-1",
            crate::company::blocker_sender::BlockerSenderSignals::default(),
        )
        .await
        .expect("parks");

    // One more append lands — the fallback's own `record_blocker_resolution`
    // — and `settle_approval`'s `record_resolved` is the write that fails.
    journal.arm();
    journal.allow_next(1);
    assert!(
        runtime
            .resolve_approval_spawned(
                &id,
                crate::ports::types::Verdict::Approve,
                crate::ports::types::Actor {
                    kind: crate::ports::types::ActorKind::Operator,
                    id: "owner".to_string(),
                },
                crate::runtime::grants::GrantScope::Once,
            )
            .await
            .is_err(),
        "the armed journal store must fail settle_approval's own record"
    );

    assert!(
        runtime.grants.peek_blocker_resolution(&id).is_none(),
        "a settle failure after the console fallback banked its answer must release \
         the live claim, the same compensation claim_and_settle_blocker makes"
    );
}
/// **P1 review finding (Codex) on PR #2038.** The group lock and the
/// atomic claim covered the four-way path only. The console's still-supported
/// two-value fallback armed through `peek_blocker_resolution`, an awaited
/// journal write, and an unconditional `arm_blocker_resolution` insert — so a
/// legacy Approve/Deny could read the slot empty, suspend on its own journal
/// write while a four-way request claimed the blocker, and then overwrite the
/// winner's resolution on the way out. The approval event recorded one verdict
/// while the resume executed another.
///
/// Whoever wins `claim_blocker_resolution` owns the slot, so this asserts the
/// two agree rather than pinning a particular winner: the interleaved request
/// reports whether it took the slot, and the armed resolution must be that
/// caller's either way.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn the_console_fallback_never_overwrites_a_blocker_claim_it_lost() {
    let home = tempfile::tempdir().expect("home");
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [policy]
        mode = "supervised"
        "#,
    )
    .expect("manifest");
    let journal = std::sync::Arc::new(RacingJournalStore::default());
    let runtime = std::sync::Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
            .with_id(crate::ports::types::CompanyId::new("acme"))
            .with_journal_store(journal.clone())
            .build()
            .await
            .expect("runtime"),
    );

    let payload = crate::ports::blockers::BlockerPayload {
        kind: crate::ports::blockers::BlockerKind::Infrastructure,
        source: crate::ports::blockers::BlockerSource::Provider,
        step: Some(crate::ports::blockers::BlockerStep::Task {
            task_id: "t-1".to_string(),
        }),
        reason: "the model `gpt-nonexistent` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    };
    let id = runtime
        .park_blocker(
            &payload,
            "t-1",
            crate::company::blocker_sender::BlockerSenderSignals::default(),
        )
        .await
        .expect("parks");

    // The four-way request lands *inside* the console path's awaited journal
    // write — the exact window the peek-then-insert pair left open.
    let rival_won = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let grants = runtime.grants.clone();
        let id = id.clone();
        let rival_won = rival_won.clone();
        journal.interleave_next(move || {
            let rival = crate::ports::blockers::BlockerResolution {
                verdict: crate::ports::blockers::BlockerVerdict::Skip,
                answer: String::new(),
                step: Some(crate::ports::blockers::BlockerStep::Task {
                    task_id: "t-1".to_string(),
                }),
            };
            rival_won.store(
                grants.claim_blocker_resolution(&id, rival),
                std::sync::atomic::Ordering::SeqCst,
            );
        });
    }

    runtime
        .resolve_approval_spawned(
            &id,
            crate::ports::types::Verdict::Approve,
            crate::ports::types::Actor {
                kind: crate::ports::types::ActorKind::Operator,
                id: crate::runtime::channel::OPERATOR_CHANNEL.to_string(),
            },
            crate::runtime::grants::GrantScope::Once,
        )
        .await
        .expect("the console resolve lands");

    let armed = runtime
        .grants
        .peek_blocker_resolution(&id)
        .expect("a resolution stays armed for the resume to consume");
    let winner = rival_won.load(std::sync::atomic::Ordering::SeqCst);
    let expected = if winner {
        crate::ports::blockers::BlockerVerdict::Skip
    } else {
        crate::ports::blockers::BlockerVerdict::Retry
    };
    assert_eq!(
        armed.verdict, expected,
        "the caller that won claim_blocker_resolution must own the armed slot \
         (rival won the claim: {winner}); the console fallback overwrote a \
         resolution it did not claim, so the approval event and the resume \
         disagree about what the operator decided"
    );
}

/// A DM reply and a console verdict both resolve through
/// `claim_blocker_resolution`, so they cannot both win — but until now
/// nothing drove one of each at the same blocker and checked the loser's
/// **own return value**, only the armed slot's content (see the test
/// above). `resolve_approval_spawned` and `apply_blocker_reply_spawned`
/// both hold `self.blocker_resolutions` for their claim-and-settle window,
/// so true interleaving is impossible by construction; what remains
/// untested is that the second caller in, whichever surface it is, is
/// handed back `AlreadyResolved` rather than a receipt that reads like it
/// was the one that settled the blocker.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_console_verdict_after_a_dm_reply_already_won_is_told_it_lost() {
    let home = tempfile::tempdir().expect("home");
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [policy]
        mode = "supervised"
        "#,
    )
    .expect("manifest");
    let runtime = std::sync::Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
            .with_id(crate::ports::types::CompanyId::new("acme"))
            .build()
            .await
            .expect("runtime"),
    );

    let payload = crate::ports::blockers::BlockerPayload {
        kind: crate::ports::blockers::BlockerKind::Infrastructure,
        source: crate::ports::blockers::BlockerSource::Provider,
        step: Some(crate::ports::blockers::BlockerStep::Task {
            task_id: "t-1".to_string(),
        }),
        reason: "the model `gpt-nonexistent` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    };
    let id = runtime
        .park_blocker(
            &payload,
            "t-1",
            crate::company::blocker_sender::BlockerSenderSignals::default(),
        )
        .await
        .expect("parks");

    // The DM reply lands first and wins the claim.
    let (dm_receipt, dm_follow_up) = runtime
        .apply_blocker_reply_spawned(
            std::slice::from_ref(&id),
            &id,
            crate::ports::blockers::BlockerVerdict::Retry,
            "",
            None,
        )
        .await
        .expect("the dm reply claims and settles");
    assert!(
        matches!(
            dm_receipt,
            crate::runtime::cycle::ResolveReceipt::Settled(_)
        ),
        "the dm reply must be the one that settles the blocker: {dm_receipt:?}"
    );
    drop(dm_follow_up);

    // The console verdict arrives on the same id after the claim is
    // already taken. It must not error, and it must not be told it won.
    let (console_receipt, _console_follow_up) = runtime
        .resolve_approval_spawned(
            &id,
            crate::ports::types::Verdict::Deny,
            crate::ports::types::Actor {
                kind: crate::ports::types::ActorKind::Operator,
                id: crate::runtime::channel::OPERATOR_CHANNEL.to_string(),
            },
            crate::runtime::grants::GrantScope::Once,
        )
        .await
        .expect("a losing resolve is a receipt, not an error");
    assert!(
        matches!(
            console_receipt,
            crate::runtime::cycle::ResolveReceipt::AlreadyResolved
        ),
        "a console verdict racing a dm reply it lost must be reported to its own \
         caller as already-resolved, not silently accepted as though it settled \
         the blocker: {console_receipt:?}"
    );
}

/// The mirror of the test above: the console verdict wins the claim, and a
/// DM reply arriving after it on the same blocker must be told it lost
/// through its own return value rather than being silently accepted.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_dm_reply_after_a_console_verdict_already_won_is_told_it_lost() {
    let home = tempfile::tempdir().expect("home");
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
        [company]
        name = "Acme"

        [[agent]]
        id = "ceo"
        role = "Chief"

        [policy]
        mode = "supervised"
        "#,
    )
    .expect("manifest");
    let runtime = std::sync::Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
            .with_id(crate::ports::types::CompanyId::new("acme"))
            .build()
            .await
            .expect("runtime"),
    );

    let payload = crate::ports::blockers::BlockerPayload {
        kind: crate::ports::blockers::BlockerKind::Infrastructure,
        source: crate::ports::blockers::BlockerSource::Provider,
        step: Some(crate::ports::blockers::BlockerStep::Task {
            task_id: "t-1".to_string(),
        }),
        reason: "the model `gpt-nonexistent` was rejected".to_string(),
        needed: "a model id this provider serves".to_string(),
        group_key: None,
    };
    let id = runtime
        .park_blocker(
            &payload,
            "t-1",
            crate::company::blocker_sender::BlockerSenderSignals::default(),
        )
        .await
        .expect("parks");

    let (console_receipt, _console_follow_up) = runtime
        .resolve_approval_spawned(
            &id,
            crate::ports::types::Verdict::Approve,
            crate::ports::types::Actor {
                kind: crate::ports::types::ActorKind::Operator,
                id: crate::runtime::channel::OPERATOR_CHANNEL.to_string(),
            },
            crate::runtime::grants::GrantScope::Once,
        )
        .await
        .expect("the console verdict claims and settles");
    assert!(
        matches!(
            console_receipt,
            crate::runtime::cycle::ResolveReceipt::Settled(_)
        ),
        "the console verdict must be the one that settles the blocker: {console_receipt:?}"
    );

    let (dm_receipt, dm_follow_up) = runtime
        .apply_blocker_reply_spawned(
            std::slice::from_ref(&id),
            &id,
            crate::ports::blockers::BlockerVerdict::Retry,
            "",
            None,
        )
        .await
        .expect("a losing dm reply is a receipt, not an error");
    assert!(
        matches!(
            dm_receipt,
            crate::runtime::cycle::ResolveReceipt::AlreadyResolved
        ),
        "a dm reply racing a console verdict it lost must be reported to its own \
         caller as already-resolved, not silently accepted as though it settled \
         the blocker: {dm_receipt:?}"
    );
    drop(dm_follow_up);
}
