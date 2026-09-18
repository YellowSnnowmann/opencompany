//! Runtime tests: a wholly refused blocked agent node prunes its checkpoint lineage.

/// A blocked agent node whose whole gated-call batch is refused starts no
/// continuation — the blocked-node twin of `resume_run`'s all-denied case
/// — and, since PR #1991's review (`3903797619`), must also stop leaving
/// that lineage's checkpoint on disk forever: nothing else ever comes back
/// for a wholly refused block's thread id.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_wholly_refused_blocked_node_prunes_its_checkpoint_lineage() {
    use tinyflows::graph::Checkpointer;

    let home = tempfile::tempdir().expect("home");
    let manifest: crate::company::CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n[policy]\n\
         mode = \"full\"\n",
    )
    .expect("manifest");
    let mut runtime = crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
        .build()
        .await
        .expect("runtime");
    let checkpoints = std::sync::Arc::new(
        crate::workflows::checkpoint_store::WorkflowCheckpointStore::new(
            home.path().join("checkpoints"),
        ),
    );
    checkpoints
        .put(tinyflows::graph::Checkpoint {
            thread_id: "blocked-thread".to_string(),
            checkpoint_id: "c1".to_string(),
            run_id: Some("blocked-thread".to_string()),
            parent_checkpoint_id: None,
            namespace: Vec::new(),
            state: serde_json::json!({}),
            next_nodes: vec![tinyflows::graph::ids::NodeId::new("agent")],
            completed_tasks: Vec::new(),
            pending_writes: Vec::new(),
            interrupts: Vec::new(),
            pending_activations: None,
            barrier_arrivals: Vec::new(),
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("seed checkpoint");
    runtime.set_workflow_checkpoints(checkpoints.clone());

    let turn = "blocked-turn";
    runtime.blocked_nodes.arm_checkpointed(
        turn,
        "gated",
        &serde_json::json!({}),
        &crate::ports::types::StartedBy::Operator,
        Some("blocked-thread"),
        None,
    );

    runtime
        .resume_blocked_agent_node(
            &crate::ports::types::ApprovalId::new("call-1"),
            turn,
            Vec::new(),
        )
        .await
        .expect("an all-refused block does not error");

    let remaining = checkpoints
        .get_thread("blocked-thread")
        .await
        .expect("checkpoint read");
    assert!(
        remaining.is_empty(),
        "a wholly refused blocked node starts no continuation, so its checkpoint lineage \
         must be pruned: {remaining:?}"
    );
}
