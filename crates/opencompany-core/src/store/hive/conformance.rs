//! The [`HiveStore`] conformance suite every backend runs.
//!
//! Parameterized over `Arc<dyn HiveStore>` and given a fresh, empty store per
//! call, exactly like the rest of [`crate::store::conformance`]. What a crash
//! leaves behind is backend-specific (a file tail, a row, a document), so the
//! crash-recovery cases live beside each backend; [`assert_orphans_ignored`]
//! is the half of them that is the same everywhere once the orphan exists.
//!
//! Fixtures carry nested, non-ASCII bodies on purpose: a backend that dropped
//! or re-encoded a field would otherwise pass on `{}`.

use std::sync::Arc;

use serde_json::json;

use crate::ports::hive::{HiveCommit, HiveMessageRow, HiveStateDoc, HiveStore};
use crate::ports::types::CompanyId;

fn state(revision: &str, next_sequence: u64) -> HiveStateDoc {
    HiveStateDoc {
        revision: revision.into(),
        next_sequence,
        body: json!({
            "watermark": next_sequence,
            "desks": {"engineering": {"present": [1, 2, 3]}},
            "note": "état — 状態",
        }),
    }
}

fn row(sequence: u64) -> HiveMessageRow {
    HiveMessageRow {
        sequence,
        body: json!({"text": format!("message {sequence} ✓"), "author": {"agent": "ceo"}}),
    }
}

fn rows(sequences: std::ops::Range<u64>) -> Vec<HiveMessageRow> {
    sequences.map(row).collect()
}

async fn commit(
    hive: &Arc<dyn HiveStore>,
    company: &CompanyId,
    expected: Option<&str>,
    next: HiveStateDoc,
    appended: Vec<HiveMessageRow>,
) -> HiveCommit {
    hive.commit_hive(company, expected, next, appended)
        .await
        .expect("commit is well formed")
}

/// Load, compare-and-swap, append, bound, isolation, refusal and purge.
pub async fn assert_hive_store(hive: Arc<dyn HiveStore>) {
    let alpha = CompanyId::new("alpha");
    let beta = CompanyId::new("beta");

    assert_eq!(hive.load_hive(&alpha, None).await.unwrap(), None);

    // The first commit creates the document.
    assert_eq!(
        commit(&hive, &alpha, None, state("r1", 3), rows(0..3)).await,
        HiveCommit::Committed
    );
    let loaded = hive.load_hive(&alpha, None).await.unwrap().expect("state");
    assert_eq!(loaded.state, state("r1", 3));
    assert_eq!(loaded.messages, rows(0..3));

    // Creating it again conflicts with what is stored, and writes nothing.
    assert_eq!(
        commit(&hive, &alpha, None, state("r9", 4), rows(3..4)).await,
        HiveCommit::Conflict {
            current: Some("r1".into())
        }
    );
    // So does a stale revision.
    assert_eq!(
        commit(&hive, &alpha, Some("r0"), state("r9", 4), rows(3..4)).await,
        HiveCommit::Conflict {
            current: Some("r1".into())
        }
    );
    let unchanged = hive.load_hive(&alpha, None).await.unwrap().unwrap();
    assert_eq!(unchanged.state, state("r1", 3));
    assert_eq!(unchanged.messages, rows(0..3), "a conflict appended rows");

    // The matching revision appends after what is there.
    assert_eq!(
        commit(&hive, &alpha, Some("r1"), state("r2", 5), rows(3..5)).await,
        HiveCommit::Committed
    );
    // A state-only commit moves the document and leaves the log alone.
    assert_eq!(
        commit(&hive, &alpha, Some("r2"), state("r3", 5), Vec::new()).await,
        HiveCommit::Committed
    );
    let loaded = hive.load_hive(&alpha, None).await.unwrap().unwrap();
    assert_eq!(loaded.state, state("r3", 5));
    assert_eq!(loaded.messages, rows(0..5));

    // `before` bounds the rows, never the document.
    let bounded = hive.load_hive(&alpha, Some(2)).await.unwrap().unwrap();
    assert_eq!(bounded.state, state("r3", 5));
    assert_eq!(bounded.messages, rows(0..2));
    let past_end = hive.load_hive(&alpha, Some(99)).await.unwrap().unwrap();
    assert_eq!(past_end.messages, rows(0..5));
    let none = hive.load_hive(&alpha, Some(0)).await.unwrap().unwrap();
    assert!(none.messages.is_empty());

    // A gap in the sequence is the caller's business, not a refusal.
    assert_eq!(
        commit(
            &hive,
            &alpha,
            Some("r3"),
            state("r4", 9),
            vec![row(6), row(8)]
        )
        .await,
        HiveCommit::Committed
    );
    let gapped = hive.load_hive(&alpha, None).await.unwrap().unwrap();
    let sequences: Vec<u64> = gapped.messages.iter().map(|m| m.sequence).collect();
    assert_eq!(sequences, vec![0, 1, 2, 3, 4, 6, 8]);

    // Malformed commits are errors and write nothing.
    for (next, appended) in [
        (state("r5", 10), vec![row(4)]),  // rewrites a committed row
        (state("r5", 10), vec![row(10)]), // beyond the new next_sequence
        (state("r5", 8), Vec::new()),     // next_sequence goes backwards
        (state("r4", 10), Vec::new()),    // revision unchanged
    ] {
        assert!(
            hive.commit_hive(&alpha, Some("r4"), next, appended)
                .await
                .is_err(),
            "a malformed commit was accepted"
        );
    }
    let after_refusals = hive.load_hive(&alpha, None).await.unwrap().unwrap();
    assert_eq!(after_refusals, gapped);

    // Isolation: beta sees nothing of alpha, and its own first commit stands
    // alone.
    assert_eq!(hive.load_hive(&beta, None).await.unwrap(), None);
    assert_eq!(
        commit(&hive, &beta, None, state("b1", 1), rows(0..1)).await,
        HiveCommit::Committed
    );
    assert_eq!(
        hive.load_hive(&alpha, None).await.unwrap().unwrap(),
        gapped,
        "beta's commit touched alpha"
    );

    // Purge removes alpha whole and leaves beta.
    hive.purge_hive(&alpha).await.unwrap();
    assert_eq!(hive.load_hive(&alpha, None).await.unwrap(), None);
    hive.purge_hive(&alpha).await.unwrap();
    assert_eq!(
        hive.load_hive(&beta, None).await.unwrap().unwrap().messages,
        rows(0..1)
    );

    // A recreated company starts clean: no row from before the purge returns.
    assert_eq!(
        commit(&hive, &alpha, None, state("n1", 1), vec![row(0)]).await,
        HiveCommit::Committed
    );
    let fresh = hive.load_hive(&alpha, None).await.unwrap().unwrap();
    assert_eq!(fresh.messages, vec![row(0)]);
    assert_eq!(fresh.state, state("n1", 1));
}

/// Concurrent writers racing one revision: exactly one commits, every other
/// one conflicts, and the log holds the winner's rows and nobody else's.
///
/// The bodies differ per writer, so a loser whose rows landed over the
/// winner's — the failure a store that writes rows before checking the swap
/// invites — reads back as the wrong text rather than passing.
pub async fn assert_hive_commit_race(hive: Arc<dyn HiveStore>) {
    let company = CompanyId::new("racing");
    commit(&hive, &company, None, state("base", 2), rows(0..2)).await;

    const WRITERS: usize = 8;
    let mut handles = Vec::new();
    for writer in 0..WRITERS {
        let hive = hive.clone();
        let company = company.clone();
        handles.push(tokio::spawn(async move {
            let appended = (2..5)
                .map(|sequence| HiveMessageRow {
                    sequence,
                    body: json!({"writer": writer, "sequence": sequence}),
                })
                .collect();
            let outcome = hive
                .commit_hive(
                    &company,
                    Some("base"),
                    state(&format!("w{writer}"), 5),
                    appended,
                )
                .await
                .expect("commit is well formed");
            (writer, outcome)
        }));
    }
    let mut winners = Vec::new();
    for handle in handles {
        let (writer, outcome) = handle.await.expect("writer task");
        match outcome {
            HiveCommit::Committed => winners.push(writer),
            HiveCommit::Conflict { current } => {
                assert!(
                    current.is_some(),
                    "a conflict must name the stored revision"
                );
            }
        }
    }
    assert_eq!(winners.len(), 1, "exactly one writer commits: {winners:?}");
    let winner = winners[0];

    let loaded = hive.load_hive(&company, None).await.unwrap().unwrap();
    assert_eq!(loaded.state.revision, format!("w{winner}"));
    assert_eq!(loaded.messages[..2], rows(0..2)[..]);
    for message in &loaded.messages[2..] {
        assert_eq!(
            message.body["writer"],
            json!(winner),
            "row {} holds a losing writer's body",
            message.sequence
        );
    }
    assert_eq!(loaded.messages.len(), 5);
}

/// The backend-neutral half of crash recovery, given a company whose backend
/// already holds uncommitted rows at sequences `3..6` above a committed
/// document `("r1", 3)` with rows `0..3`, as [`seed_committed`] writes it.
///
/// Those rows must never load, and the next commit must take their sequences
/// as if they were empty.
pub async fn assert_orphans_ignored(hive: Arc<dyn HiveStore>, company: &CompanyId) {
    let loaded = hive.load_hive(company, None).await.unwrap().unwrap();
    assert_eq!(loaded.state, state("r1", 3));
    assert_eq!(loaded.messages, rows(0..3), "an uncommitted row was loaded");
    let bounded = hive.load_hive(company, Some(99)).await.unwrap().unwrap();
    assert_eq!(
        bounded.messages,
        rows(0..3),
        "`before` reached past the commit"
    );

    let replacement: Vec<HiveMessageRow> = (3..5)
        .map(|sequence| HiveMessageRow {
            sequence,
            body: json!({"replacement": sequence}),
        })
        .collect();
    assert_eq!(
        commit(
            &hive,
            company,
            Some("r1"),
            state("r2", 5),
            replacement.clone()
        )
        .await,
        HiveCommit::Committed
    );
    let reloaded = hive.load_hive(company, None).await.unwrap().unwrap();
    let mut expected = rows(0..3);
    expected.extend(replacement);
    assert_eq!(
        reloaded.messages, expected,
        "an orphan survived the next commit"
    );
}

/// Writes the committed half of the fixture [`assert_orphans_ignored`] expects:
/// document `("r1", 3)` over rows `0..3`. The caller then plants orphans at
/// `3..6` through the backend's own storage.
pub async fn seed_committed(hive: &Arc<dyn HiveStore>, company: &CompanyId) {
    commit(hive, company, None, state("r1", 3), rows(0..3)).await;
}

/// The body an orphan row carries, so a backend test plants the same shape.
pub fn orphan_row(sequence: u64) -> HiveMessageRow {
    HiveMessageRow {
        sequence,
        body: json!({"orphan": sequence}),
    }
}
