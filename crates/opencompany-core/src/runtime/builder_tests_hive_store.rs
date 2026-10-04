//! Which [`HiveStore`](crate::ports::HiveStore) a built company reaches: the
//! filesystem default, a swapped one, and the one an opened backend hands over
//! through [`RuntimeBuilder::with_stores`].

use std::sync::Arc;

use super::tests_core::*;
use crate::ports::hive::{HiveCommit, HiveStateDoc, HiveStore};
use crate::store::{FsHiveStore, MemoryHiveStore, StorageHandles};

const MANIFEST: &str = "[company]\nname = \"Acme\"\n[[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n";

fn first_state() -> HiveStateDoc {
    HiveStateDoc {
        revision: "r1".into(),
        next_sequence: 0,
        body: serde_json::json!({"marker": "from the runtime"}),
    }
}

async fn commit_through(runtime: &CompanyRuntime) {
    let outcome = runtime
        .hive_store()
        .commit_hive(runtime.id(), None, first_state(), Vec::new())
        .await
        .expect("commit");
    assert_eq!(outcome, HiveCommit::Committed);
}

/// Every port the filesystem serves, with `hive` swapped for `hive`.
fn fs_handles(home: &std::path::Path, hive: Arc<dyn HiveStore>) -> StorageHandles {
    let ops = Arc::new(crate::store::FsOps::new(home));
    StorageHandles {
        company: Arc::new(crate::store::FsCompanyStore::new(home)),
        events: Arc::new(crate::store::FsEventLog::new(home)),
        traces: Arc::new(crate::store::FsTraceStore::new(home)),
        secrets: Arc::new(crate::store::FsSecretStore::new(home)),
        inbox: Arc::new(crate::store::FsInboxStore::new(home)),
        tasks: ops.clone(),
        ledgers: ops.clone(),
        workspace: ops.clone(),
        artifacts: ops.clone(),
        runs: ops.clone(),
        workflow_revisions: ops.clone(),
        schedule_fires: ops.clone(),
        run_outputs: ops.clone(),
        deep_trace: ops.clone(),
        usage: ops.clone(),
        skills: ops.clone(),
        read_state: ops.clone(),
        notifications: ops.clone(),
        users: ops.clone(),
        sessions: ops.clone(),
        login_codes: ops,
        journal: Arc::new(crate::store::FsJournalStore::new(home)),
        hive,
        ownership: None,
    }
}

#[tokio::test]
async fn an_unswapped_company_keeps_its_hive_in_its_bundle() {
    let dir = tmp_home("oc-hive-default-");
    let runtime = RuntimeBuilder::new(dir.path().to_path_buf(), parse(MANIFEST))
        .build()
        .await
        .unwrap();
    commit_through(&runtime).await;

    let on_disk = FsHiveStore::new(dir.path())
        .load_hive(runtime.id(), None)
        .await
        .unwrap()
        .expect("the default store is the bundle's hive/ directory");
    assert_eq!(on_disk.state, first_state());
}

#[tokio::test]
async fn a_swapped_hive_store_is_the_one_the_company_reaches() {
    let dir = tmp_home("oc-hive-swapped-");
    let memory = Arc::new(MemoryHiveStore::new());
    let runtime = RuntimeBuilder::new(dir.path().to_path_buf(), parse(MANIFEST))
        .with_hive_store(memory.clone())
        .build()
        .await
        .unwrap();
    commit_through(&runtime).await;

    assert!(memory.load_hive(runtime.id(), None).await.unwrap().is_some());
    assert!(
        FsHiveStore::new(dir.path())
            .load_hive(runtime.id(), None)
            .await
            .unwrap()
            .is_none(),
        "a swapped store must not also write the bundle"
    );
}

/// The rule `open_storage` states for every durable port: an opened backend's
/// store, never a silent filesystem fallback.
#[tokio::test]
async fn with_stores_hands_over_the_backends_hive_store() {
    let dir = tmp_home("oc-hive-handles-");
    let memory = Arc::new(MemoryHiveStore::new());
    let handles = fs_handles(dir.path(), memory.clone());
    let runtime = RuntimeBuilder::new(dir.path().to_path_buf(), parse(MANIFEST))
        .with_stores(&handles)
        .build()
        .await
        .unwrap();
    commit_through(&runtime).await;

    assert!(
        memory.load_hive(runtime.id(), None).await.unwrap().is_some(),
        "with_stores did not hand over the backend's hive store"
    );
}
