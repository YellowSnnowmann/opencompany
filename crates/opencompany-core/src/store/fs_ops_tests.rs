use super::*;
use crate::ports::facts::FactKind;
use crate::ports::skills_state::SkillSource;
use crate::ports::usage::SampleKind;
use crate::store::conformance;
use std::sync::Arc;

fn tmp_root() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-fsops-")
        .tempdir()
        .expect("tempdir")
}

fn workspace_node(id: &str, name: &str, kind: NodeKind, parent: Option<&str>) -> WorkspaceNode {
    WorkspaceNode {
        id: id.to_string(),
        name: name.to_string(),
        kind,
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

/// A file node carrying the mime a binary write requires; `size` and
/// `sha256` are the store's to compute.
fn binary_node(id: &str, name: &str, parent: Option<&str>) -> WorkspaceNode {
    WorkspaceNode {
        mime: Some("application/pdf".to_string()),
        ..workspace_node(id, name, NodeKind::File, parent)
    }
}

async fn collect_stream(stream: crate::ports::workspace::BlobStream) -> Vec<u8> {
    use futures::StreamExt;
    let mut stream = stream;
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.expect("chunk"));
    }
    out
}

/// Two root folders under one name, written straight to the index.
///
/// `create` refuses to *make* this shape now (issue #666), but it is
/// reachable in the field: an index written before that refusal existed
/// carries it, and `workspace_scaffold` finds such roots, declines to
/// resolve them and deliberately leaves them standing. So the state is
/// seeded rather than requested.
async fn seed_duplicate_roots(ops: &FsOps, company: &CompanyId) {
    let mut index = HashMap::new();
    for id in ["root-a", "root-b"] {
        index.insert(
            id.to_string(),
            workspace_node(id, "Desks", NodeKind::Folder, None),
        );
    }
    ops.bundle(company)
        .ensure_dirs()
        .await
        .expect("bundle dirs");
    ops.save_index(company, &index).await.expect("seed index");
}

/// Issue #666, one level below the sibling case: nodes under two
/// same-named folders are not siblings by `parent_id`, and still resolve to
/// one path. A check keyed on `(parent_id, name)` admits the second and
/// lets it overwrite the first node's bytes.
#[tokio::test]
async fn equal_names_under_duplicate_roots_cannot_claim_one_path() {
    let root_dir = tmp_root();
    let ops = FsOps::new(root_dir.path());
    let company = CompanyId::new("acme");
    seed_duplicate_roots(&ops, &company).await;

    ops.create_binary(
        &company,
        &binary_node("first", "report.pdf", Some("root-a")),
        b"the first payload",
    )
    .await
    .expect("the first child of the first root is fine");

    let err = ops
        .create_binary(
            &company,
            &binary_node("second", "report.pdf", Some("root-b")),
            b"a different payload entirely",
        )
        .await
        .expect_err("the second root's child resolves to the same path");
    assert!(
        matches!(err, OpenCompanyError::Conflict(_)),
        "a path already in use is a conflict, not a store error: {err:?}"
    );

    let (node, stream) = ops
        .read_bytes(&company, "first")
        .await
        .expect("read")
        .expect("the first node still exists");
    assert_eq!(
        collect_stream(stream).await,
        b"the first payload",
        "the refused create must not have overwritten the first payload"
    );
    assert_eq!(
        node.size,
        Some("the first payload".len() as u64),
        "nor left the surviving node describing bytes it no longer holds"
    );
    assert!(
        ops.tree(&company)
            .await
            .expect("tree")
            .iter()
            .all(|n| n.id != "second"),
        "a refused create must not leave a metadata row behind"
    );
}

/// The same path, reached by moving rather than creating.
#[tokio::test]
async fn a_move_cannot_claim_a_path_held_under_a_duplicate_root() {
    let root_dir = tmp_root();
    let ops = FsOps::new(root_dir.path());
    let company = CompanyId::new("acme");
    seed_duplicate_roots(&ops, &company).await;

    ops.create_binary(
        &company,
        &binary_node("first", "report.pdf", Some("root-a")),
        b"the first payload",
    )
    .await
    .expect("create under the first root");
    ops.create_binary(
        &company,
        &binary_node("mover", "draft.pdf", Some("root-b")),
        b"a different payload entirely",
    )
    .await
    .expect("a differently-named child of the second root is fine");

    let err = ops
        .rename_move(&company, "mover", Some("report.pdf"), None)
        .await
        .expect_err("renaming it onto the other root's path is refused");
    assert!(
        matches!(err, OpenCompanyError::Conflict(_)),
        "expected a conflict: {err:?}"
    );

    let (_, stream) = ops
        .read_bytes(&company, "first")
        .await
        .expect("read")
        .expect("the node whose path was targeted still exists");
    assert_eq!(
        collect_stream(stream).await,
        b"the first payload",
        "the refused move must not have overwritten it"
    );
    let (moved, stream) = ops
        .read_bytes(&company, "mover")
        .await
        .expect("read")
        .expect("and the mover still exists");
    assert_eq!(moved.name, "draft.pdf", "under its original name");
    assert_eq!(
        collect_stream(stream).await,
        b"a different payload entirely",
        "with its own bytes"
    );
}

#[tokio::test]
async fn conformance_task_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_task_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_user_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_user_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_session_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_session_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_login_code_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_login_code_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_fact_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_fact_store(Arc::new(FsOps::new(&root))).await;
    conformance::assert_artifact_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_run_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_run_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_deep_trace_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_deep_trace_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_run_store_workflow_join() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_run_store_workflow_join(Arc::new(FsOps::new(&root))).await;
}

/// The prune drops whole runs, never a torn half of one.
///
/// Backend-specific because it reaches past the cap, which would make the
/// shared conformance suite write thousands of rows on every backend to
/// assert a property one implementation owns.
#[tokio::test]
async fn deep_trace_prune_keeps_whole_runs() {
    use crate::ports::deep_trace::{
        DeepTraceStore, MAX_DEEP_RUNS_PER_COMPANY, RunStepDetailRecord, TurnStepDetail,
    };

    let root_dir = tmp_root();
    let ops = FsOps::new(root_dir.path());
    let company = CompanyId::new("alpha");

    // One run past the cap, two steps each, oldest first.
    let runs = MAX_DEEP_RUNS_PER_COMPANY + 1;
    for run in 0..runs {
        for seq in 0..2u32 {
            ops.append_step_detail(
                &company,
                &RunStepDetailRecord {
                    run_id: format!("run-{run:04}"),
                    step_seq: seq,
                    at_millis: (run as u64 + 1) * 1000,
                    detail: TurnStepDetail {
                        reasoning: Some(format!("run {run} step {seq}")),
                        ..TurnStepDetail::default()
                    },
                },
            )
            .await
            .unwrap();
        }
    }

    // The oldest run went entirely...
    assert!(
        ops.list_step_details(&company, "run-0000")
            .await
            .unwrap()
            .is_empty(),
        "the oldest run's bodies were pruned"
    );
    // ...and every survivor kept BOTH of its steps. A prune that dropped the
    // oldest N *rows* would leave a run holding half its own trace, which
    // reads as the agent stopping mid-thought.
    for run in 1..runs {
        assert_eq!(
            ops.list_step_details(&company, &format!("run-{run:04}"))
                .await
                .unwrap()
                .len(),
            2,
            "run {run} kept a torn half of its trace"
        );
    }
}

/// Appends are O(1): a flush that rewrites an existing ordinal appends a
/// line and lets the read side fold it, rather than rewriting the whole
/// company file per step (issue #1679). The raw file therefore carries both
/// lines, and the read returns the later one.
#[tokio::test]
async fn deep_trace_append_folds_at_read_not_at_write() {
    use crate::ports::deep_trace::{DeepTraceStore, RunStepDetailRecord, TurnStepDetail};

    let root_dir = tmp_root();
    let ops = FsOps::new(root_dir.path());
    let company = CompanyId::new("alpha");
    let record = |reasoning: &str, at: u64| RunStepDetailRecord {
        run_id: "run-a".to_string(),
        step_seq: 1,
        at_millis: at,
        detail: TurnStepDetail {
            reasoning: Some(reasoning.to_string()),
            ..TurnStepDetail::default()
        },
    };

    ops.append_step_detail(&company, &record("first", 10))
        .await
        .unwrap();
    ops.append_step_detail(&company, &record("second", 20))
        .await
        .unwrap();

    // The read folds last-write-wins per ordinal...
    let got = ops.list_step_details(&company, "run-a").await.unwrap();
    assert_eq!(got.len(), 1, "a re-write replaces rather than stacking");
    assert_eq!(got[0].detail.reasoning.as_deref(), Some("second"));

    // ...because the file still physically holds both lines: no rewrite
    // happened between appends.
    let path = ops.bundle(&company).deep_trace_jsonl();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        raw.lines().count(),
        2,
        "a same-ordinal append must not rewrite the whole file"
    );
}

/// A run row written before `task_id` could be absent still loads
/// (issue #983). Backend-independent — see the assertion's own docs — so it
/// is driven from the one backend every lane builds.
#[test]
fn conformance_legacy_run_row_loads() {
    conformance::assert_legacy_run_row_loads();
}

#[tokio::test]
async fn conformance_workflow_revision_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workflow_revision_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_workflow_run_output_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workflow_run_output_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_run_reaper() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_run_reaper(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_schedule_fire_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_schedule_fire_store(Arc::new(FsOps::new(&root))).await;
}

/// A fresh `FsOps` over the same root sees a prior instance's claims (issue
/// #241): the durable record is on disk, so the anchor survives the process
/// restart that motivated the whole port. Proves the fs backend's marker
/// files are read back, not just written.
#[tokio::test]
async fn schedule_fire_claim_survives_a_new_fsops_over_the_same_root() {
    use crate::ports::schedule_fires::ScheduleFireStore;
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    let company = crate::ports::types::CompanyId::new("acme");

    let first = FsOps::new(&root);
    assert!(first.claim_fire(&company, "workflow-x", 42).await.unwrap());

    // A brand-new store over the same root — the shape a restart produces.
    let second = FsOps::new(&root);
    assert!(
        !second.claim_fire(&company, "workflow-x", 42).await.unwrap(),
        "a restart must see the earlier claim and lose the repeat"
    );
    assert_eq!(
        second.latest_fire(&company, "workflow-x").await.unwrap(),
        Some(42),
        "the anchor is durable across a new instance"
    );
}

#[tokio::test]
async fn conformance_usage_meter() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_usage_meter(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_usage_retention() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_usage_retention(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_skill_state_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_skill_state_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_read_state_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_read_state_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_notification_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_notification_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_workspace_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workspace_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conformance_workspace_conditional_write() {
    let root = tmp_root();
    conformance::assert_workspace_conditional_write(
        Arc::new(FsOps::new(root.path())),
        Arc::new(FsOps::new(root.path())),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conformance_workspace_revision_mutations() {
    let root = tmp_root();
    conformance::assert_workspace_revision_mutations(
        Arc::new(FsOps::new(root.path())),
        Arc::new(FsOps::new(root.path())),
    )
    .await;
}

#[tokio::test]
async fn conformance_workspace_binary_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workspace_binary_store(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_workspace_read_capped() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workspace_read_capped(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_workspace_folder_claims() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workspace_folder_claims(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_workspace_create_rejects_an_absent_or_foreign_parent() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workspace_create_rejects_an_absent_or_foreign_parent(Arc::new(FsOps::new(
        &root,
    )))
    .await;
}

#[tokio::test]
async fn conformance_workspace_sibling_names() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workspace_sibling_names(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn conformance_workspace_adoption_lease() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workspace_adoption_lease(Arc::new(FsOps::new(&root))).await;
}

/// Issue #887, and the backend the case was written against: this is the
/// one that failed it. Node content was written with a bare
/// `tokio::fs::write`, so a reader inside the `O_TRUNC` window saw a
/// prefix — visibly when the cut split a codepoint, silently when it did
/// not. Multi-threaded because the race is between two blocking-pool
/// threads, which a current-thread runtime makes far rarer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conformance_workspace_read_never_tears() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workspace_read_never_tears(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conformance_workspace_read_capped_race() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_workspace_read_capped_race(Arc::new(FsOps::new(&root))).await;
}

#[tokio::test]
async fn workspace_files_land_on_disk_under_folders() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    let ops = FsOps::new(&root);
    let company = CompanyId::new("acme");
    let now = now_millis();
    // Qualified: `FsOps` implements `create` for the workspace, session, and
    // login-code ports, so the concrete receiver needs the trait named.
    WorkspaceStore::create(
        &ops,
        &company,
        &WorkspaceNode {
            id: "f1".into(),
            name: "brand".into(),
            kind: NodeKind::Folder,
            parent_id: None,
            updated_at_millis: now,
            created_by: WorkspaceOrigin::Operator,
            updated_by: WorkspaceOrigin::Operator,
            mime: None,
            size: None,
            sha256: None,
            adopted: false,
        },
        None,
    )
    .await
    .unwrap();
    WorkspaceStore::create(
        &ops,
        &company,
        &WorkspaceNode {
            id: "n1".into(),
            name: "voice.md".into(),
            kind: NodeKind::File,
            parent_id: Some("f1".into()),
            updated_at_millis: now,
            created_by: WorkspaceOrigin::Operator,
            updated_by: WorkspaceOrigin::Operator,
            mime: None,
            size: None,
            sha256: None,
            adopted: false,
        },
        Some("# Voice"),
    )
    .await
    .unwrap();
    let disk = root.join("companies/acme/workspace/brand/voice.md");
    assert_eq!(tokio::fs::read_to_string(&disk).await.unwrap(), "# Voice");

    // A rename physically relocates the subtree.
    ops.rename_move(&company, "f1", Some("Branding"), None)
        .await
        .unwrap();
    let moved = root.join("companies/acme/workspace/Branding/voice.md");
    assert!(tokio::fs::try_exists(&moved).await.unwrap());
    assert!(!tokio::fs::try_exists(&disk).await.unwrap());

    let _ = (FactKind::Fact, SkillSource::Company, SampleKind::Inference);
}
