use super::tests::*;
use super::*;

// -- workspace_create (issue #551) ---------------------------------------

/// The whole point of the feature, end to end: an agent creates a note that
/// was not there before, and it lands in the tree the operator reads.
#[tokio::test]
async fn create_lands_a_new_note_in_the_shared_tree() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({
            "path": "standards/Deploys.md",
            "kind": "file",
            "content": "# Deploys\nGreen builds only.",
        }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));

    let tree = store.tree(&id).await.unwrap();
    let node = tree
        .iter()
        .find(|n| n.name == "deploys.md")
        .expect("the note is in the tree");
    assert_eq!(node.kind, NodeKind::File);
    let (_, body) = store.read(&id, &node.id).await.unwrap().unwrap();
    assert_eq!(body, "# Deploys\nGreen builds only.");

    // The acknowledgement hands back the id and the revision, so an
    // immediate follow-up write needs no extra list + read round trip.
    let out = text(&out);
    assert!(out.contains(&format!("id={}", node.id)), "{out}");
    assert!(
        out.contains(&format!("expected_updated_at={}", node.updated_at_millis)),
        "{out}"
    );
}

/// Authorship: a created node is stamped with the creating agent on BOTH
/// origins, and the path it was created at has nothing to do with it.
///
/// This test is deliberately sited under `standards/` — shared,
/// operator-owned guidance, as far from the agent's own folder as the tree
/// goes. It is the executable form of the settled decision that agents
/// write **unconfined**: if someone later adds a prefix gate, this fails.
#[tokio::test]
async fn create_is_unconfined_and_stamps_the_creating_agent() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({ "path": "standards/Agent addendum.md", "kind": "file" }))
        .await
        .unwrap();
    assert!(
        !out.is_error,
        "creating outside `agents/` must be allowed: {}",
        text(&out)
    );

    let node = store
        .tree(&id)
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.name == "agent-addendum.md")
        .unwrap();
    assert_eq!(node.created_by, agent_origin());
    assert_eq!(node.updated_by, agent_origin());
}

/// The name the agent typed is normalized, and the reply names where the
/// note actually landed.
///
/// The echo is the whole contract: the agent has to be able to read back
/// what it just wrote, and an acknowledgement quoting the path it *asked*
/// for would send it to a path that does not exist.
#[tokio::test]
async fn create_normalizes_the_name_and_says_where_it_landed() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({ "path": "standards/Q3 Launch Brief.md", "kind": "file" }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    assert!(
        text(&out).contains("standards/q3-launch-brief.md"),
        "the reply must name the stored path: {}",
        text(&out)
    );

    let tree = store.tree(&id).await.unwrap();
    assert!(
        tree.iter().any(|n| n.name == "q3-launch-brief.md"),
        "{:?}",
        tree.iter().map(|n| &n.name).collect::<Vec<_>>()
    );
}

/// A parent folder stored under a legacy spelling still resolves, and the
/// reply names *its* spelling rather than the one the agent typed.
///
/// Both halves matter. Refusing would tell an agent that a folder it can
/// see in the listing does not exist; echoing the typed spelling would hand
/// it a path that resolves only by the same fallback it does not know
/// about.
#[tokio::test]
async fn create_resolves_a_legacy_parent_and_echoes_its_stored_path() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    store
        .adopt_or_create_folder(&id, None, "Playbooks", WorkspaceOrigin::Operator)
        .await
        .unwrap();
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({ "path": "playbooks/Release checklist.md", "kind": "file" }))
        .await
        .unwrap();

    assert!(!out.is_error, "{}", text(&out));
    assert!(
        text(&out).contains("Playbooks/release-checklist.md"),
        "the reply must name the folder as it is stored: {}",
        text(&out)
    );
    let tree = store.tree(&id).await.unwrap();
    assert_eq!(
        tree.iter()
            .filter(|n| n.name.eq_ignore_ascii_case("playbooks"))
            .count(),
        1,
        "no rival folder: {:?}",
        tree.iter().map(|n| &n.name).collect::<Vec<_>>()
    );
}

/// The steered-for case: the agent's own folder, created as a folder and
/// then filled.
#[tokio::test]
async fn create_makes_a_folder_then_a_note_inside_it() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    for args in [
        json!({ "path": "Agents", "kind": "folder" }),
        json!({ "path": "agents/ceo", "kind": "folder" }),
        json!({ "path": "agents/ceo/Launch brief.md", "kind": "file", "content": "# Launch" }),
    ] {
        let out = tool.execute(args.clone()).await.unwrap();
        assert!(!out.is_error, "{args}: {}", text(&out));
    }

    let tree = store.tree(&id).await.unwrap();
    let brief = tree.iter().find(|n| n.name == "launch-brief.md").unwrap();
    let ceo = tree.iter().find(|n| n.name == "ceo").unwrap();
    assert_eq!(brief.parent_id.as_deref(), Some(ceo.id.as_str()));
    assert_eq!(ceo.kind, NodeKind::Folder);
}

/// A retry whose initial snapshot already contains the agent's home adopts
/// that folder instead of rejecting it before the ownership-aware path runs.
#[tokio::test]
async fn create_inside_existing_own_home_adopts_without_duplication() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    crate::company::workspace_scaffold::ensure_workspace_scaffold(store.as_ref(), &id)
        .await
        .unwrap();
    let home =
        crate::company::workspace_scaffold::ensure_agent_folder(store.as_ref(), &id, TEST_AGENT)
            .await
            .unwrap();
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({
            "path": "agents/ceo/Retry note.md",
            "kind": "file",
            "content": "# Retry",
        }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));

    let tree = store.tree(&id).await.unwrap();
    assert_eq!(
        tree.iter()
            .filter(|node| node.parent_id.as_deref() == Some(home.as_str()))
            .count(),
        1,
        "the existing home must not be duplicated"
    );
}

/// straight to the note, with no folder call first.
///
/// Since issue #551 stopped provisioning a folder per roster member, the
/// home does not exist until it is used — so this call is the *only* way it
/// ever comes into existence, and refusing it would make the brief's
/// instruction unfollowable.
#[tokio::test]
async fn create_in_the_agents_own_home_mints_the_home_folder() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    crate::company::workspace_scaffold::ensure_workspace_scaffold(store.as_ref(), &id)
        .await
        .unwrap();
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({
            "path": "agents/ceo/Launch brief.md",
            "kind": "file",
            "content": "# Launch",
        }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));

    let tree = store.tree(&id).await.unwrap();
    let root = tree
        .iter()
        .find(|n| n.name == AGENTS_ROOT && n.parent_id.is_none())
        .expect("the scaffolded root");
    let home = tree
        .iter()
        .find(|n| n.name == TEST_AGENT)
        .expect("the home folder was minted");
    assert_eq!(home.kind, NodeKind::Folder);
    assert_eq!(home.parent_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(
        home.created_by,
        agent_origin(),
        "the folder belongs to the agent that earned it"
    );
    let brief = tree.iter().find(|n| n.name == "launch-brief.md").unwrap();
    assert_eq!(brief.parent_id.as_deref(), Some(home.id.as_str()));

    // A second note goes into the same folder — minting is find-or-create,
    // not create.
    let out = tool
        .execute(json!({ "path": "agents/ceo/Retro.md", "kind": "file" }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    let tree = store.tree(&id).await.unwrap();
    assert_eq!(
        tree.iter().filter(|n| n.name == TEST_AGENT).count(),
        1,
        "the second create minted a rival home folder"
    );
    assert_eq!(
        tree.iter()
            .find(|n| n.name == "retro.md")
            .unwrap()
            .parent_id
            .as_deref(),
        Some(home.id.as_str())
    );
}

/// The mint repairs its own root too: an agent whose company never got the
/// boot scaffold (or whose create fail-softed) still lands its work under
/// `agents/`, rather than being stuck behind a folder nobody will make.
#[tokio::test]
async fn the_home_mint_creates_the_agents_root_when_it_is_missing() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({ "path": "agents/ceo/Brief.md", "kind": "file" }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));

    let tree = store.tree(&id).await.unwrap();
    let root = tree
        .iter()
        .find(|n| n.name == AGENTS_ROOT && n.parent_id.is_none())
        .expect("the root was minted alongside the home");
    assert_eq!(root.created_by, WorkspaceOrigin::Seed);
    assert_eq!(
        tree.iter()
            .find(|n| n.name == TEST_AGENT)
            .unwrap()
            .parent_id
            .as_deref(),
        Some(root.id.as_str())
    );
}

/// The exception is *this* agent's own home and nothing else. A teammate's
/// home is somebody else's folder to earn, so the ordinary missing-parent
/// refusal stands — an agent must not be able to conjure a folder that
/// then reads as belonging to a teammate who never produced anything.
#[tokio::test]
async fn create_does_not_mint_another_agents_home() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    crate::company::workspace_scaffold::ensure_workspace_scaffold(store.as_ref(), &id)
        .await
        .unwrap();
    let before = store.tree(&id).await.unwrap().len();
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({ "path": "agents/cmo/Brief.md", "kind": "file" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(text(&out).contains("agents/cmo"), "{}", text(&out));
    assert_eq!(
        store.tree(&id).await.unwrap().len(),
        before,
        "a refused create must not have made a teammate's folder"
    );
}

/// One node per call survives the exception: the home is minted only when
/// it is the *direct* parent, so a deeper path is still an actionable
/// refusal and still creates nothing at all — not even the home.
#[tokio::test]
async fn create_below_the_home_still_refuses_and_mints_nothing() {
    let (_dir, store) = seeded("acme").await;
    let id = CompanyId::new("acme");
    crate::company::workspace_scaffold::ensure_workspace_scaffold(store.as_ref(), &id)
        .await
        .unwrap();
    let before = store.tree(&id).await.unwrap().len();
    let tool = WorkspaceCreateTool::new(ws(store.clone(), id.clone()));

    let out = tool
        .execute(json!({ "path": "agents/ceo/drafts/Brief.md", "kind": "file" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(text(&out).contains("agents/ceo/drafts"), "{}", text(&out));
    assert_eq!(
        store.tree(&id).await.unwrap().len(),
        before,
        "a refused create made intermediate folders"
    );
}

/// A store wrapper over a real backend with the two test knobs issue #1801
/// needs. `hidden` drops one node from every `tree()` read — the stale
/// snapshot a racing create acts on, while the real folder still answers
/// `adopt_or_create_folder`. `refuse_note` fails every *file* create — the
/// shape a store error or quota refusal takes — while folders still mint,
/// so the home is created on the way in and only the note fails.
pub(super) struct ProxyStore {
    inner: Arc<dyn WorkspaceStore>,
    hidden: Option<String>,
    refuse_note: bool,
}

impl ProxyStore {
    pub(super) fn hiding(inner: Arc<dyn WorkspaceStore>, hidden: &str) -> Self {
        Self {
            inner,
            hidden: Some(hidden.to_string()),
            refuse_note: false,
        }
    }
    pub(super) fn refusing_notes(inner: Arc<dyn WorkspaceStore>) -> Self {
        Self {
            inner,
            hidden: None,
            refuse_note: true,
        }
    }
}

#[async_trait]
impl WorkspaceStore for ProxyStore {
    async fn tree(&self, company: &CompanyId) -> crate::Result<Vec<WorkspaceNode>> {
        let mut nodes = self.inner.tree(company).await?;
        if let Some(hidden) = &self.hidden {
            nodes.retain(|node| &node.id != hidden);
        }
        Ok(nodes)
    }
    async fn read(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> crate::Result<Option<(WorkspaceNode, String)>> {
        self.inner.read(company, id).await
    }
    async fn read_capped(
        &self,
        company: &CompanyId,
        id: &str,
        max_bytes: u64,
    ) -> crate::Result<Option<(WorkspaceNode, String, u64)>> {
        self.inner.read_capped(company, id, max_bytes).await
    }
    async fn write_with_revision(
        &self,
        company: &CompanyId,
        id: &str,
        content: &str,
        author: WorkspaceOrigin,
        expected_updated_at: Option<u64>,
    ) -> crate::Result<WorkspaceNode> {
        self.inner
            .write_with_revision(company, id, content, author, expected_updated_at)
            .await
    }
    async fn create(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        content: Option<&str>,
    ) -> crate::Result<()> {
        if self.refuse_note && node.kind == NodeKind::File {
            return Err(crate::error::OpenCompanyError::InvalidRequest(
                "over quota".to_string(),
            ));
        }
        self.inner.create(company, node, content).await
    }
    async fn adopt_or_create_folder(
        &self,
        company: &CompanyId,
        parent: Option<&str>,
        name: &str,
        origin: WorkspaceOrigin,
    ) -> crate::Result<crate::ports::workspace::FolderClaim> {
        self.inner
            .adopt_or_create_folder(company, parent, name, origin)
            .await
    }
    async fn create_binary(
        &self,
        company: &CompanyId,
        node: &WorkspaceNode,
        bytes: &[u8],
    ) -> crate::Result<WorkspaceNode> {
        self.inner.create_binary(company, node, bytes).await
    }
    async fn write_binary(
        &self,
        company: &CompanyId,
        id: &str,
        bytes: &[u8],
        mime: Option<&str>,
        author: WorkspaceOrigin,
    ) -> crate::Result<WorkspaceNode> {
        self.inner
            .write_binary(company, id, bytes, mime, author)
            .await
    }
    async fn read_bytes(
        &self,
        company: &CompanyId,
        id: &str,
    ) -> crate::Result<Option<(WorkspaceNode, crate::ports::workspace::BlobStream)>> {
        self.inner.read_bytes(company, id).await
    }
    async fn rename_move(
        &self,
        company: &CompanyId,
        id: &str,
        name: Option<&str>,
        parent: Option<Option<&str>>,
    ) -> crate::Result<WorkspaceNode> {
        self.inner.rename_move(company, id, name, parent).await
    }
    async fn swap_files(
        &self,
        company: &CompanyId,
        expected_id: Option<&str>,
        replacement_id: &str,
        name: &str,
    ) -> crate::Result<Option<WorkspaceNode>> {
        self.inner
            .swap_files(company, expected_id, replacement_id, name)
            .await
    }
    async fn delete(&self, company: &CompanyId, id: &str) -> crate::Result<bool> {
        self.inner.delete(company, id).await
    }
    async fn is_empty(&self, company: &CompanyId) -> crate::Result<bool> {
        self.inner.is_empty(company).await
    }
}
