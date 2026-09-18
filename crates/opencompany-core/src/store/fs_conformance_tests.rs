use super::tests::tmp_root;
use super::*;
use crate::store::conformance;

// The fs backend runs the identical port-conformance suite the sqlite
// backend runs under `--features sqlite`. Each test gets a fresh root so the
// stores start empty.
#[tokio::test]
async fn conformance_isolation_by_company() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_isolation_by_company(
        Arc::new(FsCompanyStore::new(&root)),
        Arc::new(FsEventLog::new(&root)),
        Arc::new(FsMemoryStore::new(&root)),
        Arc::new(FsContextStore::new(&root)),
    )
    .await;
}

#[tokio::test]
async fn conformance_paused_ordinary_save_preserves_activation_gate() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_paused_ordinary_save_preserves_activation_gate(Arc::new(
        FsCompanyStore::new(&root),
    ))
    .await;
}

#[tokio::test]
async fn conformance_append_only_event_and_ledger() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_append_only_event_and_ledger(
        Arc::new(FsCompanyStore::new(&root)),
        Arc::new(FsEventLog::new(&root)),
    )
    .await;
}

#[tokio::test]
async fn conformance_monotonic_event_seq() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_monotonic_event_seq(Arc::new(FsEventLog::new(&root))).await;
}

#[tokio::test]
async fn conformance_event_subscription_surfaces_gap() {
    let root_dir = tmp_root();
    conformance::assert_event_subscription_surfaces_gap(Arc::new(FsEventLog::new(root_dir.path())))
        .await;
}

#[tokio::test]
async fn conformance_event_read_before() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_event_read_before(Arc::new(FsEventLog::new(&root))).await;
}

#[tokio::test]
async fn conformance_event_retention() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_event_retention(Arc::new(FsEventLog::new(&root))).await;
}

#[tokio::test]
async fn conformance_inbox_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_inbox_store(Arc::new(FsInboxStore::new(&root))).await;
}

/// Issue #1505. The port holds this company's inference credential, its MCP
/// OAuth tokens and its SMTP password, and had no conformance case on any
/// backend until this one.
#[tokio::test]
async fn conformance_secret_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_secret_store(Arc::new(FsSecretStore::new(&root))).await;
}

#[tokio::test]
async fn conformance_context_chunk_stamps() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_context_chunk_stamps(Arc::new(FsContextStore::new(&root))).await;
}

// The fs backend keeps the port's default `peek_many` (per-file reads are
// its floor), so this run is also the default implementation's own proof.
#[tokio::test]
async fn conformance_context_peek_many() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_context_peek_many_answers_positionally(Arc::new(FsContextStore::new(
        &root,
    )))
    .await;
}

#[tokio::test]
async fn conformance_context_multibyte_bodies() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_multibyte_bodies_survive_search_and_ranged_peek(Arc::new(
        FsContextStore::new(&root),
    ))
    .await;
}

#[tokio::test]
async fn conformance_context_identical_body_two_labels() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_identical_body_two_labels(Arc::new(FsContextStore::new(&root))).await;
}

#[tokio::test]
async fn conformance_context_delete_label_scoped() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_delete_label_scoped(Arc::new(FsContextStore::new(&root))).await;
}

#[tokio::test]
async fn conformance_context_delete_label_survives_a_concurrent_identical_put() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_delete_label_survives_a_concurrent_identical_put(Arc::new(
        FsContextStore::new(&root),
    ))
    .await;
}

/// This backend keeps `put` an O(1) append and applies the (addr, label)
/// set semantics on the read side (#1300), so the two halves need pinning
/// together: a duplicate row on disk, exactly one claim through `list`,
/// one hit through `search`, and a `delete_label` that takes every
/// duplicate row with it — a survivor would resurrect a forgotten claim.
#[tokio::test]
async fn a_duplicate_index_row_reads_back_as_one_claim_and_deletes_whole() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    let context = FsContextStore::new(&root);
    let id = CompanyId::new("acme");
    let chunk = || ContextChunk {
        label: "notes/one".to_string(),
        body: "remembered twice".to_string(),
    };

    let addr = context.put(&id, chunk()).await.unwrap();
    assert_eq!(context.put(&id, chunk()).await.unwrap(), addr);

    // The append really did write a second row — this is the cost the
    // read-side dedupe exists to absorb, so assert it rather than assume.
    let index_path = Bundle::new(root.clone(), &id).context_index_jsonl();
    let rows = read_jsonl::<IndexEntry>(&index_path).await.unwrap();
    assert_eq!(rows.len(), 2, "put appends without reading the index");

    let metas = context.list(&id, "").await.unwrap();
    assert_eq!(metas.len(), 1, "the duplicate row is one claim: {metas:?}");
    assert_eq!(metas[0].stored_at_millis, rows[0].stored_at_millis);
    assert_eq!(
        context.search(&id, "remembered", 10).await.unwrap().len(),
        1,
        "recall reports the body once, not once per row"
    );

    assert!(context.delete_label(&id, &addr, "notes/one").await.unwrap());
    assert!(context.list(&id, "").await.unwrap().is_empty());
    assert!(
        context.peek(&id, &addr, None).await.is_err(),
        "every duplicate row went, so the body is unreferenced and reaped"
    );
}

/// The deterministic half of the atomicity guarantee: a re-`put` publishes
/// a *new* file over the old name, so the blob's inode changes. A plain
/// truncating write — the shape that let a reader see a prefix — keeps the
/// same inode and fails this every run, where the racing test below only
/// reddens when the reader happens to land inside the truncate window.
#[cfg(unix)]
#[tokio::test]
async fn a_re_put_publishes_a_new_blob_instead_of_truncating_in_place() {
    use std::os::unix::fs::MetadataExt;

    let root_dir = tmp_root();
    let store = FsContextStore::new(root_dir.path().to_path_buf());
    let id = CompanyId::new("alpha");
    let chunk = ContextChunk {
        label: "agent/atomic".to_string(),
        body: "the same body, published twice".to_string(),
    };
    let addr = store.put(&id, chunk.clone()).await.unwrap();
    let blob_path = Bundle::new(root_dir.path().to_path_buf(), &id).context_blob(addr.as_ref());
    let before = tokio::fs::metadata(&blob_path).await.unwrap().ino();

    store.put(&id, chunk).await.unwrap();

    let after = tokio::fs::metadata(&blob_path).await.unwrap().ino();
    assert_ne!(
        before, after,
        "the blob was rewritten in place; a concurrent reader can see the \
             truncate window"
    );
}

/// A re-`put` of an already-indexed blob must never expose a torn body: the
/// blob is republished via tmp-then-rename, so a racing `peek` sees the
/// old bytes or the new bytes in full — with a plain truncating write it
/// could read an empty or partial file for the whole write window.
///
/// Racing, so its redness is probabilistic (the reader must land inside the
/// write window); the inode test above is the every-run proof. This one
/// guards what the inode cannot: that the bytes a racing reader *does* get
/// are always a whole body.
#[tokio::test]
async fn a_concurrent_peek_never_sees_a_torn_blob_rewrite() {
    let root_dir = tmp_root();
    let store = Arc::new(FsContextStore::new(root_dir.path().to_path_buf()));
    let id = CompanyId::new("alpha");
    // Large enough that a truncate-then-stream write has a visible window.
    let body = "x".repeat(64 * 1024);
    let chunk = ContextChunk {
        label: "agent/atomic".to_string(),
        body: body.clone(),
    };
    let addr = store.put(&id, chunk.clone()).await.unwrap();

    for _ in 0..50 {
        let writer = {
            let store = store.clone();
            let id = id.clone();
            let chunk = chunk.clone();
            tokio::spawn(async move { store.put(&id, chunk).await.unwrap() })
        };
        let reader = {
            let store = store.clone();
            let id = id.clone();
            let addr = addr.clone();
            tokio::spawn(async move { store.peek(&id, &addr, None).await.unwrap() })
        };
        let read = reader.await.unwrap();
        assert_eq!(
            read.len(),
            body.len(),
            "a concurrent peek saw a torn blob rewrite"
        );
        assert_eq!(read, body);
        writer.await.unwrap();
    }
}

/// Once the index rows are gone the delete HAS happened; a blob that will
/// not remove is an unreferenced orphan, and reporting an error for it
/// would tell the caller nothing was deleted after half of it was.
#[tokio::test]
async fn delete_reports_the_index_result_even_when_the_blob_will_not_remove() {
    let root_dir = tmp_root();
    let store = FsContextStore::new(root_dir.path().to_path_buf());
    let id = CompanyId::new("alpha");
    let addr = store
        .put(
            &id,
            ContextChunk {
                label: "agent/orphan".to_string(),
                body: "orphan me".to_string(),
            },
        )
        .await
        .unwrap();

    // Swap the blob for a non-empty directory so `remove_file` must fail
    // with something other than NotFound, on every platform.
    let blob_path = Bundle::new(root_dir.path().to_path_buf(), &id).context_blob(addr.as_ref());
    tokio::fs::remove_file(&blob_path).await.unwrap();
    tokio::fs::create_dir(&blob_path).await.unwrap();
    tokio::fs::write(blob_path.join("occupant"), b"x")
        .await
        .unwrap();

    assert!(
        store.delete(&id, &addr).await.unwrap(),
        "the index rows were removed, so the delete happened"
    );
    assert!(
        store.list(&id, "").await.unwrap().is_empty(),
        "the index is the source of truth and it is empty"
    );
    assert!(
        store.search(&id, "orphan", 8).await.unwrap().is_empty(),
        "search is index-driven, so the orphan does not surface"
    );
    // Documented residual: peek is blob-path-driven, so the exact addr
    // can still read the orphan until the file is reclaimed. Here the
    // blob was replaced by a directory, so the read fails — the point
    // pinned is that delete's answer did not depend on it either way.
    let _ = store.peek(&id, &addr, None).await;
}

/// The same search semantics as every other backend.
#[tokio::test]
async fn conformance_context_search_ranking() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_context_search_ranking(Arc::new(FsContextStore::new(&root))).await;
}

/// Two event logs over one data root must not hand out the same sequence
/// number (issue #388).
///
/// `EventLog::append` computes the next `seq` by counting the lines already
/// in the file, then appends. That read-then-append is only atomic under a
/// lock, and the lock used to be a **field** on `FsEventLog` — so two
/// instances over one bundle serialised against nothing, both read the same
/// count, and both wrote the same `seq`. A duplicate sequence number breaks
/// every consumer that treats it as an identity: `read_from`'s `seq >=`
/// cursor silently replays, and the console's resume-from-seq skips.
///
/// Nothing stops a second instance being constructed — `FsEventLog::new`
/// takes a root and is called wherever one is needed — so this is reachable
/// without any exotic setup, which is exactly what makes it worth a lock in
/// the registry rather than a convention.
#[tokio::test]
async fn two_event_logs_over_one_root_never_reuse_a_sequence() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    // Two independently-constructed logs over the same data root — the shape
    // a second runtime, a maintenance task, or an export job produces.
    let first = Arc::new(FsEventLog::new(&root));
    let second = Arc::new(FsEventLog::new(&root));
    let id = CompanyId::new("acme");

    const N: u64 = 32;
    let mut set = tokio::task::JoinSet::new();
    for i in 0..N {
        let log = if i % 2 == 0 {
            first.clone()
        } else {
            second.clone()
        };
        let id = id.clone();
        set.spawn(async move {
            log.append(
                &id,
                CompanyEvent::OperatorMessage {
                    mentions: Vec::new(),
                    parent: None,
                    text: format!("event {i}"),
                    by: None,
                    chat: None,
                    deliverable: None,
                    attachments: Vec::new(),
                },
            )
            .await
            .expect("append succeeds")
        });
    }
    let mut handed_out = Vec::new();
    while let Some(res) = set.join_next().await {
        handed_out.push(res.expect("task joins").value());
    }

    handed_out.sort_unstable();
    assert_eq!(
        handed_out,
        (0..N).collect::<Vec<_>>(),
        "the sequences handed to callers must be unique and dense — a repeat \
             means two instances read the same line count before either appended"
    );

    // And the same must hold for what actually landed on disk.
    let stored = first.read_from(&id, EventSeq::new(0), 1024).await.unwrap();
    assert_eq!(stored.len() as u64, N, "every append is on disk");
    let mut persisted: Vec<u64> = stored.iter().map(|e| e.seq.value()).collect();
    persisted.sort_unstable();
    assert_eq!(
        persisted,
        (0..N).collect::<Vec<_>>(),
        "the persisted sequences must be unique and dense too"
    );
}

/// The fs backend's migration path: index lines written before
/// `stored_at_millis` existed carry no such field, and must still
/// deserialize — reporting an unknown (`0`) store time rather than failing
/// the read and blanking the whole Brain list.
#[test]
fn legacy_context_index_line_without_a_stamp_still_parses() {
    let legacy = r#"{"addr":"abc123","label":"agent/ceo","len":24}"#;
    let entry: IndexEntry = serde_json::from_str(legacy).expect("legacy index line parses");
    assert_eq!(entry.addr, "abc123");
    assert_eq!(entry.label, "agent/ceo");
    assert_eq!(entry.len, 24);
    assert_eq!(entry.stored_at_millis, 0);
}

#[tokio::test]
async fn conformance_export_totality() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_export_totality(
        Arc::new(FsCompanyStore::new(&root)),
        Arc::new(FsEventLog::new(&root)),
        Arc::new(FsMemoryStore::new(&root)),
        Arc::new(FsContextStore::new(&root)),
    )
    .await;
}
