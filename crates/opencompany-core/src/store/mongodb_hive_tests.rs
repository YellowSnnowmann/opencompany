//! [`MongoStore`] as a [`HiveStore`]: the shared suite, plus rows a commit
//! never covered. Env-gated on a live server like the rest of the MongoDB
//! suite (`OPENCOMPANY_TEST_MONGODB_URI`).

use std::sync::Arc;

use super::tests::{drop_db, store};
use super::*;
use crate::ports::hive::{HiveCommit, HiveStateDoc, HiveStore};
use crate::store::hive::conformance;

#[tokio::test]
async fn conformance_hive_store() {
    let Some(s) = store().await else { return };
    conformance::assert_hive_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_hive_commit_race() {
    let Some(s) = store().await else { return };
    conformance::assert_hive_commit_race(s.clone()).await;
    drop_db(&s).await;
}

/// Rows already in `hive_messages` above the committed `next_sequence` —
/// what a recreated company's predecessor or a stray writer leaves — never
/// load, and the commit that claims their sequences replaces them.
#[tokio::test]
async fn rows_above_next_sequence_are_ignored_and_overwritten() {
    let Some(s) = store().await else { return };
    let hive: Arc<dyn HiveStore> = s.clone();
    let company = CompanyId::new("acme");
    conformance::seed_committed(&hive, &company).await;
    for sequence in 3..6i64 {
        let body = serde_json::to_string(&conformance::orphan_row(sequence as u64).body).unwrap();
        s.collection(crate::store::hive::HIVE_MESSAGES)
            .insert_one(doc! {
                "_id": {"c": company.as_ref(), "s": sequence},
                "company_id": company.as_ref(),
                "sequence": sequence,
                "body_json": body,
            })
            .await
            .unwrap();
    }
    conformance::assert_orphans_ignored(hive, &company).await;
    drop_db(&s).await;
}

/// A commit that crashed after its swap and before its rows reached
/// `hive_messages`: the rows exist only in the document's `pending`. They
/// load, and the next commit writes them out before it swaps.
#[tokio::test]
async fn rows_only_in_pending_load_and_are_materialized_by_the_next_commit() {
    let Some(s) = store().await else { return };
    let hive: Arc<dyn HiveStore> = s.clone();
    let company = CompanyId::new("acme");
    conformance::seed_committed(&hive, &company).await;
    // Simulate the crash: the swap's rows never left `pending`.
    s.collection(crate::store::hive::HIVE_MESSAGES)
        .delete_many(doc! {"company_id": company.as_ref()})
        .await
        .unwrap();

    let loaded = hive.load_hive(&company, None).await.unwrap().unwrap();
    assert_eq!(loaded.messages.len(), 3, "pending rows must load");

    let next = HiveStateDoc {
        revision: "r2".into(),
        next_sequence: 3,
        body: serde_json::json!({}),
    };
    assert_eq!(
        hive.commit_hive(&company, Some("r1"), next, Vec::new())
            .await
            .unwrap(),
        HiveCommit::Committed
    );
    let materialized = s
        .collection(crate::store::hive::HIVE_MESSAGES)
        .count_documents(doc! {"company_id": company.as_ref()})
        .await
        .unwrap();
    assert_eq!(
        materialized, 3,
        "the previous commit's rows were not written out"
    );
    let reloaded = hive.load_hive(&company, None).await.unwrap().unwrap();
    assert_eq!(reloaded.messages, loaded.messages);
    drop_db(&s).await;
}
