use super::*;
use crate::feedback::types::{ConsentMode, FeedbackCategory, FeedbackInput, FeedbackItem};
use crate::ports::types::CompanyId;

/// The caller must hold the returned handle: it owns the bundle's root and
/// removes it on drop.
fn tmp_bundle() -> (tempfile::TempDir, Bundle) {
    let root = tempfile::Builder::new()
        .prefix("oc-feedback-")
        .tempdir()
        .expect("tempdir");
    let bundle = Bundle::new(root.path().to_path_buf(), &CompanyId::new("acme"));
    (root, bundle)
}

fn item(note: &str) -> FeedbackItem {
    FeedbackItem::capture(
        FeedbackInput {
            category: FeedbackCategory::Bug,
            note: note.into(),
            work_ref: None,
            template_name: None,
            template_version: None,
        },
        "0.1.0",
        ConsentMode::Manual,
    )
}

#[tokio::test]
async fn append_and_list_round_trips() {
    let (_root, bundle) = tmp_bundle();
    let store = FeedbackStore::new(&bundle);
    assert!(store.list().await.unwrap().is_empty());

    store.append(&item("first")).await.unwrap();
    store.append(&item("second")).await.unwrap();
    let all = store.list().await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].operator_words, "first");
    assert_eq!(all[1].operator_words, "second");
    tokio::fs::remove_dir_all(bundle.dir()).await.ok();
}

#[tokio::test]
async fn update_status_records_url_without_losing_others() {
    let (_root, bundle) = tmp_bundle();
    let store = FeedbackStore::new(&bundle);
    let a = item("a");
    let b = item("b");
    store.append(&a).await.unwrap();
    store.append(&b).await.unwrap();

    store
        .update_status(&b.id, "https://example/issues/7", "open")
        .await
        .unwrap();

    let all = store.list().await.unwrap();
    assert_eq!(all.len(), 2);
    let updated = all.iter().find(|i| i.id == b.id).unwrap();
    assert_eq!(
        updated.filed_issue_url.as_deref(),
        Some("https://example/issues/7")
    );
    assert_eq!(updated.issue_status.as_deref(), Some("open"));
    // The other item is untouched.
    let other = all.iter().find(|i| i.id == a.id).unwrap();
    assert!(other.filed_issue_url.is_none());
    tokio::fs::remove_dir_all(bundle.dir()).await.ok();
}

#[tokio::test]
async fn get_returns_the_named_item_or_none() {
    let (_root, bundle) = tmp_bundle();
    let store = FeedbackStore::new(&bundle);
    let a = item("a");
    let b = item("b");
    store.append(&a).await.unwrap();
    store.append(&b).await.unwrap();

    assert_eq!(store.get(&a.id).await.unwrap(), Some(a));
    assert_eq!(store.get(&b.id).await.unwrap(), Some(b));
    assert!(store.get("ghost").await.unwrap().is_none());
    tokio::fs::remove_dir_all(bundle.dir()).await.ok();
}

#[tokio::test]
async fn record_preview_freeze_the_body_a_confirm_will_post() {
    let (_root, bundle) = tmp_bundle();
    let store = FeedbackStore::new(&bundle);
    let it = item("a");
    store.append(&it).await.unwrap();

    store
        .record_preview(&it.id, "**Category:** bug\n\nthe run crashed")
        .await
        .unwrap();

    let stored = store.get(&it.id).await.unwrap().expect("item exists");
    assert_eq!(
        stored.scrubbed_body.as_deref(),
        Some("**Category:** bug\n\nthe run crashed")
    );
    // The other fields are untouched.
    assert!(stored.filed_issue_url.is_none());
    assert_eq!(stored.operator_words, "a");
    tokio::fs::remove_dir_all(bundle.dir()).await.ok();
}

/// Two feedback stores over one bundle must not erase each other's writes
/// (issue #388).
///
/// `update_status` is a read-modify-write: it reads the whole log, edits one
/// item in memory, and rewrites the file atomically. An append that lands
/// between that read and the rename is **gone** — the rewrite replaces the
/// file with a snapshot taken before it existed.
///
/// The lock that was supposed to prevent this was a bare
/// `TokioMutex<()>` **field** on `FeedbackStore`, so it only ever serialised
/// one instance against itself. `FeedbackStore::new(&bundle)` takes a bundle
/// and builds a fresh mutex every time, so two call sites over one company —
/// a console request handler and the closing-the-loop poller, say — hold two
/// unrelated locks over one file and serialise against nothing. Keying the
/// lock on the *path* in a process-wide registry is what makes the two
/// instances meet.
#[tokio::test]
async fn two_feedback_stores_over_one_bundle_do_not_erase_each_others_writes() {
    let (_root, bundle) = tmp_bundle();
    // Two independently-constructed stores over the same bundle.
    let appender = std::sync::Arc::new(FeedbackStore::new(&bundle));
    let updater = std::sync::Arc::new(FeedbackStore::new(&bundle));

    // Seed one item so the updater has something to rewrite around.
    let seed = item("seed");
    updater.append(&seed).await.unwrap();

    const APPENDS: usize = 32;
    const UPDATES: usize = 8;
    let mut set = tokio::task::JoinSet::new();
    for i in 0..APPENDS {
        let store = appender.clone();
        set.spawn(async move { store.append(&item(&format!("note-{i}"))).await });
    }
    for i in 0..UPDATES {
        let store = updater.clone();
        let id = seed.id.clone();
        set.spawn(async move {
            store
                .update_status(&id, &format!("https://example/issues/{i}"), "open")
                .await
        });
    }
    while let Some(res) = set.join_next().await {
        res.expect("task joins").expect("write succeeds");
    }

    let all = appender.list().await.expect("no corrupt lines");
    let mut notes: Vec<String> = all.iter().map(|i| i.operator_words.clone()).collect();
    notes.sort();
    let mut want: Vec<String> = (0..APPENDS).map(|i| format!("note-{i}")).collect();
    want.push("seed".to_string());
    want.sort();
    assert_eq!(
        notes, want,
        "a whole-file rewrite must not erase an append that raced it — every \
         item written by either instance has to survive"
    );

    // The update itself still landed, so serialising did not cost the write.
    let updated = all.iter().find(|i| i.id == seed.id).expect("seed survives");
    assert!(
        updated.filed_issue_url.is_some(),
        "the status update must be recorded"
    );
    assert_eq!(updated.issue_status.as_deref(), Some("open"));

    tokio::fs::remove_dir_all(bundle.dir()).await.ok();
}

/// Every appended item must occupy its own physical line.
///
/// `append` used to write the record and its `\n` as two `write_all` calls on
/// a `tokio::fs::File`, which buffers and can return before the kernel write
/// lands — so a newline could be reordered or lost and two records shared one
/// line, which `list` then rejects with a `serde_json` "trailing characters"
/// error. That is what intermittently failed
/// `update_status_records_url_without_losing_others` in CI. Mirrors
/// `store::fs::test::concurrent_appends_stay_one_record_per_line` (PR #43).
#[tokio::test]
async fn concurrent_appends_stay_one_record_per_line() {
    let (_root, bundle) = tmp_bundle();
    let store = std::sync::Arc::new(FeedbackStore::new(&bundle));

    const N: usize = 32;
    let mut set = tokio::task::JoinSet::new();
    for i in 0..N {
        let store = store.clone();
        set.spawn(async move { store.append(&item(&format!("note-{i}"))).await });
    }
    while let Some(res) = set.join_next().await {
        res.unwrap().expect("append succeeds");
    }

    // `list` parses every line: a merged record would fail here rather than
    // silently vanish.
    let all = store.list().await.expect("no corrupt lines");
    assert_eq!(all.len(), N, "every append is its own line");
    let mut notes: Vec<String> = all.into_iter().map(|i| i.operator_words).collect();
    notes.sort();
    let mut want: Vec<String> = (0..N).map(|i| format!("note-{i}")).collect();
    want.sort();
    assert_eq!(notes, want, "all records intact");

    tokio::fs::remove_dir_all(bundle.dir()).await.ok();
}
