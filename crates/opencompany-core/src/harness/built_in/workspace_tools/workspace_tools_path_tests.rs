use super::tests::*;
use super::*;

// -- path rendering and validation --------------------------------------

#[test]
fn paths_render_from_the_ancestor_chain() {
    let nodes = vec![
        folder("a", "standards", None),
        file("b", "engineering-standards.md", Some("a")),
        file("c", "readme.md", None),
    ];
    let index = PathIndex::build(nodes);
    assert_eq!(index.by_id["b"].path, "standards/engineering-standards.md");
    assert_eq!(index.by_id["c"].path, "readme.md");
    assert_eq!(index.unaddressable, 0);
}

#[test]
fn the_agent_index_omits_the_secrets_subtree_by_path_and_id() {
    let nodes = vec![
        folder("secret-root", "secrets", None),
        file("secret-note", "token.md", Some("secret-root")),
        file("public-note", "secrets-old.md", None),
    ];

    let index = PathIndex::build_for_agent(nodes);

    assert!(!index.by_id.contains_key("secret-root"));
    assert!(!index.by_id.contains_key("secret-note"));
    assert_eq!(index.by_id["public-note"].path, "secrets-old.md");
    assert_eq!(index.unaddressable, 0, "hidden is not malformed");
}

#[tokio::test]
async fn secrets_are_operator_visible_but_absent_from_every_agent_workspace_tool() {
    let (_dir, store) = seeded("acme").await;
    let company = CompanyId::new("acme");
    store
        .create(&company, &folder("secret-root", "secrets", None), None)
        .await
        .unwrap();
    store
        .create(
            &company,
            &file("secret-note", "keys.md", Some("secret-root")),
            Some("launch-codeword-umbra"),
        )
        .await
        .unwrap();
    let workspace = ws(store.clone(), company.clone());

    let listed = text(
        &WorkspaceListTool::new(workspace.clone())
            .execute(json!({}))
            .await
            .unwrap(),
    );
    assert!(!listed.contains("secrets"), "{listed}");
    assert!(!listed.contains("secret-note"), "{listed}");

    for args in [
        json!({"path": "secrets/keys.md"}),
        json!({"id": "secret-note"}),
    ] {
        let result = WorkspaceReadTool::new(workspace.clone())
            .execute(args)
            .await
            .unwrap();
        assert!(result.is_error, "{}", text(&result));
        assert!(!text(&result).contains("launch-codeword-umbra"));
    }

    let searched = text(
        &WorkspaceSearchTool::new(workspace.clone())
            .execute(json!({"query": "launch-codeword-umbra"}))
            .await
            .unwrap(),
    );
    assert!(searched.contains("No workspace notes match"), "{searched}");
    assert!(!searched.contains("secret-note"), "{searched}");

    let write = WorkspaceWriteTool::new(workspace.clone())
        .execute(json!({
            "id": "secret-note",
            "content": "overwritten",
            "expected_updated_at": 2_000,
        }))
        .await
        .unwrap();
    assert!(write.is_error, "{}", text(&write));

    let rename = WorkspaceRenameTool::new(workspace.clone())
        .execute(json!({"id": "secret-note", "new_name": "public.md"}))
        .await
        .unwrap();
    assert!(rename.is_error, "{}", text(&rename));

    let delete = WorkspaceDeleteTool::new(workspace.clone())
        .execute(json!({
            "id": "secret-note",
            "expected_updated_at": 2_000,
        }))
        .await
        .unwrap();
    assert!(delete.is_error, "{}", text(&delete));

    let create = WorkspaceCreateTool::new(workspace)
        .execute(json!({
            "path": "Secrets/new.md",
            "kind": "file",
            "content": "agent value",
        }))
        .await
        .unwrap();
    assert!(create.is_error, "{}", text(&create));

    // Operator-facing helpers deliberately retain the complete tree.
    let operator = crate::company::workspace_search::search_workspace(
        store.as_ref(),
        &company,
        "launch-codeword-umbra",
        None,
        NonZeroUsize::MIN,
    )
    .await
    .unwrap();
    assert_eq!(operator.hits[0].path, "secrets/keys.md");
    let (_, body) = store.read(&company, "secret-note").await.unwrap().unwrap();
    assert_eq!(body, "launch-codeword-umbra");
}

/// A child excluded from the path maps must still be counted against its
/// parent, because "is this folder empty" decides whether a folder may be
/// handed to a port whose `delete` is recursive.
///
/// This asserts both measures at once, so the regression is explicit: the
/// path-prefix count that `workspace_delete` used to run sees **nothing**
/// under the folder, while `child_count` sees the child. A gate reading the
/// first would call this folder empty and delete an unbounded subtree that
/// was never counted, announced, or named on the approval card — the exact
/// outcome the module docs say recursion is refused to prevent.
///
/// The shapes here are creatable through the sqlite and mongodb backends;
/// only `fs` rejects them at creation (`reject_unsafe_name`), which is why
/// the tool layer has to stay closed against them on its own.
#[test]
fn an_unaddressable_child_is_still_counted_against_its_parent() {
    let nodes = vec![
        folder("f", "archive", None),
        // Name carries a separator: no renderable path, so absent from both
        // maps by design.
        file("hidden", "quarterly/report.md", Some("f")),
    ];
    let index = PathIndex::build(nodes);

    assert_eq!(index.unaddressable, 1, "the child must be excluded by path");
    assert!(
        !index.by_id.contains_key("hidden"),
        "an unaddressable node must not be reachable by id either"
    );

    // What the old gate measured: rendered paths beneath the folder.
    let prefix = format!("{}/", index.by_id["f"].path);
    let by_path_count: usize = index
        .by_path
        .iter()
        .filter(|(path, _)| path.starts_with(&prefix))
        .map(|(_, entries)| entries.len())
        .sum();
    assert_eq!(
        by_path_count, 0,
        "precondition: the path-shaped measure cannot see this child — that is the bug"
    );

    // What the gate measures now.
    assert_eq!(
        index.child_count.get("f").copied().unwrap_or_default(),
        1,
        "the structural measure must see a child the path rules exclude"
    );
}

/// A rename re-renders the path of every node under a folder, so the
/// ownership gate must see descendants the path maps omit. This is the
/// rename-side half of the emptiness test above: `entries_under` reads
/// `by_path`, which cannot see `hidden` at all, while a parent-id walk must
/// hand the gate exactly that node — and its own descendants too.
#[test]
fn a_folder_rename_sees_unaddressable_descendants() {
    let nodes = vec![
        folder("f", "archive", None),
        // Name carries a separator: no renderable path, absent from the
        // address maps, but still moved by a parent-id `rename_move`.
        file("hidden", "quarterly/report.md", Some("f")),
        // A grandchild under the unaddressable node is itself unaddressable
        // (its chain carries an illegal name), and must also be found.
        file("nested", "nested.md", Some("hidden")),
        // An ordinary addressable sibling stays included as before.
        file("plain", "plain.md", Some("f")),
    ];
    let index = PathIndex::build(nodes);

    assert_eq!(index.unaddressable, 2, "hidden and nested must be excluded");
    assert!(
        !index.by_id.contains_key("hidden") && !index.by_id.contains_key("nested"),
        "an unaddressable node must not be reachable by id either"
    );

    let mut subtree = index.subtree_ids("f");
    subtree.sort_unstable();
    assert_eq!(
        subtree,
        vec!["hidden", "nested", "plain"],
        "the walk must see the addressable child and both unaddressable ones"
    );
}

/// The parent-id walk used by the rename gate must terminate on a
/// hand-edited backing store that cycles, exactly as the path renderer's
/// depth limit does — a cycle inside a subtree would otherwise hang the
/// walk on a folder rename.
#[test]
fn subtree_ids_terminates_on_a_cycle() {
    // x ↔ y: each names the other as its parent, so no path exists for
    // either — but a parent-id walk from one of them must still finish.
    let cyclic = vec![folder("x", "X", Some("y")), folder("y", "Y", Some("x"))];
    let index = PathIndex::build(cyclic);
    let mut subtree = index.subtree_ids("x");
    subtree.sort_unstable();
    assert_eq!(
        subtree,
        vec!["x", "y"],
        "the visited set must keep the walk finite and still name both nodes"
    );
}

/// A [`WorkspaceStore`] returning a fixed tree, for ownership-gate shapes
/// the `fs` backend refuses to create: a name carrying a separator has no
/// renderable path, yet a parent-id rename still moves it, so the gate has
/// to decide on nodes no `FsOps`-seeded test can reach.
#[derive(Clone)]
struct FixedWorkspaceTree(Vec<WorkspaceNode>);

#[async_trait]
impl WorkspaceStore for FixedWorkspaceTree {
    async fn tree(&self, _company: &CompanyId) -> crate::Result<Vec<WorkspaceNode>> {
        Ok(self.0.clone())
    }
    async fn read(
        &self,
        _company: &CompanyId,
        _id: &str,
    ) -> crate::Result<Option<(WorkspaceNode, String)>> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn read_capped(
        &self,
        _company: &CompanyId,
        _id: &str,
        _max_bytes: u64,
    ) -> crate::Result<Option<(WorkspaceNode, String, u64)>> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn write_with_revision(
        &self,
        _company: &CompanyId,
        _id: &str,
        _content: &str,
        _author: WorkspaceOrigin,
        _expected_updated_at: Option<u64>,
    ) -> crate::Result<WorkspaceNode> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn create(
        &self,
        _company: &CompanyId,
        _node: &WorkspaceNode,
        _content: Option<&str>,
    ) -> crate::Result<()> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn adopt_or_create_folder(
        &self,
        _company: &CompanyId,
        _parent: Option<&str>,
        _name: &str,
        _origin: WorkspaceOrigin,
    ) -> crate::Result<crate::ports::workspace::FolderClaim> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn create_binary(
        &self,
        _company: &CompanyId,
        _node: &WorkspaceNode,
        _bytes: &[u8],
    ) -> crate::Result<WorkspaceNode> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn write_binary(
        &self,
        _company: &CompanyId,
        _id: &str,
        _bytes: &[u8],
        _mime: Option<&str>,
        _author: WorkspaceOrigin,
    ) -> crate::Result<WorkspaceNode> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn read_bytes(
        &self,
        _company: &CompanyId,
        _id: &str,
    ) -> crate::Result<Option<(WorkspaceNode, crate::ports::workspace::BlobStream)>> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn rename_move(
        &self,
        _company: &CompanyId,
        _id: &str,
        _name: Option<&str>,
        _parent: Option<Option<&str>>,
    ) -> crate::Result<WorkspaceNode> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn swap_files(
        &self,
        _company: &CompanyId,
        _expected_id: Option<&str>,
        _replacement_id: &str,
        _name: &str,
    ) -> crate::Result<Option<WorkspaceNode>> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn delete(&self, _company: &CompanyId, _id: &str) -> crate::Result<bool> {
        unreachable!("the ownership gate only reads the tree")
    }
    async fn is_empty(&self, _company: &CompanyId) -> crate::Result<bool> {
        unreachable!("the ownership gate only reads the tree")
    }
}

/// The rename half of the auto-tier exception must fail closed on an
/// unaddressable descendant. A folder holding an operator-authored node
/// whose name the path rules exclude (creatable through the sqlite and
/// mongodb backends, which do not run `reject_unsafe_name`) would still be
/// relocated by a parent-id `rename_move`, so the gate must park even
/// though `entries_under` cannot see the node.
#[tokio::test]
async fn rename_of_a_folder_with_an_unaddressable_operator_descendant_parks() {
    let company = CompanyId::new("acme");
    let own = WorkspaceOrigin::Agent {
        id: TEST_AGENT.to_string(),
    };
    let mut agent_folder = folder("f", "archive", None);
    agent_folder.created_by = own.clone();
    agent_folder.updated_by = own.clone();
    // Name carries a separator: no renderable path, operator-authored.
    let mut operator_hidden = file("hidden", "quarterly/report.md", Some("f"));
    operator_hidden.created_by = WorkspaceOrigin::Operator;
    operator_hidden.updated_by = WorkspaceOrigin::Operator;
    let store: Arc<dyn WorkspaceStore> =
        Arc::new(FixedWorkspaceTree(vec![agent_folder, operator_hidden]));

    let owned = mutation_is_owned_by_agent(
        &store,
        &company,
        TEST_AGENT,
        WORKSPACE_RENAME_TOOL,
        &serde_json::json!({ "id": "f" }),
    )
    .await;
    assert!(
        !owned,
        "an unaddressable operator-authored child must restore the approval gate"
    );
}

/// The same shape with the hidden child agent-authored stays inside the
/// exception — an agent's own tidying runs unattended even when one of its
/// notes has a name no path can render.
#[tokio::test]
async fn rename_of_a_folder_with_an_unaddressable_agent_descendant_runs() {
    let company = CompanyId::new("acme");
    let own = WorkspaceOrigin::Agent {
        id: TEST_AGENT.to_string(),
    };
    let mut agent_folder = folder("f", "archive", None);
    agent_folder.created_by = own.clone();
    agent_folder.updated_by = own.clone();
    let mut agent_hidden = file("hidden", "quarterly/report.md", Some("f"));
    agent_hidden.created_by = own.clone();
    agent_hidden.updated_by = own;
    let store: Arc<dyn WorkspaceStore> =
        Arc::new(FixedWorkspaceTree(vec![agent_folder, agent_hidden]));

    let owned = mutation_is_owned_by_agent(
        &store,
        &company,
        TEST_AGENT,
        WORKSPACE_RENAME_TOOL,
        &serde_json::json!({ "id": "f" }),
    )
    .await;
    assert!(
        owned,
        "an unaddressable descendant the agent itself authored stays within the exception"
    );
}

#[test]
fn a_dangling_or_cyclic_ancestor_chain_is_not_path_addressable() {
    // Parent id names a node that is not in the tree.
    let orphan = PathIndex::build(vec![file("b", "note.md", Some("missing"))]);
    assert_eq!(orphan.unaddressable, 1);
    assert!(orphan.by_id.is_empty());

    // A two-node cycle must terminate the walk rather than hang.
    let cycle = PathIndex::build(vec![
        folder("a", "A", Some("b")),
        folder("b", "B", Some("a")),
    ]);
    assert_eq!(cycle.unaddressable, 2);
}

/// The sqlite and mongodb backends do not run the `fs` backend's
/// `reject_unsafe_name` on create, so a separator-bearing or `..` name can
/// reach the tool layer. Such a node must never render a path that could be
/// resolved — it stays id-addressable only.
#[test]
fn a_name_that_is_not_a_legal_segment_is_not_path_addressable() {
    for name in ["..", ".", "a/b", "a\\b", ""] {
        let index = PathIndex::build(vec![file("x", name, None)]);
        assert_eq!(
            index.unaddressable, 1,
            "name {name:?} must not be path-addressable"
        );
        assert!(index.by_path.is_empty(), "name {name:?} rendered a path");
    }
}

#[test]
fn traversal_shaped_paths_are_rejected_before_resolution() {
    for path in [
        "../secrets.md",
        "standards/../../etc/passwd",
        "./Standards",
        "..",
        "standards/..",
        "C:\\Windows",
        "   ",
    ] {
        assert!(
            split_logical_path(path).is_err(),
            "path {path:?} must be rejected"
        );
    }
}

#[test]
fn redundant_separators_are_tolerated_but_segments_are_not_invented() {
    assert_eq!(
        split_logical_path("/standards/").unwrap(),
        vec!["standards"]
    );
    assert_eq!(
        split_logical_path("standards//eng.md").unwrap(),
        vec!["standards", "eng.md"]
    );
    assert!(split_logical_path("/").unwrap_err().contains("segments"));
}

/// An absolute-looking host path cannot resolve: `/etc/passwd` normalises to
/// the segments `etc/passwd`, which no node in the company tree carries.
#[test]
fn an_absolute_host_path_resolves_to_nothing() {
    let index = PathIndex::build(vec![
        folder("a", "standards", None),
        file("b", "engineering-standards.md", Some("a")),
    ]);
    let err = index.resolve(Some("/etc/passwd"), None).unwrap_err();
    assert!(matches!(err, ResolveError::NotFound(_)), "{err:?}");
}

// -- ambiguity ----------------------------------------------------------

/// Nothing in the port enforces unique sibling names, so two notes can share
/// a path. Resolving one arbitrarily would let a write land on the wrong
/// operator-owned note — the resolver must refuse and name the candidates.
#[test]
fn a_duplicated_path_is_refused_rather_than_guessed() {
    let index = PathIndex::build(vec![
        folder("a", "standards", None),
        file("b1", "dup.md", Some("a")),
        file("b2", "dup.md", Some("a")),
    ]);
    let err = index.resolve(Some("standards/dup.md"), None).unwrap_err();
    match &err {
        ResolveError::Ambiguous { ids, .. } => assert_eq!(ids, &["b1", "b2"]),
        other => panic!("expected Ambiguous, got {other:?}"),
    }
    let message = err.message();
    assert!(
        message.contains("b1") && message.contains("b2"),
        "{message}"
    );
    // Addressing by id stays available and unambiguous.
    assert_eq!(index.resolve(None, Some("b2")).unwrap().node.id, "b2");
}

#[test]
fn resolve_requires_exactly_one_of_path_and_id() {
    let index = PathIndex::build(vec![file("b", "note.md", None)]);
    assert!(matches!(
        index.resolve(Some("note.md"), Some("b")).unwrap_err(),
        ResolveError::BadArgs(_)
    ));
    assert!(matches!(
        index.resolve(None, None).unwrap_err(),
        ResolveError::BadArgs(_)
    ));
}

// -- truncation ---------------------------------------------------------

#[test]
fn clamp_body_never_splits_a_codepoint() {
    // Each crab is 4 bytes, so every cap from 1..8 lands mid-codepoint.
    let body = "🦀🦀";
    for cap in 0..=body.len() {
        let (kept, dropped) = clamp_body(body, cap);
        assert!(body.starts_with(kept), "cap {cap}");
        assert_eq!(kept.len() + dropped, body.len(), "cap {cap}");
        assert!(kept.len() <= cap, "cap {cap} kept {}", kept.len());
    }
    let (kept, dropped) = clamp_body(body, 64);
    assert_eq!(kept, body);
    assert_eq!(dropped, 0);
}

// -- tenancy ------------------------------------------------------------

/// The boundary proof, step 1: company B's tools see an empty index even
/// though company A's notes exist in the same store.
#[tokio::test]
async fn tenancy_company_b_cannot_list_company_a_notes() {
    let (_dir, store) = seeded("acme").await;
    let tool = WorkspaceListTool::new(ws(store.clone(), CompanyId::new("other")));
    let out = text(&tool.execute(json!({})).await.unwrap());
    assert!(out.contains("workspace is empty"), "{out}");
    assert!(!out.contains("engineering-standards.md"), "{out}");
}

/// Step 2: a *valid* node id lifted from company A cannot be read by
/// company B's tool — it is absent from B's index, so the store is never
/// asked for it.
#[tokio::test]
async fn tenancy_a_borrowed_node_id_does_not_resolve_for_another_company() {
    let (_dir, store) = seeded("acme").await;
    // Sanity: the id is real and readable for its owner.
    let owner = WorkspaceReadTool::new(ws(store.clone(), CompanyId::new("acme")));
    let owned = text(&owner.execute(json!({"id": "n-eng"})).await.unwrap());
    assert!(owned.contains("Review every PR."), "{owned}");

    let intruder = WorkspaceReadTool::new(ws(store.clone(), CompanyId::new("other")));
    let result = intruder.execute(json!({"id": "n-eng"})).await.unwrap();
    assert!(result.is_error, "a borrowed id must not read");
    let out = text(&result);
    assert!(out.contains("No workspace note matches"), "{out}");
    assert!(!out.contains("Review every PR."), "leaked body: {out}");
}

/// Step 3: the write path is bounded the same way — company B cannot
/// overwrite company A's note by id, and A's note is untouched afterwards.
#[tokio::test]
async fn tenancy_a_borrowed_node_id_cannot_be_written_by_another_company() {
    let (_dir, store) = seeded("acme").await;
    let intruder = WorkspaceWriteTool::new(ws(store.clone(), CompanyId::new("other")));
    let result = intruder
        .execute(json!({
            "id": "n-eng",
            "content": "pwned",
            "expected_updated_at": 2_000,
        }))
        .await
        .unwrap();
    assert!(result.is_error, "{}", text(&result));

    let (_, body) = store
        .read(&CompanyId::new("acme"), "n-eng")
        .await
        .unwrap()
        .expect("note still there");
    assert_eq!(body, "# Engineering\nReview every PR.");
}

/// Step 4: traversal-shaped paths cannot reach the host filesystem. The
/// tool never joins agent input onto a path, so these resolve to nothing
/// rather than escaping the company tree.
#[tokio::test]
async fn traversal_paths_cannot_escape_the_company_tree() {
    let (_dir, store) = seeded("acme").await;
    let tool = WorkspaceReadTool::new(ws(store, CompanyId::new("acme")));
    for path in [
        "../../../../etc/passwd",
        "standards/../../../etc/passwd",
        "/etc/passwd",
        "..",
    ] {
        let result = tool.execute(json!({"path": path})).await.unwrap();
        assert!(result.is_error, "path {path:?} must not resolve");
        let out = text(&result);
        assert!(!out.contains("root:"), "path {path:?} leaked: {out}");
    }
}
