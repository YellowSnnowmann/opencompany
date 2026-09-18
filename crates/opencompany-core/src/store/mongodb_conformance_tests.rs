use super::tests::{drop_db, store};
use super::*;
use crate::store::conformance;

#[tokio::test]
async fn conformance_isolation_by_company() {
    let Some(s) = store().await else { return };
    conformance::assert_isolation_by_company(s.clone(), s.clone(), s.clone(), s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_paused_ordinary_save_preserves_activation_gate() {
    let Some(s) = store().await else { return };
    conformance::assert_paused_ordinary_save_preserves_activation_gate(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_user_store() {
    let Some(s) = store().await else { return };
    conformance::assert_user_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_session_store() {
    let Some(s) = store().await else { return };
    conformance::assert_session_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_login_code_store() {
    let Some(s) = store().await else { return };
    conformance::assert_login_code_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_append_only_event_and_ledger() {
    let Some(s) = store().await else { return };
    conformance::assert_append_only_event_and_ledger(s.clone(), s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_monotonic_event_seq() {
    let Some(s) = store().await else { return };
    conformance::assert_monotonic_event_seq(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_event_subscription_surfaces_gap() {
    let Some(s) = store().await else { return };
    conformance::assert_event_subscription_surfaces_gap(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_event_read_before() {
    let Some(s) = store().await else { return };
    conformance::assert_event_read_before(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_event_retention() {
    let Some(s) = store().await else { return };
    conformance::assert_event_retention(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_export_totality() {
    let Some(s) = store().await else { return };
    conformance::assert_export_totality(s.clone(), s.clone(), s.clone(), s.clone()).await;
    drop_db(&s).await;
}

/// KeepRecent eviction deletes strictly-older-than-the-n-th-newest traces,
/// so a trace saved after the eviction snapshot can never be evicted by the
/// sweep that means to keep it. The old `$nin` predicate deleted any doc
/// whose seq was not in the keep set, so a `save_trace` landing between
/// evict's find and delete was dropped the same pass it was written — the
/// race this delete-by-cutoff form removes.
#[tokio::test]
async fn evict_keep_recent_spares_a_trace_saved_after_its_snapshot() {
    let Some(s) = store().await else { return };
    let id = CompanyId::new("acme");
    // 33 traces: `next_seq` hands out 0..=32 in save order.
    for i in 0..=32 {
        s.save_trace(&id, CompressedTrace::now(format!("c{i}"), format!("s{i}")))
            .await
            .unwrap();
    }
    // Keeps the newest 32 (seqs 1..=32), evicting exactly seq 0.
    let removed = s
        .evict(&id, EvictionPolicy::KeepRecent { n: 32 })
        .await
        .unwrap();
    assert_eq!(removed, 1);
    let kept = s.recent_traces(&id, usize::MAX).await.unwrap();
    assert_eq!(kept.len(), 32);
    assert_eq!(kept.first().unwrap().cycle_id, "c1");
    assert_eq!(kept.last().unwrap().cycle_id, "c32");

    // A trace written after the sweep's snapshot has seq 33, above the
    // cutoff (1): it must survive the retention policy. This is the write
    // the `$nin` predicate deleted when it landed between evict's find and
    // delete.
    s.save_trace(&id, CompressedTrace::now("c33", "s33"))
        .await
        .unwrap();
    let after = s.recent_traces(&id, usize::MAX).await.unwrap();
    assert_eq!(after.len(), 33);
    assert_eq!(after.last().unwrap().cycle_id, "c33");

    // KeepRecent{0} keeps nothing: every trace goes.
    let removed = s
        .evict(&id, EvictionPolicy::KeepRecent { n: 0 })
        .await
        .unwrap();
    assert_eq!(removed, 33);
    assert!(s.recent_traces(&id, usize::MAX).await.unwrap().is_empty());

    // KeepRecent above the current count is a no-op.
    s.save_trace(&id, CompressedTrace::now("c34", "s34"))
        .await
        .unwrap();
    let removed = s
        .evict(&id, EvictionPolicy::KeepRecent { n: 10 })
        .await
        .unwrap();
    assert_eq!(removed, 0);

    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_inbox_store() {
    let Some(s) = store().await else { return };
    conformance::assert_inbox_store(s.clone()).await;
    drop_db(&s).await;
}

/// Issue #1505. The port holds this tenant's inference credential, its MCP
/// OAuth tokens and its SMTP password, and had no conformance case on any
/// backend until this one — on the backend a hosted tenant actually runs,
/// where the company scope in the query IS the tenant boundary.
#[tokio::test]
async fn conformance_secret_store() {
    let Some(s) = store().await else { return };
    conformance::assert_secret_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_task_store() {
    let Some(s) = store().await else { return };
    conformance::assert_task_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_fact_store() {
    let Some(s) = store().await else { return };
    conformance::assert_fact_store(s.clone()).await;
    conformance::assert_artifact_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_context_chunk_stamps() {
    let Some(s) = store().await else { return };
    conformance::assert_context_chunk_stamps(s.clone()).await;
    drop_db(&s).await;
}

// Exercises this backend's single-`$in`-find `peek_many` override against
// the same positional contract the default implementation gives.
#[tokio::test]
async fn conformance_context_peek_many() {
    let Some(s) = store().await else { return };
    conformance::assert_context_peek_many_answers_positionally(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_context_multibyte_bodies() {
    let Some(s) = store().await else { return };
    conformance::assert_multibyte_bodies_survive_search_and_ranged_peek(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_context_identical_body_two_labels() {
    let Some(s) = store().await else { return };
    conformance::assert_identical_body_two_labels(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_context_delete_label_scoped() {
    let Some(s) = store().await else { return };
    conformance::assert_delete_label_scoped(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_context_delete_label_survives_a_concurrent_identical_put() {
    let Some(s) = store().await else { return };
    conformance::assert_delete_label_survives_a_concurrent_identical_put(s.clone()).await;
    drop_db(&s).await;
}

/// The legacy scalar-`label` document shape (written before the `labels`
/// set existed) keeps working through every read and through the
/// label-scoped delete — `doc_labels` unions the two shapes, and the
/// `$unset` leg of `delete_label` is what removes a scalar claim.
#[tokio::test]
async fn legacy_scalar_label_documents_list_and_label_delete() {
    let Some(s) = store().await else { return };
    let id = CompanyId::new("acme");
    let body = "written before the labels set";
    // Seed the pre-#1300 shape directly: scalar label, no labels array —
    // at the body's real content address, so a later put folds into it.
    s.collection("context_chunks")
        .insert_one(doc! {
            "company_id": id.as_ref(),
            "addr": content_address(body),
            "label": "agent/ceo",
            "body": body,
            "len": body.len() as i64,
            "ord": 1_i64,
            "stored_ms": 7_i64,
        })
        .await
        .expect("seed a legacy document");

    let metas = ContextStore::list(s.as_ref(), &id, "").await.expect("list");
    assert_eq!(
        metas.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(),
        ["agent/ceo"],
        "the scalar label is a claim"
    );

    // A second label on the same body folds into the set beside it.
    let addr = s
        .put(
            &id,
            ContextChunk {
                label: "agent/ops".to_string(),
                body: body.to_string(),
            },
        )
        .await
        .expect("put an identical body under a new label");
    let mut labels: Vec<String> = ContextStore::list(s.as_ref(), &id, "")
        .await
        .unwrap()
        .into_iter()
        .filter(|m| m.addr == addr)
        .map(|m| m.label)
        .collect();
    labels.sort();
    assert_eq!(labels, ["agent/ceo", "agent/ops"]);

    // Deleting the scalar claim leaves the set claim and the body.
    assert!(
        s.delete_label(&id, &addr, "agent/ceo")
            .await
            .expect("delete the scalar claim")
    );
    let after: Vec<String> = ContextStore::list(s.as_ref(), &id, "")
        .await
        .unwrap()
        .into_iter()
        .filter(|m| m.addr == addr)
        .map(|m| m.label)
        .collect();
    assert_eq!(after, ["agent/ops"]);
    s.peek(&id, &addr, None).await.expect("the body survives");

    // The rollback contract, read from the raw document: while any claim
    // remains the scalar `label` is present AND names a live claim. A
    // pre-#1300 binary reads that field with a hard error on absence, so
    // a document left without one would fail its whole `list`.
    let raw = s
        .collection("context_chunks")
        .find_one(doc! {"company_id": id.as_ref(), "addr": addr.as_ref()})
        .await
        .unwrap()
        .expect("the document survives");
    assert_eq!(
        raw.get_str("label").ok(),
        Some("agent/ops"),
        "the scalar must re-point at a surviving claim: {raw:?}"
    );

    // And the last claim takes the document with it.
    assert!(s.delete_label(&id, &addr, "agent/ops").await.unwrap());
    assert!(s.peek(&id, &addr, None).await.is_err());
    drop_db(&s).await;
}

/// A document carrying claims but no scalar `label` heals on the next
/// `put`, rather than gaining a claim and staying unreadable to a
/// pre-#1300 `list` (which reads that field with a hard error on
/// absence). `$setOnInsert` could not do this — it is skipped entirely on
/// a document that already exists — which is why `put` is a pipeline.
///
/// Seeded directly rather than raced for: `delete_label` no longer leaves
/// this state (it either deletes the document atomically or re-points the
/// scalar in the same update), so the only honest way to test the healing
/// is to construct the state a rollback or an older build could leave.
#[tokio::test]
async fn a_put_restores_a_missing_scalar_label_and_keeps_first_write_wins() {
    let Some(s) = store().await else { return };
    let id = CompanyId::new("acme");
    let body = "a document that lost its scalar label";
    s.collection("context_chunks")
        .insert_one(doc! {
            "company_id": id.as_ref(),
            "addr": content_address(body),
            "body": body,
            "len": body.len() as i64,
            "ord": 1_i64,
            "stored_ms": 7_i64,
            "labels": [],
        })
        .await
        .expect("seed a scalar-less document");

    let addr = s
        .put(
            &id,
            ContextChunk {
                label: "agent/ops".to_string(),
                body: body.to_string(),
            },
        )
        .await
        .expect("put onto the scalar-less document");

    let raw = s
        .collection("context_chunks")
        .find_one(doc! {"company_id": id.as_ref(), "addr": addr.as_ref()})
        .await
        .unwrap()
        .expect("the document is still there");
    assert_eq!(
        raw.get_str("label").ok(),
        Some("agent/ops"),
        "the write restores the scalar it found missing: {raw:?}"
    );
    assert_eq!(doc_labels(&raw), ["agent/ops"], "and claims it once");
    assert_eq!(
        raw.get_i64("stored_ms").ok(),
        Some(7),
        "first-write-wins still holds for the fields that were present"
    );
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_deep_trace_store() {
    let Some(s) = store().await else { return };
    conformance::assert_deep_trace_store(s.clone()).await;
    drop_db(&s).await;
}

/// The prune ranks by RUN and keeps the newest `MAX` runs whole — and a run
/// that fell past the cap survives once it is written to again. The
/// conformance suite never crosses the cap, so the aggregation the prune
/// uses to rank, and the re-verification that spares a refreshed run, are
/// exercised here against the real server.
#[tokio::test]
async fn deep_trace_prune_keeps_the_newest_runs_and_spares_a_refreshed_one() {
    use crate::ports::deep_trace::DeepTraceStore;
    use crate::ports::deep_trace::{
        MAX_DEEP_RUNS_PER_COMPANY, RunStepDetailRecord, TurnStepDetail,
    };
    let Some(s) = store().await else { return };
    let company = CompanyId::new("pruner");
    let detail = |run: &str, seq: u32, at: u64, reasoning: &str| RunStepDetailRecord {
        run_id: run.to_string(),
        step_seq: seq,
        at_millis: at,
        detail: TurnStepDetail {
            reasoning: Some(reasoning.to_string()),
            ..TurnStepDetail::default()
        },
    };
    // Fill past the cap with two rows per run, so the prune must drop whole
    // runs rather than tear them.
    let cap = MAX_DEEP_RUNS_PER_COMPANY;
    for i in 0..cap + 2 {
        let run = format!("r{i:03}");
        s.append_step_detail(&company, &detail(&run, 0, (i * 2) as u64, "first"))
            .await
            .unwrap();
        s.append_step_detail(&company, &detail(&run, 1, (i * 2 + 1) as u64, "second"))
            .await
            .unwrap();
    }
    // The two oldest runs fell past the cap and are gone whole…
    assert!(
        s.list_step_details(&company, "r000")
            .await
            .unwrap()
            .is_empty(),
        "r000 ranked oldest and must be pruned"
    );
    assert!(
        s.list_step_details(&company, "r001")
            .await
            .unwrap()
            .is_empty()
    );
    // …and every surviving run kept both rows.
    for i in 2..cap + 2 {
        let run = format!("r{i:03}");
        assert_eq!(
            s.list_step_details(&company, &run).await.unwrap().len(),
            2,
            "{run} must survive whole"
        );
    }
    // A run that already ranked stale is written to again — the concurrent
    // refresh the delete's re-verification exists to protect. The next
    // append's prune must keep it, not delete what it just received.
    s.append_step_detail(
        &company,
        &detail("r000", 2, 1_000_000, "refreshed after ranking stale"),
    )
    .await
    .unwrap();
    let refreshed = s.list_step_details(&company, "r000").await.unwrap();
    assert_eq!(
        refreshed.len(),
        1,
        "a refreshed run is not deleted by the prune that follows"
    );
    assert_eq!(
        refreshed[0].detail.reasoning.as_deref(),
        Some("refreshed after ranking stale"),
        "the refreshed detail is the one that survives"
    );
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_run_store_workflow_join() {
    let Some(s) = store().await else { return };
    conformance::assert_run_store_workflow_join(s.clone()).await;
    drop_db(&s).await;
}

/// The same search semantics as every other backend. This is the production
/// backend, so this is the row that matters most.
#[tokio::test]
async fn conformance_context_search_ranking() {
    let Some(s) = store().await else { return };
    conformance::assert_context_search_ranking(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_journal_store() {
    let Some(s) = store().await else { return };
    conformance::assert_journal_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_journal_import() {
    let Some(s) = store().await else { return };
    conformance::assert_journal_import(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_run_store() {
    let Some(s) = store().await else { return };
    conformance::assert_run_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_workflow_revision_store() {
    let Some(s) = store().await else { return };
    conformance::assert_workflow_revision_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_run_reaper() {
    let Some(s) = store().await else { return };
    conformance::assert_run_reaper(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_schedule_fire_store() {
    let Some(s) = store().await else { return };
    conformance::assert_schedule_fire_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_usage_meter() {
    let Some(s) = store().await else { return };
    conformance::assert_usage_meter(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_usage_retention() {
    let Some(s) = store().await else { return };
    conformance::assert_usage_retention(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_skill_state_store() {
    let Some(s) = store().await else { return };
    conformance::assert_skill_state_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_read_state_store() {
    let Some(s) = store().await else { return };
    conformance::assert_read_state_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_notification_store() {
    let Some(s) = store().await else { return };
    conformance::assert_notification_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_workspace_store() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conformance_workspace_conditional_write() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_conditional_write(s.clone(), s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conformance_workspace_revision_mutations() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_revision_mutations(s.clone(), s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_workspace_sibling_names() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_sibling_names(s.clone()).await;
    drop_db(&s).await;
}

/// The reason issue #553 could not defer the Mongo backend: hosted tenants
/// run MongoDB, and this is the only lane where the GridFS path — and the
/// 17 MiB case that proves the 16 MB BSON document cap is not in play —
/// actually executes.
#[tokio::test]
async fn conformance_workspace_binary_store() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_binary_store(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_workspace_read_capped() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_read_capped(s.clone()).await;
    drop_db(&s).await;
}

/// Issue #759. The only lane where the folder-claim primitive's contention
/// case runs against the partial unique index that actually decides it —
/// this backend has neither a lock nor a transaction to fall back on.
#[tokio::test]
async fn conformance_workspace_folder_claims() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_folder_claims(s.clone()).await;
    drop_db(&s).await;
}

#[tokio::test]
async fn conformance_workspace_create_rejects_an_absent_or_foreign_parent() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_create_rejects_an_absent_or_foreign_parent(s.clone()).await;
    drop_db(&s).await;
}

/// Issue #1839: the adoption lease, on the backend that can only *narrow*
/// Race 1 — the `$set` an adoption writes is what a later `delete_if_empty`
/// reads, so the sequential contract (mark, then refuse) still holds even
/// though a truly concurrent delete can still miss an in-flight `$set`.
#[tokio::test]
async fn conformance_workspace_adoption_lease() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_adoption_lease(s.clone()).await;
    drop_db(&s).await;
}

/// Issue #887's no-torn-read contract, on the other backend hosted tenants
/// run. A document replace is atomic, so this passed before the `fs` fix
/// and passes after — the contract is the port's, not one backend's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conformance_workspace_read_never_tears() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_read_never_tears(s.clone()).await;
    drop_db(&s).await;
}

/// The stat-then-open race fixed in the `fs` backend's `read_capped`.
/// This backend measures and reads under one aggregation and passes on
/// both sides of that fix — the contract is the port's, not one backend's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conformance_workspace_read_capped_race() {
    let Some(s) = store().await else { return };
    conformance::assert_workspace_read_capped_race(s.clone()).await;
    drop_db(&s).await;
}

/// The sqlite pin's mirror, on the other backend hosted tenants run
/// (issue #700).
///
/// Both are needed: the guard exists because a backend accepts a node name
/// carrying a path separator, and "accepts it" is a fact about each backend
/// rather than about the port. A folder holding only such a child has no
/// renderable path to it, reads as empty by every path-shaped measure, and
/// would still lose it to the port's recursive `delete` (issue #671) — so
/// the sweep must leave that folder standing here too.
#[tokio::test]
async fn workspace_sweep_keeps_a_folder_whose_only_child_has_no_renderable_path() {
    use crate::company::workspace_sweep::sweep_empty_agent_folders;
    use crate::ports::workspace::{NodeKind, WorkspaceNode, WorkspaceOrigin, WorkspaceStore};

    let Some(s) = store().await else { return };
    let company = CompanyId::new("acme");
    let node = |id: &str, name: &str, kind, parent: Option<&str>| WorkspaceNode {
        id: id.to_string(),
        name: name.to_string(),
        kind,
        parent_id: parent.map(str::to_string),
        updated_at_millis: 1,
        created_by: WorkspaceOrigin::Seed,
        updated_by: WorkspaceOrigin::Seed,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    };

    for (id, name, kind, parent, body) in [
        ("root", "Agents", NodeKind::Folder, None, None),
        ("ghost", "ceo", NodeKind::Folder, Some("root"), None),
        ("empty", "cto", NodeKind::Folder, Some("root"), None),
        (
            "hidden",
            "q/r.md",
            NodeKind::File,
            Some("ghost"),
            Some("# quarterly"),
        ),
    ] {
        WorkspaceStore::create(s.as_ref(), &company, &node(id, name, kind, parent), body)
            .await
            .expect("mongodb accepts these names — that is why the guard exists");
    }

    let removed = sweep_empty_agent_folders(s.as_ref(), &company, false)
        .await
        .unwrap();

    assert_eq!(
        removed.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        vec!["empty"],
        "only the genuinely empty folder may go"
    );
    let ids: Vec<String> = WorkspaceStore::tree(s.as_ref(), &company)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert!(
        ids.contains(&"ghost".to_string()) && ids.contains(&"hidden".to_string()),
        "the folder and its unaddressable child must both survive, got {ids:?}"
    );
    drop_db(&s).await;
}

#[tokio::test]
async fn durable_ownership_round_trip() {
    let Some(s) = store().await else { return };
    let id = CompanyId::new("acme");
    s.set_owner(&id, "tenant-a").await.expect("set owner");
    s.set_owner(&id, "tenant-b").await.expect("update owner");
    let owners = s.owners().await.expect("owners");
    assert_eq!(owners, vec![(id.clone(), "tenant-b".to_string())]);
    s.remove_owner(&id).await.expect("remove owner");
    assert!(s.owners().await.expect("owners").is_empty());
    drop_db(&s).await;
}
