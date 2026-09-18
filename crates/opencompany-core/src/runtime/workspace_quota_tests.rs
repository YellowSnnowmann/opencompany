use super::*;
use crate::ports::workspace::NodeKind;
use crate::store::FsOps;

fn wired(quota: WorkspaceQuota) -> (tempfile::TempDir, QuotaEnforcedWorkspace, CompanyId) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = QuotaEnforcedWorkspace::new(Arc::new(FsOps::new(dir.path())), quota);
    (dir, store, CompanyId::new("quota-co"))
}

fn png(id: &str, name: &str) -> WorkspaceNode {
    WorkspaceNode {
        id: id.to_string(),
        name: name.to_string(),
        kind: NodeKind::File,
        parent_id: None,
        updated_at_millis: 1,
        created_by: WorkspaceOrigin::Operator,
        updated_by: WorkspaceOrigin::Operator,
        mime: Some("image/png".to_string()),
        size: None,
        sha256: None,
        adopted: false,
    }
}

/// The per-file cap refuses, names both numbers, and says nothing was
/// stored — the sentence an operator has to act on.
#[tokio::test]
async fn a_file_over_the_per_file_cap_is_refused_by_name_and_size() {
    let (_dir, store, co) = wired(WorkspaceQuota {
        max_blob_bytes: 10,
        tree_quota_bytes: None,
    });

    let err = store
        .create_binary(&co, &png("big", "hero.png"), &[0u8; 64])
        .await
        .expect_err("over the per-file cap");
    let msg = err.to_string();
    assert!(msg.contains("hero.png"), "{msg}");
    assert!(msg.contains("64 bytes"), "{msg}");
    assert!(msg.contains("10 bytes"), "{msg}");
    assert!(msg.contains("Nothing was stored"), "{msg}");
}

/// A refused write leaves **nothing** behind. This is the property the
/// buffered-write design exists to make true, so it is asserted against a
/// real store rather than argued for in a comment.
#[tokio::test]
async fn a_refused_write_leaves_the_tree_exactly_as_it_was() {
    let (_dir, store, co) = wired(WorkspaceQuota {
        max_blob_bytes: 10,
        tree_quota_bytes: None,
    });
    store
        .create_binary(&co, &png("ok", "small.png"), b"12345")
        .await
        .unwrap();
    let before = store.tree(&co).await.unwrap();

    assert!(
        store
            .create_binary(&co, &png("big", "hero.png"), &[0u8; 64])
            .await
            .is_err()
    );

    let after = store.tree(&co).await.unwrap();
    assert_eq!(after.len(), before.len(), "no node was created");
    assert!(after.iter().all(|n| n.id != "big"));
    assert!(
        store.read_bytes(&co, "big").await.unwrap().is_none(),
        "and no payload was left behind for a sweep to find"
    );
}

/// The tree quota totals what is stored and refuses the write that would
/// cross it, naming the usage so the operator knows what to delete.
#[tokio::test]
async fn the_tree_quota_counts_what_is_stored_and_names_the_usage() {
    let (_dir, store, co) = wired(WorkspaceQuota {
        max_blob_bytes: DEFAULT_MAX_BLOB_BYTES,
        tree_quota_bytes: Some(100),
    });
    store
        .create_binary(&co, &png("a", "a.png"), &[0u8; 60])
        .await
        .unwrap();
    // 60 + 30 fits.
    store
        .create_binary(&co, &png("b", "b.png"), &[0u8; 30])
        .await
        .unwrap();
    // 90 + 20 does not.
    let err = store
        .create_binary(&co, &png("c", "c.png"), &[0u8; 20])
        .await
        .expect_err("over the tree quota");
    let msg = err.to_string();
    assert!(msg.contains("c.png"), "{msg}");
    assert!(msg.contains("90 bytes"), "already-used total: {msg}");
    assert!(msg.contains("100 bytes"), "the limit: {msg}");

    // Freeing space makes the same write succeed — the total is read from
    // the tree, not from a counter that could drift.
    assert!(store.delete(&co, "b").await.unwrap());
    store
        .create_binary(&co, &png("c", "c.png"), &[0u8; 20])
        .await
        .expect("60 + 20 fits under 100");
}

/// Replacing a payload does not double-count the one it replaces: a company
/// at its limit must still be able to swap a file for a smaller one.
#[tokio::test]
async fn replacing_a_payload_frees_the_bytes_it_overwrites() {
    let (_dir, store, co) = wired(WorkspaceQuota {
        max_blob_bytes: DEFAULT_MAX_BLOB_BYTES,
        tree_quota_bytes: Some(100),
    });
    store
        .create_binary(&co, &png("a", "a.png"), &[0u8; 95])
        .await
        .unwrap();

    // Naively 95 + 90 = 185 > 100; correctly the old 95 is being freed.
    store
        .write_binary(&co, "a", &[0u8; 90], None, WorkspaceOrigin::Operator)
        .await
        .expect("a replacement is measured against the tree without the node it replaces");

    // …but a replacement that is genuinely too big is still refused.
    assert!(
        store
            .write_binary(&co, "a", &[0u8; 120], None, WorkspaceOrigin::Operator)
            .await
            .is_err()
    );
}

/// Prose is uncounted, by design — see the module docs. Pinned so the
/// narrowing is a decision on record rather than something a later reader
/// has to infer from the absence of a check.
#[tokio::test]
async fn text_notes_are_not_metered() {
    let (_dir, store, co) = wired(WorkspaceQuota {
        max_blob_bytes: 10,
        tree_quota_bytes: Some(10),
    });
    let note = WorkspaceNode {
        mime: None,
        ..png("note", "brief.md")
    };
    store
        .create(&co, &note, Some(&"x".repeat(5_000)))
        .await
        .expect("a note is not measured against the blob quota");
    store
        .write(&co, "note", &"y".repeat(9_000), WorkspaceOrigin::Operator)
        .await
        .expect("nor is an overwrite");
}

/// The upload-only admission seam uses the company's configured per-file
/// cap without applying the binary-tree total to prose that remains
/// deliberately uncounted.
#[tokio::test]
async fn upload_admission_uses_the_custom_cap_but_not_the_tree_total() {
    let (_dir, store, co) = wired(WorkspaceQuota {
        max_blob_bytes: 64,
        tree_quota_bytes: Some(1),
    });

    store
        .admit_upload(&co, "at.csv", 64)
        .await
        .expect("exactly at the configured per-file cap is admitted");
    let err = store
        .admit_upload(&co, "over.csv", 65)
        .await
        .expect_err("one byte over the configured cap is refused");
    let message = err.to_string();
    assert!(message.contains("over.csv"), "{message}");
    assert!(message.contains("65 bytes"), "{message}");
    assert!(message.contains("64 bytes"), "{message}");
}

/// The digest is the store's, not the caller's — the decorator forwards
/// bytes and never a claim about them, so a lying caller changes nothing.
#[tokio::test]
async fn the_stored_digest_is_computed_from_the_bytes_not_taken_from_the_caller() {
    let (_dir, store, co) = wired(WorkspaceQuota::default());
    let lying = WorkspaceNode {
        size: Some(1),
        sha256: Some("deadbeef".to_string()),
        adopted: false,
        ..png("img", "hero.png")
    };
    store
        .create_binary(&co, &lying, b"real-bytes")
        .await
        .unwrap();

    let (node, _) = store.read_bytes(&co, "img").await.unwrap().unwrap();
    let (size, sha) = crate::ports::workspace::blob_metadata(b"real-bytes");
    assert_eq!(node.size, Some(size));
    assert_eq!(node.sha256.as_deref(), Some(sha.as_str()));
}

/// The default admits an ordinary upload and still caps a runaway one.
#[tokio::test]
async fn the_default_quota_caps_per_file_and_leaves_the_tree_unlimited() {
    let quota = WorkspaceQuota::default();
    assert_eq!(quota.max_blob_bytes, DEFAULT_MAX_BLOB_BYTES);
    assert_eq!(quota.tree_quota_bytes, None);
    assert!(!quota.is_unlimited());
}

/// The boundary, from both sides.
///
/// An off-by-one here is not cosmetic: one direction refuses a deliverable
/// that is exactly at the documented limit, and the other is the first byte
/// of an unbounded write path. Asserted on the decorator rather than by
/// storing 64 MiB three times, because the comparison *is* the logic —
/// everything below it is the backends' round-trip, which
/// `assert_workspace_binary_store` already pins with a 17 MiB payload.
#[tokio::test]
async fn a_write_at_the_cap_is_admitted_and_one_byte_over_is_refused() {
    let (_dir, store, co) = wired(WorkspaceQuota {
        max_blob_bytes: 64,
        tree_quota_bytes: None,
    });

    store
        .create_binary(&co, &png("at", "at.png"), &[0u8; 64])
        .await
        .expect("exactly at the cap must be stored");

    let err = store
        .create_binary(&co, &png("over", "over.png"), &[0u8; 65])
        .await
        .expect_err("one byte over must be refused");
    // Actionable: what was attempted and what is allowed. A truncation
    // would be worse than either — a truncated binary is a corrupt binary,
    // and it would carry a digest the store computed over the wrong bytes.
    let msg = err.to_string();
    assert!(msg.contains("65 bytes"), "the attempted size: {msg}");
    assert!(msg.contains("64 bytes"), "the limit: {msg}");
    assert!(msg.contains("Nothing was stored"), "{msg}");

    // The refusal stored nothing — not a truncated 64-byte node.
    assert!(store.read_bytes(&co, "over").await.unwrap().is_none());
    // …and the replacement path enforces the same boundary.
    assert!(
        store
            .write_binary(&co, "at", &[0u8; 65], None, WorkspaceOrigin::Operator)
            .await
            .is_err(),
        "the cap is per write, so a replacement is capped too"
    );
}
