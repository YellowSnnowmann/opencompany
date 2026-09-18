use super::tests::*;
use super::tests_create_1::*;
use super::*;

/// Issue #1801, Fix B: a folder create that slips past the up-front
/// duplicate check because the folder appeared *after* the snapshot was
/// read — the stale-snapshot race — adopts the folder already there rather
/// than minting a rival sibling under one name. Routing the create through
/// the store's atomic adopt-or-create is what closes the window the
/// tool-level pre-check cannot.
#[tokio::test]
async fn create_folder_adopts_a_racing_twin_instead_of_duplicating() {
    let (_dir, ops) = seeded("acme").await;
    let id = CompanyId::new("acme");
    crate::company::workspace_scaffold::ensure_workspace_scaffold(ops.as_ref(), &id)
        .await
        .unwrap();
    let home =
        crate::company::workspace_scaffold::ensure_agent_folder(ops.as_ref(), &id, TEST_AGENT)
            .await
            .unwrap();
    // The twin a racing publisher already committed to the store — present
    // for real, but hidden from the snapshot this call will read.
    let plans = ops
        .adopt_or_create_folder(&id, Some(&home), "plans", agent_origin())
        .await
        .unwrap()
        .into_node()
        .id;

    let stale: Arc<dyn WorkspaceStore> = Arc::new(ProxyStore::hiding(ops.clone(), &plans));
    let tool = WorkspaceCreateTool::new(ws(stale, id.clone()));
    let out = tool
        .execute(json!({ "path": "agents/ceo/plans", "kind": "folder" }))
        .await
        .unwrap();

    assert!(!out.is_error, "{}", text(&out));
    assert!(
        text(&out).contains("already exists"),
        "an adopted folder must not be reported as freshly created: {}",
        text(&out)
    );

    let siblings = ops
        .tree(&id)
        .await
        .unwrap()
        .into_iter()
        .filter(|n| n.name == "plans" && n.parent_id.as_deref() == Some(home.as_str()))
        .count();
    assert_eq!(
        siblings, 1,
        "the race must adopt the existing folder, never duplicate it"
    );
}

/// Issue #1801, Fix A: a note create that fails after this call minted the
/// agent's own home must not leave an empty `agents/<id>/` behind. The
/// `ProxyStore` mints the home for real, then refuses the note, and the
/// rollback sweeps the home it just made rather than stranding it for the
/// Repair button.
#[tokio::test]
async fn a_failed_note_create_does_not_orphan_the_minted_home() {
    let (_dir, ops) = seeded("acme").await;
    let id = CompanyId::new("acme");
    crate::company::workspace_scaffold::ensure_workspace_scaffold(ops.as_ref(), &id)
        .await
        .unwrap();

    let refusing: Arc<dyn WorkspaceStore> = Arc::new(ProxyStore::refusing_notes(ops.clone()));
    let tool = WorkspaceCreateTool::new(ws(refusing, id.clone()));
    let out = tool
        .execute(json!({
            "path": "agents/ceo/brief.md",
            "kind": "file",
            "content": "# Brief",
        }))
        .await
        .unwrap();

    assert!(
        out.is_error,
        "the refused note create must surface an error: {}",
        text(&out)
    );

    let tree = ops.tree(&id).await.unwrap();
    assert!(
        !tree
            .iter()
            .any(|n| n.name == TEST_AGENT && n.kind == NodeKind::Folder),
        "the empty home minted for the refused note was orphaned: {tree:?}"
    );
    assert!(
        tree.iter()
            .any(|n| n.name == AGENTS_ROOT && n.parent_id.is_none()),
        "the scaffolded root must survive: {tree:?}"
    );
}

/// Create never overwrites. A path that already resolves is refused with
/// the note left byte-identical — the failure mode this tool must never
/// have, since it carries no compare-and-swap token to protect one.
#[tokio::test]
async fn create_refuses_a_path_that_already_exists_and_changes_nothing() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({
            "path": "standards/engineering-standards.md",
            "kind": "file",
            "content": "# Mine now",
        }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(text(&out).contains(WORKSPACE_WRITE_TOOL), "{}", text(&out));

    let (_, body) = store.read(&id, "n-eng").await.unwrap().unwrap();
    assert_eq!(
        body, "# Engineering\nReview every PR.",
        "the existing note was clobbered"
    );
    assert_eq!(store.tree(&id).await.unwrap().len(), 3, "a node was added");
}

/// The reserved-root case of the rule above, called out because it is the
/// one that matters most: identity in `agents/` is by path, so an agent
/// that could mint a rival root named `Agents` would make every
/// `agents/...` path permanently ambiguous — for itself, for its teammates
/// and for the provisioner.
#[tokio::test]
async fn create_cannot_mint_a_rival_agents_root() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    crate::company::workspace_scaffold::ensure_workspace_scaffold(store.as_ref(), &id)
        .await
        .unwrap();
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({ "path": "Agents", "kind": "folder" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert_eq!(
        store
            .tree(&id)
            .await
            .unwrap()
            .iter()
            // Case-insensitively: the reserved root is `agents/`, and a
            // company that predates the naming rule carries `Agents/`.
            // Either way there must be exactly one — a rival root under the
            // other spelling is the failure this test exists to catch.
            .filter(|n| n.name.eq_ignore_ascii_case(AGENTS_ROOT) && n.parent_id.is_none())
            .count(),
        1,
    );
}

/// One node per call: a missing parent is an actionable refusal, not a
/// silent `mkdir -p`. A single typo in a deep path would otherwise grow a
/// whole phantom subtree nobody asked for.
#[tokio::test]
async fn create_refuses_a_missing_parent_and_says_what_to_do() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({ "path": "playbooks/Launch/Checklist.md", "kind": "file" }))
        .await
        .unwrap();
    assert!(out.is_error);
    let message = text(&out);
    assert!(message.contains("playbooks/Launch"), "{message}");
    assert!(message.contains(WORKSPACE_CREATE_TOOL), "{message}");
    assert!(message.contains("folder"), "{message}");
    assert_eq!(
        store.tree(&id).await.unwrap().len(),
        3,
        "a refused create must not have made intermediate folders"
    );
}

/// A note is not a folder, so nothing can be created inside one.
#[tokio::test]
async fn create_refuses_a_parent_that_is_a_note() {
    let (_dir, store) = seeded("acme").await;
    let tool = WorkspaceCreateTool::new(ws(store, CompanyId::new("acme")));
    let out = tool
        .execute(json!({ "path": "README.md/child.md", "kind": "file" }))
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(text(&out).contains("not a folder"), "{}", text(&out));
}

/// The same traversal rules as every other tool, and applied on the
/// argument's *shape* before anything resolves.
#[tokio::test]
async fn create_refuses_traversal_shaped_paths() {
    let (_dir, store) = seeded("acme").await;
    let tool = WorkspaceCreateTool::new(ws(store.clone(), CompanyId::new("acme")));
    for path in ["../escape.md", "standards/../../etc/passwd", "./x.md", ".."] {
        let out = tool
            .execute(json!({ "path": path, "kind": "file" }))
            .await
            .unwrap();
        assert!(out.is_error, "path {path:?} must be refused");
    }
    assert_eq!(
        store.tree(&CompanyId::new("acme")).await.unwrap().len(),
        3,
        "a traversal-shaped path created something"
    );
}

/// A body an agent could not read back in full must never be created —
/// the next `workspace_write` on it would be refused as oversized, leaving
/// a note nobody but the operator can ever touch again.
#[tokio::test]
async fn create_refuses_a_body_over_the_write_cap() {
    let (_dir, store) = seeded("acme").await;
    let tool = WorkspaceCreateTool::new(ws(store.clone(), CompanyId::new("acme")));
    let out = tool
        .execute(json!({
            "path": "standards/Huge.md",
            "kind": "file",
            "content": "x".repeat(MAX_WRITE_BYTES + 1),
        }))
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(text(&out).contains("over the"), "{}", text(&out));
    assert_eq!(store.tree(&CompanyId::new("acme")).await.unwrap().len(), 3);
}

/// Bad arguments answer with the fix, not with a stack of nulls.
#[tokio::test]
async fn create_rejects_a_missing_or_unknown_kind() {
    let (_dir, store) = seeded("acme").await;
    let tool = WorkspaceCreateTool::new(ws(store, CompanyId::new("acme")));
    for args in [
        json!({ "path": "standards/x.md" }),
        json!({ "path": "standards/x.md", "kind": "note" }),
        json!({ "kind": "file" }),
        json!({ "path": "standards/x", "kind": "folder", "content": "body" }),
    ] {
        let out = tool.execute(args.clone()).await.unwrap();
        assert!(out.is_error, "{args} must be refused");
    }
}

/// The acceptance criterion issue #551 is actually about: one agent's
/// output is another agent's input. Agent A creates, agent B — a different
/// `CompanyWorkspace`, its own tool instances — lists and reads it.
#[tokio::test]
async fn one_agent_creates_and_another_reads_it_back() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");

    let author = CompanyWorkspace::new(store.clone(), id.clone(), "cmo".to_string());
    let out = WorkspaceCreateTool::new(author)
        .execute(json!({
            "path": "standards/Brand voice.md",
            "kind": "file",
            "content": "# Brand voice\nWarm, plain, specific.",
        }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));

    let reader = CompanyWorkspace::new(store, id, "engineer".to_string());
    let listing = text(
        &WorkspaceListTool::new(reader.clone())
            .execute(json!({}))
            .await
            .unwrap(),
    );
    assert!(listing.contains("standards/brand-voice.md"), "{listing}");

    let read = text(
        &WorkspaceReadTool::new(reader)
            .execute(json!({ "path": "standards/Brand voice.md" }))
            .await
            .unwrap(),
    );
    assert!(read.contains("Warm, plain, specific."), "{read}");
}

/// A write restamps `updated_by` with the writer and leaves `created_by`
/// alone, so "who made this" survives someone else editing it.
#[tokio::test]
async fn a_write_restamps_the_writer_and_preserves_the_creator() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");

    let created = WorkspaceCreateTool::new(CompanyWorkspace::new(
        store.clone(),
        id.clone(),
        "cmo".to_string(),
    ))
    .execute(json!({ "path": "standards/Voice.md", "kind": "file", "content": "v1" }))
    .await
    .unwrap();
    assert!(!created.is_error, "{}", text(&created));

    let node = store
        .tree(&id)
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.name == "voice.md")
        .unwrap();

    let out = WorkspaceWriteTool::new(ws(store.clone(), id.clone()))
        .execute(json!({
            "path": "standards/Voice.md",
            "content": "v2",
            "expected_updated_at": node.updated_at_millis,
        }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));

    let after = store
        .tree(&id)
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.name == "voice.md")
        .unwrap();
    assert_eq!(
        after.created_by,
        WorkspaceOrigin::Agent {
            id: "cmo".to_string()
        },
        "the creator must survive another agent's edit"
    );
    assert_eq!(after.updated_by, agent_origin());
}
