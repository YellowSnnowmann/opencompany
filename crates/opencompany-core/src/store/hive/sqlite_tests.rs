//! [`SqliteStore`] as a [`HiveStore`]: the shared suite, plus rows a commit
//! never covered.

use std::sync::Arc;

use rusqlite::params;

use super::*;
use crate::store::hive::conformance;

fn store() -> Arc<SqliteStore> {
    Arc::new(SqliteStore::open_in_memory().expect("open in-memory sqlite"))
}

#[tokio::test]
async fn conformance_hive_store() {
    conformance::assert_hive_store(store()).await;
}

#[tokio::test]
async fn conformance_hive_commit_race() {
    conformance::assert_hive_commit_race(store()).await;
}

/// The transaction makes a half-written commit impossible here, so the
/// orphans are planted by hand: rows above `next_sequence` that something
/// other than a landed commit put in the table must still never load, and the
/// next commit must replace them.
#[tokio::test]
async fn rows_above_next_sequence_are_ignored_and_overwritten() {
    let sqlite = store();
    let hive: Arc<dyn HiveStore> = sqlite.clone();
    let company = CompanyId::new("acme");
    conformance::seed_committed(&hive, &company).await;
    {
        let conn = sqlite.conn();
        for sequence in 3..6 {
            let body = serde_json::to_string(&conformance::orphan_row(sequence).body).unwrap();
            conn.execute(
                "INSERT INTO hive_messages (company_id, sequence, body_json) VALUES (?1, ?2, ?3)",
                params![company.as_ref(), sequence as i64, body],
            )
            .unwrap();
        }
    }
    conformance::assert_orphans_ignored(hive, &company).await;
}

/// A file-backed database reopened: the tables are created by the ordinary
/// migrations, so a store that predates them gains them on open.
#[tokio::test]
async fn the_hive_survives_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencompany.db");
    let company = CompanyId::new("acme");
    {
        let hive: Arc<dyn HiveStore> = Arc::new(SqliteStore::open(&path).unwrap());
        conformance::seed_committed(&hive, &company).await;
    }
    let hive: Arc<dyn HiveStore> = Arc::new(SqliteStore::open(&path).unwrap());
    let loaded = hive.load_hive(&company, None).await.unwrap().unwrap();
    assert_eq!(loaded.state.revision, "r1");
    assert_eq!(loaded.messages.len(), 3);
}
