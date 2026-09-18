use super::tests::*;
use super::*;
use crate::store::FsOps;

// -- issue #552: an overwrite of a published note reaches its chain ------

/// A note in the shared tree may be another agent's published deliverable,
/// whose authoritative history is the artifact chain. An agent overwriting
/// one must record the revision there too — otherwise the Artifacts tab and
/// `human_edit_diff`, which read the chain and not the tree, would keep
/// showing a body that no longer exists.
///
/// Recorded as an **agent** version stamped with this agent's id, so an
/// overwrite by a teammate never masquerades as the human edit the port
/// exists to isolate.
#[tokio::test]
async fn overwriting_a_published_note_records_the_revision_on_its_artifact() {
    use crate::ports::artifacts::{ArtifactKind, ArtifactRecord};

    let dir = tempfile::tempdir().unwrap();
    let ops = Arc::new(FsOps::new(dir.path()));
    let store: Arc<dyn WorkspaceStore> = ops.clone();
    let artifacts: Arc<dyn ArtifactStore> = ops.clone();
    let id = CompanyId::new("acme");

    let node = WorkspaceNode {
        id: "n-deliverable".to_string(),
        name: "launch.md".to_string(),
        kind: NodeKind::File,
        parent_id: None,
        updated_at_millis: 2_000,
        created_by: WorkspaceOrigin::Agent {
            id: "maya".to_string(),
        },
        updated_by: WorkspaceOrigin::Agent {
            id: "maya".to_string(),
        },
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    };
    store
        .create(&id, &node, Some("maya's draft"))
        .await
        .unwrap();

    let mut published = ArtifactRecord::new(
        "art-1",
        "t-1",
        "Launch spec",
        ArtifactKind::Markdown,
        "maya's draft",
        "maya",
        1,
    );
    published.stamp_workspace_node("n-deliverable");
    artifacts.upsert(&id, &published).await.unwrap();

    let tool = WorkspaceWriteTool::new(
        CompanyWorkspace::new(store.clone(), id.clone(), TEST_AGENT.to_string())
            .with_artifacts(Some(artifacts.clone())),
    );
    let result = tool
        .execute(json!({
            "id": "n-deliverable",
            "content": "the ceo's revision",
            "expected_updated_at": 2_000,
        }))
        .await
        .unwrap();
    assert!(!result.is_error, "{}", text(&result));

    let stored = artifacts.get(&id, "art-1").await.unwrap().unwrap();
    assert_eq!(stored.versions.len(), 2, "the chain must see the overwrite");
    assert_eq!(stored.latest().unwrap().body, "the ceo's revision");
    assert_eq!(stored.latest().unwrap().author, ArtifactAuthor::Agent);
    assert_eq!(
        stored.latest().unwrap().author_id,
        TEST_AGENT,
        "an agent overwrite must not be filed as the operator's human edit"
    );
    assert_eq!(
        stored.workspace_node_id(),
        Some("n-deliverable"),
        "the new version keeps the node, or the next overwrite mirrors nothing"
    );
    assert!(
        stored.human_edit_diff().is_none(),
        "two agent versions are not a human edit"
    );
}

/// Nearly every note is an ordinary note. Overwriting one records nothing
/// on any artifact — and a refused write records nothing either, because
/// the mirror runs only after the CAS'd store write actually lands.
#[tokio::test]
async fn an_ordinary_or_refused_write_records_no_artifact_version() {
    use crate::ports::artifacts::{ArtifactKind, ArtifactRecord};

    let dir = tempfile::tempdir().unwrap();
    let ops = Arc::new(FsOps::new(dir.path()));
    let store: Arc<dyn WorkspaceStore> = ops.clone();
    let artifacts: Arc<dyn ArtifactStore> = ops.clone();
    let id = CompanyId::new("acme");

    let node = WorkspaceNode {
        id: "n-plain".to_string(),
        name: "notes.md".to_string(),
        kind: NodeKind::File,
        parent_id: None,
        updated_at_millis: 2_000,
        created_by: WorkspaceOrigin::Operator,
        updated_by: WorkspaceOrigin::Operator,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    };
    store.create(&id, &node, Some("a note")).await.unwrap();

    // An artifact exists, but points at a different node.
    let mut published = ArtifactRecord::new(
        "art-1",
        "t-1",
        "Launch spec",
        ArtifactKind::Markdown,
        "deliverable",
        "maya",
        1,
    );
    published.stamp_workspace_node("n-deliverable");
    artifacts.upsert(&id, &published).await.unwrap();

    let tool = WorkspaceWriteTool::new(
        CompanyWorkspace::new(store.clone(), id.clone(), TEST_AGENT.to_string())
            .with_artifacts(Some(artifacts.clone())),
    );

    // An ordinary note: the write lands, the chain is untouched.
    assert!(
        !tool
            .execute(json!({
                "id": "n-plain",
                "content": "an edited note",
                "expected_updated_at": 2_000,
            }))
            .await
            .unwrap()
            .is_error
    );

    // A stale revision: the write is refused, so nothing may be recorded —
    // a version appended before the CAS would claim an edit never made.
    assert!(
        tool.execute(json!({
            "id": "n-plain",
            "content": "clobber",
            "expected_updated_at": 1,
        }))
        .await
        .unwrap()
        .is_error
    );

    assert_eq!(
        artifacts
            .get(&id, "art-1")
            .await
            .unwrap()
            .unwrap()
            .versions
            .len(),
        1,
        "neither an unrelated note nor a refused write may touch the chain"
    );
}

// -- own-home scope (issue #671) ----------------------------------------

/// The two home predicates answer different questions and must keep
/// answering them differently.
///
/// `is_own_home` is create's mint-on-demand exception: exactly the folder,
/// nothing else. `is_strictly_inside_own_home` is the lifecycle gate: the
/// contents, and never the folder itself. Collapsing either into the other
/// would either let an agent delete the folder the company finds its work
/// in, or refuse it the call that brings that folder into existence.
#[test]
fn the_two_home_predicates_disagree_exactly_on_the_folder_itself() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let workspace = ws(store, CompanyId::new("acme"));

    // The folder itself: mintable, never inside.
    assert!(workspace.is_own_home(&[AGENTS_ROOT, TEST_AGENT]));
    assert!(!workspace.is_strictly_inside_own_home(&[AGENTS_ROOT, TEST_AGENT]));

    // Inside it, at any depth: never the folder, always inside.
    for segments in [
        vec![AGENTS_ROOT, TEST_AGENT, "brief.md"],
        vec![AGENTS_ROOT, TEST_AGENT, "drafts", "q3", "notes.md"],
    ] {
        assert!(!workspace.is_own_home(&segments), "{segments:?}");
        assert!(
            workspace.is_strictly_inside_own_home(&segments),
            "{segments:?}"
        );
    }

    // Everything else is neither — a teammate's home and its contents
    // included, and the `Agents` root itself, which belongs to nobody.
    for segments in [
        vec![AGENTS_ROOT],
        vec![AGENTS_ROOT, "cmo"],
        vec![AGENTS_ROOT, "cmo", "brief.md"],
        vec!["standards", "engineering-standards.md"],
        // A name that merely starts with the agent's id is a different
        // folder, because the comparison is segment-wise and not a prefix.
        vec![AGENTS_ROOT, "ceo-archive", "brief.md"],
    ] {
        assert!(!workspace.is_own_home(&segments), "{segments:?}");
        assert!(
            !workspace.is_strictly_inside_own_home(&segments),
            "{segments:?}"
        );
    }
}

// -- wiring -------------------------------------------------------------

#[test]
fn the_mutating_tools_are_only_present_when_writes_are_granted() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));

    let read_only = workspace_tools(
        store.clone(),
        None,
        CompanyId::new("acme"),
        TEST_AGENT.to_string(),
        false,
        None,
        Default::default(),
    );
    let names: Vec<&str> = read_only.iter().map(|t| t.name()).collect();
    assert_eq!(
        names,
        vec![
            WORKSPACE_LIST_TOOL,
            WORKSPACE_READ_TOOL,
            // Issue #607: search is a read and rides the read set. Behind
            // `can_write` it would be unreachable for the default (`*`)
            // agent, leaving exactly the crawl it exists to end.
            WORKSPACE_SEARCH_TOOL
        ]
    );

    let writable = workspace_tools(
        store,
        None,
        CompanyId::new("acme"),
        TEST_AGENT.to_string(),
        true,
        None,
        Default::default(),
    );
    let names: Vec<&str> = writable.iter().map(|t| t.name()).collect();
    assert_eq!(
        names,
        vec![
            WORKSPACE_LIST_TOOL,
            WORKSPACE_READ_TOOL,
            WORKSPACE_SEARCH_TOOL,
            WORKSPACE_CREATE_TOOL,
            WORKSPACE_WRITE_TOOL,
            // Issue #671. No fifth grant name: the write grant already
            // confers unconfined overwrite, which reaches further than
            // own-folder lifecycle does.
            WORKSPACE_RENAME_TOOL,
            WORKSPACE_DELETE_TOOL
        ],
        "all four mutations ride the same explicit grant"
    );
}

#[test]
fn declared_permission_levels_match_what_each_tool_does() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkspaceStore> = Arc::new(FsOps::new(dir.path()));
    let tools = workspace_tools(
        store,
        None,
        CompanyId::new("acme"),
        TEST_AGENT.to_string(),
        true,
        None,
        Default::default(),
    );
    assert_eq!(tools[0].permission_level(), PermissionLevel::ReadOnly);
    assert_eq!(tools[1].permission_level(), PermissionLevel::ReadOnly);
    assert_eq!(tools[2].permission_level(), PermissionLevel::ReadOnly);
    assert_eq!(tools[3].permission_level(), PermissionLevel::Write);
    assert_eq!(tools[4].permission_level(), PermissionLevel::Write);
    assert_eq!(tools[5].permission_level(), PermissionLevel::Write);
    assert_eq!(tools[6].permission_level(), PermissionLevel::Write);
    assert_eq!(tools.len(), 7, "a tool was added without a declared level");
}

#[test]
fn the_brief_is_static_and_mentions_writes_only_when_granted() {
    let read_only = workspace_brief(false);
    assert!(read_only.contains(WORKSPACE_LIST_TOOL));
    // Describing a tool the agent does not hold is how a turn gets spent
    // calling something that does not exist — so the read-only brief has to
    // omit every mutation, the lifecycle pair included.
    for tool in [
        WORKSPACE_WRITE_TOOL,
        WORKSPACE_CREATE_TOOL,
        WORKSPACE_RENAME_TOOL,
        WORKSPACE_DELETE_TOOL,
    ] {
        assert!(!read_only.contains(tool), "{tool}: {read_only}");
    }
    let writable = workspace_brief(true);
    for tool in [
        WORKSPACE_WRITE_TOOL,
        WORKSPACE_CREATE_TOOL,
        WORKSPACE_RENAME_TOOL,
        WORKSPACE_DELETE_TOOL,
    ] {
        assert!(writable.contains(tool), "{tool}: {writable}");
    }
    assert!(writable.contains("expected_updated_at"));
}

/// The steering half of issue #607, pinned like the tool itself.
///
/// A tool an agent is never told to prefer is a tool an agent does not
/// reach for: the list-then-read crawl is what the brief taught for four
/// issues, and adding a search tool without changing that paragraph would
/// leave the habit in place and the cost unchanged. So the brief has to
/// name search *before* listing and say why, and it has to do so on the
/// read-only brief too — the agent that benefits most is the ungranted one
/// that can only read.
#[test]
fn the_brief_sends_agents_to_search_before_crawling_the_tree() {
    for brief in [workspace_brief(false), workspace_brief(true)] {
        assert!(
            brief.contains(WORKSPACE_SEARCH_TOOL),
            "the brief must name the search tool: {brief}"
        );
        assert!(
            brief.contains("Search first"),
            "the brief must say which one to reach for first: {brief}"
        );
        let search_at = brief.find(WORKSPACE_SEARCH_TOOL).expect("search");
        let list_at = brief.find(WORKSPACE_LIST_TOOL).expect("list");
        assert!(
            search_at < list_at,
            "search must be named before listing, or the habit does not change: {brief}"
        );
    }
}

/// Issue #551 replaced a refusal with steering, so the steering is the
/// mechanism and has to be asserted like one.
///
/// The brief must name the agent's own folder as the default home, mark
/// shared guidance as conditional rather than forbidden (create and write
/// are unconfined — saying "never" here would be a lie those tools do not
/// back), and, since issue #671, ask for tidying while keeping the
/// lifecycle pair's confinement and permanence explicit. It must NOT still
/// say rename and delete are the operator's, full stop: that sentence
/// became false the moment the tools shipped, and an agent that believes it
/// will never clean up after itself.
#[test]
fn the_brief_steers_toward_the_agents_own_folder() {
    let brief = workspace_brief(true);
    assert!(
        brief.contains(&format!("{AGENTS_ROOT}/<your agent id>/")),
        "the brief must name the agent's own folder: {brief}"
    );
    for phrase in [
        "default home",
        // The folder is minted on first use, so the brief has to say so —
        // an agent told to look for a folder that is not there yet would
        // otherwise reasonably conclude it has none.
        "appears the first time you use it",
        "anywhere in the tree",
        "standards/",
        // Issue #671: tidying is asked for, bounded, and honest about what
        // a delete costs.
        "part of producing work in it",
        "one node at a time",
        "Deleting is permanent",
        "OUTSIDE your own folder",
    ] {
        assert!(
            brief.contains(phrase),
            "the brief dropped {phrase:?}: {brief}"
        );
    }
    assert!(
        !brief.contains("Renaming and deleting stay the operator's job"),
        "the brief still tells agents they cannot tidy their own folder: {brief}"
    );
}
