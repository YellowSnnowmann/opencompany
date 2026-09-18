use super::tests::*;
use super::*;

// -- binary nodes (issue #553) ------------------------------------------

/// A binary node, created through the port so its size and digest are the
/// store's own.
async fn with_payload(company: &str) -> (tempfile::TempDir, Arc<dyn WorkspaceStore>) {
    let (dir, ops) = seeded(company).await;
    let id = CompanyId::new(company);
    let node = WorkspaceNode {
        mime: Some("image/png".to_string()),
        ..file("n-img", "hero.png", None)
    };
    ops.create_binary(&id, &node, &[0x89, b'P', b'N', b'G', 0xff, 0xfe])
        .await
        .expect("payload");
    (dir, ops)
}

/// `workspace_read` of a payload is a **success** carrying metadata — not
/// an error, and never the bytes. The agent asked a reasonable question and
/// gets a complete answer; the bytes would be unusable to it and would blow
/// the result budget (issue #417) that the text cap exists to defend.
#[tokio::test]
async fn reading_a_binary_node_returns_metadata_and_never_bytes() {
    let (_dir, store) = with_payload("acme").await;
    let tool = WorkspaceReadTool::new(ws(store, CompanyId::new("acme")));

    let result = tool.execute(json!({"id": "n-img"})).await.unwrap();
    assert!(
        !result.is_error,
        "describing a payload is an answer, not a failure"
    );
    let out = text(&result);
    assert!(out.contains("image/png"), "{out}");
    assert!(out.contains("6 bytes"), "the store's size: {out}");
    let (_, sha) = crate::ports::workspace::blob_metadata(&[0x89, b'P', b'N', b'G', 0xff, 0xfe]);
    assert!(out.contains(&sha), "the store's digest: {out}");
    // The payload's own bytes must not appear, in any rendering.
    assert!(!out.contains("PNG"), "the bytes must not be echoed: {out}");
    assert!(
        !out.contains("BEGIN WORKSPACE NOTE"),
        "a payload is not fenced as prose: {out}"
    );
}

/// `workspace_write` refuses a payload, and says what the file actually is.
/// The store refuses this too — this layer exists to make the refusal
/// legible to a model rather than to be the guarantee.
#[tokio::test]
async fn writing_over_a_binary_node_is_refused_with_a_reason() {
    let (_dir, store) = with_payload("acme").await;
    let tool = WorkspaceWriteTool::new(ws(store.clone(), CompanyId::new("acme")));

    let result = tool
        .execute(json!({
            "id": "n-img",
            "content": "# not an image",
            "expected_updated_at": 2_000,
        }))
        .await
        .unwrap();
    assert!(result.is_error, "{}", text(&result));
    let out = text(&result);
    assert!(out.contains("image/png"), "{out}");

    // And the payload is untouched.
    let (node, _) = store
        .read_bytes(&CompanyId::new("acme"), "n-img")
        .await
        .unwrap()
        .expect("still a payload");
    assert_eq!(node.size, Some(6));
}

/// A manifest that declares no write-scoped `context` entry is unconfined
/// — `workspace_write` reaches anywhere in the tree, exactly as before this
/// existed. This is the regression the opt-in confinement must not cause.
#[tokio::test]
async fn workspace_write_stays_unconfined_by_default() {
    let (_dir, store) = seeded("acme").await;
    let tool = WorkspaceWriteTool::new(ws(store.clone(), CompanyId::new("acme")));

    let result = tool
        .execute(json!({
            "id": "n-eng",
            "content": "# Engineering\nRevised.",
            "expected_updated_at": 2_000,
        }))
        .await
        .unwrap();
    assert!(!result.is_error, "{}", text(&result));
}

/// A manifest that declares a write-scoped `context` entry confines
/// `workspace_write` to exactly those paths — a path outside the scope is
/// refused, and the tree is untouched.
#[tokio::test]
async fn workspace_write_refuses_a_path_outside_the_declared_write_scope() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let workspace =
        ws(store.clone(), id.clone()).with_write_scope(Some(vec!["Somewhere/Else.md".to_string()]));
    let tool = WorkspaceWriteTool::new(workspace);

    let result = tool
        .execute(json!({
            "id": "n-eng",
            "content": "# Engineering\nRevised.",
            "expected_updated_at": 2_000,
        }))
        .await
        .unwrap();
    assert!(result.is_error, "{}", text(&result));
    let out = text(&result);
    assert!(out.contains("write scope"), "{out}");

    let (node, body) = store
        .read(&id, "n-eng")
        .await
        .unwrap()
        .expect("still there");
    assert_eq!(node.updated_at_millis, 2_000, "untouched");
    assert_eq!(body, "# Engineering\nReview every PR.");
}

/// A path that *is* in the declared write scope still succeeds.
#[tokio::test]
async fn workspace_write_allows_a_path_inside_the_declared_write_scope() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let workspace = ws(store.clone(), id.clone())
        .with_write_scope(Some(vec!["standards/engineering-standards.md".to_string()]));
    let tool = WorkspaceWriteTool::new(workspace);

    let result = tool
        .execute(json!({
            "id": "n-eng",
            "content": "# Engineering\nRevised.",
            "expected_updated_at": 2_000,
        }))
        .await
        .unwrap();
    assert!(!result.is_error, "{}", text(&result));
}

/// A write-scoped agent may still create inside its own `agents/<id>/`
/// home — the scope narrows the shared tree, not the ability to produce
/// and revise its own work.
#[tokio::test]
async fn a_write_scoped_agent_can_still_create_in_its_own_home() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let workspace =
        ws(store.clone(), id.clone()).with_write_scope(Some(vec!["Somewhere/Else.md".to_string()]));
    let tool = WorkspaceCreateTool::new(workspace);

    let result = tool
        .execute(json!({
            "path": "agents/ceo/Notes.md",
            "kind": "file",
            "content": "# Notes"
        }))
        .await
        .unwrap();
    assert!(!result.is_error, "{}", text(&result));
}

/// The other half: `workspace_create` at a shared path outside scope is
/// refused before anything is written.
#[tokio::test]
async fn workspace_create_refuses_a_path_outside_the_declared_write_scope() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let workspace =
        ws(store.clone(), id.clone()).with_write_scope(Some(vec!["Somewhere/Else.md".to_string()]));
    let tool = WorkspaceCreateTool::new(workspace);

    let result = tool
        .execute(json!({
            "path": "standards/New standard.md",
            "kind": "file",
            "content": "# New"
        }))
        .await
        .unwrap();
    assert!(result.is_error, "{}", text(&result));
    assert!(text(&result).contains("write scope"));

    let tree = store.tree(&id).await.unwrap();
    assert!(!tree.iter().any(|n| n.name == "New standard.md"));
}

/// The two boundaries compose, and the operator-only one wins.
///
/// A declared write scope narrows what an agent may touch; it can never
/// widen it into `secrets/`. Naming that subtree explicitly in a scope is
/// still refused, and with the *neutral* refusal rather than the
/// scope-shaped one — a scoped agent must not be able to tell an
/// operator-only path from an out-of-scope one, which is the whole point of
/// checking the hidden root first.
#[tokio::test]
async fn a_declared_write_scope_cannot_reach_into_the_operator_only_subtree() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let workspace =
        ws(store.clone(), id.clone()).with_write_scope(Some(vec!["secrets/keys.md".to_string()]));
    let tool = WorkspaceCreateTool::new(workspace);

    let result = tool
        .execute(json!({
            "path": "secrets/keys.md",
            "kind": "file",
            "content": "agent value"
        }))
        .await
        .unwrap();
    let out = text(&result);
    assert!(result.is_error, "{out}");
    assert!(out.contains("not available to agents"), "{out}");
    assert!(
        !out.contains("write scope"),
        "the refusal must not differ from an ordinary agent's: {out}"
    );

    let tree = store.tree(&id).await.unwrap();
    assert!(!tree.iter().any(|n| n.name == "keys.md"));
}

/// The unconfined default still gets the operator-only refusal, and a
/// write-scoped agent still keeps its own home inside the visible tree —
/// neither boundary swallows the other.
#[tokio::test]
async fn the_operator_only_refusal_precedes_the_write_scope_check() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");

    // Unconfined: `secrets/` is refused on the hidden-root rule alone.
    let unconfined = WorkspaceCreateTool::new(ws(store.clone(), id.clone()))
        .execute(json!({"path": "Secrets/new.md", "kind": "file", "content": "x"}))
        .await
        .unwrap();
    assert!(unconfined.is_error, "{}", text(&unconfined));
    assert!(
        text(&unconfined).contains("not available to agents"),
        "{}",
        text(&unconfined)
    );

    // Scoped: the always-writable home is unaffected by the hidden root.
    let scoped = WorkspaceCreateTool::new(
        ws(store.clone(), id.clone()).with_write_scope(Some(vec!["Somewhere/Else.md".to_string()])),
    )
    .execute(json!({
        "path": format!("{AGENTS_ROOT}/{TEST_AGENT}/Brief.md"),
        "kind": "file",
        "content": "# Brief"
    }))
    .await
    .unwrap();
    assert!(!scoped.is_error, "{}", text(&scoped));
}

/// The listing marks a payload, so an agent never spends a `workspace_read`
/// call to discover that a file is not text.
#[tokio::test]
async fn the_listing_marks_binary_entries_with_their_type_and_size() {
    let (_dir, store) = with_payload("acme").await;
    let tool = WorkspaceListTool::new(ws(store, CompanyId::new("acme")));

    let out = text(&tool.execute(json!({})).await.unwrap());
    let line = out
        .lines()
        .find(|l| l.contains("hero.png"))
        .expect("the payload is listed");
    assert!(line.contains("image/png"), "{line}");
    assert!(line.contains("6B"), "{line}");

    // A prose note carries no such marker — presence of the type is the
    // discriminator, so it must not appear on text entries.
    let note = out
        .lines()
        .find(|l| l.contains("readme.md"))
        .expect("the note is listed");
    assert!(!note.contains("image/"), "{note}");
}
