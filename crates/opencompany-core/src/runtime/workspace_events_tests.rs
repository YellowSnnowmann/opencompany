use super::*;
use crate::ports::types::{EventSeq, StoredEvent};
use crate::ports::workspace::{NodeKind, WorkspaceOrigin};
use crate::store::FsOps;
use futures::stream::BoxStream;
use std::sync::Mutex;

#[derive(Default)]
struct MemLog {
    appended: Mutex<Vec<CompanyEvent>>,
}

#[async_trait]
impl EventLog for MemLog {
    async fn append(&self, _id: &CompanyId, event: CompanyEvent) -> Result<EventSeq> {
        let mut got = self.appended.lock().unwrap();
        got.push(event);
        Ok(EventSeq::new(got.len() as u64))
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        _seq: EventSeq,
        _limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        Ok(Vec::new())
    }
    fn subscribe(
        &self,
        _id: &CompanyId,
    ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(futures::stream::empty())
    }
}

/// An event log that refuses every append, so the "record-keeping never
/// fails the work it records" contract is tested rather than asserted.
struct BrokenLog;

#[async_trait]
impl EventLog for BrokenLog {
    async fn append(&self, _id: &CompanyId, _event: CompanyEvent) -> Result<EventSeq> {
        Err(crate::error::OpenCompanyError::Store(
            "log is down".to_string(),
        ))
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        _seq: EventSeq,
        _limit: usize,
    ) -> Result<Vec<StoredEvent>> {
        Ok(Vec::new())
    }
    fn subscribe(
        &self,
        _id: &CompanyId,
    ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(futures::stream::empty())
    }
}

/// A real fs-backed tree behind the decorator, so it is exercised against a
/// genuine `WorkspaceStore` rather than a stub that cannot tell a create
/// from an overwrite.
fn wired() -> (
    tempfile::TempDir,
    WorkspaceAnnouncer,
    Arc<MemLog>,
    CompanyId,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let log = Arc::new(MemLog::default());
    let store = WorkspaceAnnouncer::new(Arc::new(FsOps::new(dir.path())), log.clone());
    (dir, store, log, CompanyId::new("announcer-co"))
}

fn note(id: &str, name: &str, parent: Option<&str>) -> WorkspaceNode {
    WorkspaceNode {
        id: id.to_string(),
        name: name.to_string(),
        kind: NodeKind::File,
        parent_id: parent.map(str::to_string),
        updated_at_millis: 1,
        created_by: WorkspaceOrigin::Operator,
        updated_by: WorkspaceOrigin::Operator,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    }
}

fn changes(log: &MemLog) -> Vec<(String, String)> {
    log.appended
        .lock()
        .unwrap()
        .iter()
        .map(|event| match event {
            CompanyEvent::WorkspaceChanged { node_id, change } => (node_id.clone(), change.clone()),
            other => panic!("expected a workspace announcement, got {other:?}"),
        })
        .collect()
}

/// The headline of #327: a note written by *anything* announces itself, and
/// the first write of an id is an `opened` rather than an `updated`.
#[tokio::test]
async fn a_new_note_announces_that_it_opened() {
    let (_dir, store, log, co) = wired();
    store
        .create(&co, &note("n-1", "brief.md", None), Some("body"))
        .await
        .unwrap();

    assert_eq!(
        changes(&log),
        vec![("n-1".to_string(), "opened".to_string())]
    );
}

/// Issue #759: a folder claim announces `opened` when it **mints** the
/// folder and says nothing when it adopts one.
///
/// Both halves are load-bearing. Without the first, a published deliverable
/// would appear in a tab that never learned its folder existed. Without the
/// second, every publish into an established folder — which is nearly every
/// publish a company ever makes — would put an `opened` frame on the feed
/// for a node that has been sitting there for weeks.
#[tokio::test]
async fn a_folder_claim_announces_only_when_it_mints_the_folder() {
    let (_dir, store, log, co) = wired();

    let claim = store
        .adopt_or_create_folder(&co, None, "Agents", WorkspaceOrigin::Seed)
        .await
        .unwrap();
    assert!(claim.was_created());
    assert_eq!(
        changes(&log),
        vec![(claim.node().id.clone(), "opened".to_string())]
    );
    log.appended.lock().unwrap().clear();

    let adopted = store
        .adopt_or_create_folder(&co, None, "Agents", WorkspaceOrigin::Seed)
        .await
        .unwrap();
    assert!(!adopted.was_created());
    assert_eq!(adopted.node().id, claim.node().id);
    assert!(
        changes(&log).is_empty(),
        "adopting a folder that was already standing changed nothing"
    );
}

/// An overwrite announces an update — the frame an agent's `workspace_write`
/// and the console's `PUT` both need, and the one the Workspace tab had no
/// way to learn about at all.
#[tokio::test]
async fn overwriting_a_note_announces_an_update() {
    let (_dir, store, log, co) = wired();
    store
        .create(&co, &note("n-1", "brief.md", None), Some("body"))
        .await
        .unwrap();
    store
        .write(&co, "n-1", "new body", WorkspaceOrigin::Operator)
        .await
        .unwrap();

    assert_eq!(
        changes(&log),
        vec![
            ("n-1".to_string(), "opened".to_string()),
            ("n-1".to_string(), "updated".to_string()),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conditional_writes_cross_all_decorators_and_only_announce_success() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(FsOps::new(dir.path()));
    let guarded = Arc::new(crate::runtime::DerivedGuardWorkspace::new(
        backend.clone(),
        backend,
    ));
    let metered = Arc::new(crate::runtime::QuotaEnforcedWorkspace::new(
        guarded,
        crate::runtime::WorkspaceQuota::default(),
    ));
    let log = Arc::new(MemLog::default());
    let store = Arc::new(WorkspaceAnnouncer::new(metered, log.clone()));
    crate::store::conformance::assert_workspace_conditional_write(store.clone(), store).await;
    assert_eq!(
        changes(&log),
        vec![
            ("note".to_string(), CHANGE_OPENED.to_string()),
            ("note".to_string(), CHANGE_UPDATED.to_string()),
            ("note".to_string(), CHANGE_UPDATED.to_string()),
            ("note".to_string(), CHANGE_UPDATED.to_string()),
        ]
    );
}

/// A rename that renames announces; one that names no change is silent. A
/// frame for a tree that did not move is noise a console cannot tell from a
/// real change.
#[tokio::test]
async fn a_rename_announces_only_when_something_actually_moved() {
    let (_dir, store, log, co) = wired();
    store
        .create(&co, &note("n-1", "brief.md", None), Some("body"))
        .await
        .unwrap();
    log.appended.lock().unwrap().clear();

    // A no-op: the port reads `None` as "leave this alone".
    store.rename_move(&co, "n-1", None, None).await.unwrap();
    assert!(changes(&log).is_empty(), "a no-op rename must be silent");

    // A rename to the name it already carries is the same non-event — an
    // operator opening the dialog and pressing save.
    store
        .rename_move(&co, "n-1", Some("brief.md"), None)
        .await
        .unwrap();
    assert!(
        changes(&log).is_empty(),
        "renaming to the current name changed nothing"
    );

    store
        .rename_move(&co, "n-1", Some("launch.md"), None)
        .await
        .unwrap();
    assert_eq!(
        changes(&log),
        vec![("n-1".to_string(), "updated".to_string())]
    );
}

/// A removed note announces its removal, and deleting an id the tree never
/// held announces nothing — it changed nothing.
#[tokio::test]
async fn a_deleted_note_announces_removal_and_an_absent_one_is_silent() {
    let (_dir, store, log, co) = wired();
    store
        .create(&co, &note("n-1", "brief.md", None), Some("body"))
        .await
        .unwrap();
    log.appended.lock().unwrap().clear();

    assert!(!store.delete(&co, "never-existed").await.unwrap());
    assert!(changes(&log).is_empty());

    assert!(store.delete(&co, "n-1").await.unwrap());
    assert_eq!(
        changes(&log),
        vec![("n-1".to_string(), "removed".to_string())]
    );
}

/// Deleting a folder is one frame for the folder, not one per descendant.
/// The frame says the tree moved; the console re-reads the tree and
/// discovers everything that went with it.
#[tokio::test]
async fn deleting_a_folder_announces_once_not_once_per_descendant() {
    let (_dir, store, log, co) = wired();
    let folder = WorkspaceNode {
        kind: NodeKind::Folder,
        ..note("f-1", "Specs", None)
    };
    store.create(&co, &folder, None).await.unwrap();
    store
        .create(&co, &note("n-1", "a.md", Some("f-1")), Some("a"))
        .await
        .unwrap();
    store
        .create(&co, &note("n-2", "b.md", Some("f-1")), Some("b"))
        .await
        .unwrap();
    log.appended.lock().unwrap().clear();

    assert!(store.delete(&co, "f-1").await.unwrap());
    assert_eq!(
        changes(&log),
        vec![("f-1".to_string(), "removed".to_string())],
        "a subtree removal is one frame, not a burst saying the same thing"
    );
}

/// Reads pass straight through and announce nothing — this decorator is
/// about writes, and a tab that refetched on every read would loop.
#[tokio::test]
async fn reads_announce_nothing() {
    let (_dir, store, log, co) = wired();
    store
        .create(&co, &note("n-1", "brief.md", None), Some("body"))
        .await
        .unwrap();
    log.appended.lock().unwrap().clear();

    store.tree(&co).await.unwrap();
    store.read(&co, "n-1").await.unwrap();
    store.is_empty(&co).await.unwrap();
    assert!(changes(&log).is_empty());
}

/// Record-keeping never fails the work it records: a refusing event log
/// leaves the write successful and the note in the tree.
#[tokio::test]
async fn a_refusing_event_log_does_not_fail_the_workspace_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceAnnouncer::new(Arc::new(FsOps::new(dir.path())), Arc::new(BrokenLog));
    let co = CompanyId::new("announcer-broken");

    store
        .create(&co, &note("n-1", "brief.md", None), Some("body"))
        .await
        .expect("the write succeeds even when the announcement cannot be filed");
    store
        .write(&co, "n-1", "revised", WorkspaceOrigin::Operator)
        .await
        .expect("and so does the overwrite");
    assert_eq!(store.tree(&co).await.unwrap().len(), 1);
    assert_eq!(store.read(&co, "n-1").await.unwrap().unwrap().1, "revised");
}
