//! Fault-injection and task-folder-naming tests: unreadable/unwritable
//! artifact stores, and the task-folder naming/adoption rules that keep
//! one card in one folder (split out of `artifact_mirror_tests.rs`).

use super::artifact_mirror_tests_support::*;
use super::*;

/// A store that cannot be listed establishes **nothing**, and must not be
/// reported as the ordinary-note answer.
///
/// This is the variant that carries the whole guarantee: `Ordinary` is what
/// callers are entitled to write a node behind. If a read fault collapsed
/// into it, every published deliverable would silently lose its fail-closed
/// protection the moment the store got sick — the one moment it matters.
#[tokio::test]
async fn an_unreadable_store_is_undetermined_not_ordinary() {
    let co = CompanyId::new("acme");
    let artifacts = FaultyArtifacts {
        listed: Vec::new(),
        list_fails: true,
        upsert_fails: false,
    };

    let outcome = mirror_node_edit(
        &artifacts,
        &co,
        "node-1",
        "edit",
        ArtifactAuthor::Operator,
        "operator",
        None,
    )
    .await
    .expect("an unreadable store is the caller's decision, not an error");

    assert!(
        matches!(outcome, MirrorOutcome::Undetermined(_)),
        "a read fault must stay distinguishable from `Ordinary`: {outcome:?}"
    );
}

/// Once the store has answered and named this node a deliverable, a version
/// that cannot be appended is an error — the caller must not go on to write
/// the node, because that is the silent, permanent direction.
#[tokio::test]
async fn a_refused_append_on_a_published_node_still_fails_closed() {
    let co = CompanyId::new("acme");
    let artifacts = FaultyArtifacts {
        listed: vec![published_as("node-1")],
        list_fails: false,
        upsert_fails: true,
    };

    assert!(
        mirror_node_edit(
            &artifacts,
            &co,
            "node-1",
            "edit",
            ArtifactAuthor::Operator,
            "operator",
            None,
        )
        .await
        .is_err(),
        "a known deliverable whose version cannot be recorded must refuse the save"
    );
}

/// Issue #1687: the task folder is named for the *work*, and still carries
/// the id.
///
/// The whole complaint is legibility. `artifacts/cmo/01hq8zm4x…/` is a
/// perfectly good key and tells an operator scanning the tree nothing
/// whatsoever — every sibling looks identical, and finding the most recent
/// one means opening each. The title goes first because the explorer pane
/// truncates from the right; the id stays because it is the only unique,
/// immutable half and it is what an operator holding a card id matches
/// against.
#[tokio::test]
async fn a_task_folder_is_named_for_the_card_and_keeps_its_id() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let id = materialize(
        ws,
        &co,
        PublishTarget {
            task_title: Some("Q3 Launch Brief"),
            ..target("launch.md", "# Launch")
        },
    )
    .await
    .expect("materialize")
    .node_id;

    assert_eq!(
        path_of(ws, &co, &id).await,
        format!("{ARTIFACTS_ROOT}/cmo/q3-launch-brief.t-1/launch.md"),
        "the folder must read as the work and still end in the card id"
    );
}

/// Renaming a card must not split its deliverables across two folders.
///
/// The folder name is now a function of an **editable** string, so an
/// exact-name lookup would miss the folder the moment somebody retitled
/// the card and the next publish would mint a rival beside it. The lookup
/// therefore matches the id suffix, which is the half that cannot change —
/// and the existing folder keeps the name it was minted under, because
/// nothing in this runtime renames a node an operator may have linked to.
#[tokio::test]
async fn a_publish_after_the_card_was_retitled_stays_in_the_same_folder() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(
        ws,
        &co,
        PublishTarget {
            task_title: Some("Q3 Launch Brief"),
            ..target("launch.md", "# Launch")
        },
    )
    .await
    .expect("first publish")
    .node_id;
    let second = materialize(
        ws,
        &co,
        PublishTarget {
            task_title: Some("Q3 Launch Brief (revised scope)"),
            ..target("timeline.md", "# Timeline")
        },
    )
    .await
    .expect("second publish")
    .node_id;

    assert_eq!(
        path_of(ws, &co, &first).await,
        format!("{ARTIFACTS_ROOT}/cmo/q3-launch-brief.t-1/launch.md")
    );
    assert_eq!(
        path_of(ws, &co, &second).await,
        format!("{ARTIFACTS_ROOT}/cmo/q3-launch-brief.t-1/timeline.md"),
        "a retitled card must publish into the folder it already has, not a second one"
    );
}

/// A company that published before this change keeps the folders it has.
///
/// Its task folders are named by the bare id, and the id-suffix lookup
/// matches those too — so the next publish *adopts* the existing folder
/// rather than opening a titled twin beside it and splitting one task's
/// deliverables in half. Nothing is renamed: an operator must not find
/// their tree rearranged, and a rename breaks every link kept to the old
/// name.
#[tokio::test]
async fn a_folder_minted_before_titles_is_adopted_rather_than_twinned() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let legacy = materialize(ws, &co, target("launch.md", "# Launch"))
        .await
        .expect("legacy publish")
        .node_id;
    assert_eq!(
        path_of(ws, &co, &legacy).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/launch.md"),
        "a caller with no title still names the folder by the id alone"
    );

    let titled = materialize(
        ws,
        &co,
        PublishTarget {
            task_title: Some("Q3 Launch Brief"),
            ..target("timeline.md", "# Timeline")
        },
    )
    .await
    .expect("titled publish")
    .node_id;

    assert_eq!(
        path_of(ws, &co, &titled).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/timeline.md"),
        "the pre-existing id-named folder must be adopted, not joined by a titled twin"
    );
}

/// The name is composed under the same rule every other minted name obeys,
/// and the id half survives every input the title half can throw at it.
#[test]
fn a_task_folder_name_is_readable_first_and_addressable_last() {
    use super::super::workspace_names::is_kebab_name;

    let ulid = "01hq8zm4xk3n7y2p9v1w5c8t4b";

    // No title at all — a caller with no board record — is exactly what
    // every folder was called before this.
    assert_eq!(task_folder_name(ulid, None), ulid);

    // The ordinary case: readable half first, id last, one workspace name.
    let named = task_folder_name(ulid, Some("Q3 Launch Brief"));
    assert_eq!(named, format!("q3-launch-brief.{ulid}"));
    assert!(is_kebab_name(&named), "{named}");

    // A title that normalizes to nothing must not collapse every such card
    // onto `untitled.<id>`; the id alone is both shorter and truer.
    assert_eq!(task_folder_name(ulid, Some("🎉 ✨")), ulid);
    assert_eq!(task_folder_name(ulid, Some("   ")), ulid);

    // But a card an operator actually titled "Untitled" has a real name,
    // and it must keep it: `kebab_name` answers `untitled` for that title
    // and for `🎉` alike, and only the second of those has nothing to say.
    assert_eq!(
        task_folder_name(ulid, Some("Untitled")),
        format!("untitled.{ulid}"),
        "a real title must not be mistaken for the fallback it collides with"
    );

    // The id half is read back whole, whatever the title half held — the
    // one property the lookup depends on.
    assert_eq!(
        task_folder_task_id(&task_folder_name("fix-login", Some("v1.2 Plan"))),
        Some("fix-login")
    );

    // A long title is trimmed to whatever the id leaves — never the id, a
    // partial ULID being no id at all — and leaves no dangling separator.
    let long = task_folder_name(ulid, Some(&"Very Long Card Title ".repeat(20)));
    assert!(long.len() <= MAX_NAME_BYTES, "{} bytes", long.len());
    assert!(long.ends_with(ulid), "{long}");
    assert!(is_kebab_name(&long), "{long}");
}

/// One card's id being the tail of another's must not file one card's
/// deliverables in the other's folder.
///
/// A seed card's id is `[a-z0-9-]` (`task_file::normalize_task_id`), so
/// `login` and `fix-login` are both legal ids on one board, and a dash
/// join would leave `password-reset-fix-login` ending in `-login` exactly
/// as `login`'s own folder does. The shorter card would then adopt the
/// longer card's folder — silently, and overwriting its deliverables
/// wherever the two publish the same source path — and once both had
/// published, every later lookup for `login` would match two folders. The
/// dot boundary makes the id half an equality test rather than a suffix
/// search, so neither can reach the other.
#[tokio::test]
async fn a_card_whose_id_ends_in_another_cards_id_keeps_its_own_folder() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let longer = materialize(
        ws,
        &co,
        PublishTarget {
            task_id: "fix-login",
            task_title: Some("Password reset"),
            ..target("notes.md", "# Notes")
        },
    )
    .await
    .expect("publish for `fix-login`")
    .node_id;
    assert_eq!(
        path_of(ws, &co, &longer).await,
        format!("{ARTIFACTS_ROOT}/cmo/password-reset.fix-login/notes.md")
    );

    let shorter = materialize(
        ws,
        &co,
        PublishTarget {
            task_id: "login",
            task_title: Some("Login page"),
            ..target("spec.md", "# Spec")
        },
    )
    .await
    .expect("publish for `login`")
    .node_id;
    assert_eq!(
        path_of(ws, &co, &shorter).await,
        format!("{ARTIFACTS_ROOT}/cmo/login-page.login/spec.md"),
        "`login` must mint its own folder, not adopt the one `fix-login` already has"
    );
}

/// A note wearing the task's id is refused even when a folder matches too.
///
/// No backend enforces unique sibling names, so a legacy or imported tree
/// can carry a note and a folder under one name. Publishing into the folder
/// would land the deliverable at a path the agents' `PathIndex` reads as
/// ambiguous — a note the agent that just wrote it could not open again —
/// so the wrong kind is checked before any folder is chosen, not only when
/// no folder matched.
#[tokio::test]
async fn a_note_wearing_the_task_id_is_refused_even_beside_a_matching_folder() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(
        ws,
        &co,
        PublishTarget {
            task_title: Some("Launch brief"),
            ..target("launch.md", "# Launch")
        },
    )
    .await
    .expect("first publish")
    .node_id;
    let tree = ws.tree(&co).await.expect("tree");
    let published = tree.iter().find(|n| n.id == first).expect("published node");
    let task_folder = tree
        .iter()
        .find(|n| Some(&n.id) == published.parent_id.as_ref())
        .expect("task folder");
    let agent_folder = task_folder.parent_id.clone().expect("agent folder");

    // The shape an import can leave behind: a note carrying the bare task
    // id, beside the folder that actually holds the deliverables.
    let note = WorkspaceNode {
        id: "imported-note".to_string(),
        name: "t-1".to_string(),
        kind: NodeKind::File,
        parent_id: Some(agent_folder),
        updated_at_millis: 1,
        created_by: origin("cmo"),
        updated_by: origin("cmo"),
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    };
    ws.create(&co, &note, Some("imported"))
        .await
        .expect("imported note");

    let refused = materialize(
        ws,
        &co,
        PublishTarget {
            task_title: Some("Launch brief"),
            ..target("timeline.md", "# Timeline")
        },
    )
    .await;
    assert!(
        matches!(refused, Err(OpenCompanyError::Conflict(_))),
        "a matching note must fail the publish closed rather than be stepped over: {refused:?}"
    );
}

/// Two folders for one task converge on one of them, rather than refusing
/// every publish that follows.
///
/// The create is atomic under the store's lock but keyed by *name*, and two
/// first publishes of one card can compute two names — one caller holding
/// the title and one holding `None`. Answering the result with `Conflict`
/// would make a race lasting microseconds refuse that task's publishes
/// permanently, which is the failure `resolve_folder` exists to prevent.
/// Both folders were matched on the card's own immutable id, so there is no
/// identity to guess at: the older wins, on every later publish and in
/// every process.
#[tokio::test]
async fn two_folders_for_one_task_resolve_to_the_oldest_rather_than_refusing() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(ws, &co, target("launch.md", "# Launch"))
        .await
        .expect("untitled first publish")
        .node_id;
    let tree = ws.tree(&co).await.expect("tree");
    let published = tree.iter().find(|n| n.id == first).expect("published node");
    let task_folder = tree
        .iter()
        .find(|n| Some(&n.id) == published.parent_id.as_ref())
        .expect("task folder");
    assert_eq!(task_folder.name, "t-1");
    let agent_folder = task_folder.parent_id.clone().expect("agent folder");

    // What a concurrent first publish holding the card's title would have
    // left behind: the same task, under a second name.
    let twin = ws
        .adopt_or_create_folder(&co, Some(&agent_folder), "launch-brief.t-1", origin("cmo"))
        .await
        .expect("twin folder")
        .into_node();
    assert!(
        twin.id > task_folder.id,
        "the twin must be the younger of the two for this to prove anything"
    );

    let second = materialize(
        ws,
        &co,
        PublishTarget {
            task_title: Some("Launch brief"),
            ..target("timeline.md", "# Timeline")
        },
    )
    .await
    .expect("a task with two folders must still be publishable")
    .node_id;
    assert_eq!(
        path_of(ws, &co, &second).await,
        format!("{ARTIFACTS_ROOT}/cmo/t-1/timeline.md"),
        "the older of the two folders must win, and win the same way every time"
    );
}

// -- no empty / duplicate folders (#1801) -------------------------------

/// Publishing one deliverable twice lands exactly one `artifacts/<agent>/`
/// and one task folder beneath it, and leaves no empty folder anywhere —
/// the compensating rollback added for the failure path must not fire on a
/// publish that succeeds.
#[tokio::test]
async fn republishing_one_deliverable_leaves_one_folder_chain_and_no_empties() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize(ws, &co, target("launch.md", "v1"))
        .await
        .unwrap()
        .node_id;
    let second = materialize(ws, &co, target("launch.md", "v2"))
        .await
        .unwrap()
        .node_id;
    assert_eq!(first, second, "one deliverable, one node");

    let nodes = ws.tree(&co).await.unwrap();
    assert_eq!(
        nodes.iter().filter(|n| n.name == "cmo").count(),
        1,
        "exactly one agent folder: {nodes:?}"
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|n| n.name == "t-1" || task_folder_task_id(&n.name) == Some("t-1"))
            .count(),
        1,
        "exactly one task folder: {nodes:?}"
    );
    for folder in nodes.iter().filter(|n| n.kind == NodeKind::Folder) {
        assert!(
            nodes
                .iter()
                .any(|n| n.parent_id.as_deref() == Some(folder.id.as_str())),
            "`{}` is an empty folder a successful publish must not leave: {nodes:?}",
            folder.name
        );
    }

    let (_, body) = ws.read(&co, &second).await.unwrap().unwrap();
    assert_eq!(body, "v2");
}

/// A first publish whose note create is refused **after** its folders were
/// minted must leave no empty `artifacts/<agent>/…` skeleton behind — the
/// residual, non-race seam this issue is about. `RefusingCreate` mints
/// folders but refuses the note, the exact shape a quota refusal takes on a
/// first publish.
#[tokio::test]
async fn a_refused_first_publish_leaves_no_empty_folders() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();
    let refusing = RefusingCreate(ops.clone());

    let err = materialize(&refusing, &co, target("launch.md", "# Launch"))
        .await
        .expect_err("the refused note create must fail the publish");
    assert!(
        err.to_string().contains("over quota"),
        "the store's refusal must reach the caller: {err}"
    );

    let nodes = ws.tree(&co).await.unwrap();
    assert!(
        !nodes.iter().any(|n| n.name == "cmo"),
        "the empty agent folder minted for the refused publish must be swept: {nodes:?}"
    );
    assert!(
        !nodes
            .iter()
            .any(|n| n.name == "t-1" || task_folder_task_id(&n.name) == Some("t-1")),
        "and so must the empty task folder: {nodes:?}"
    );
    assert!(
        nodes
            .iter()
            .all(|n| n.kind != NodeKind::Folder || n.name == ARTIFACTS_ROOT),
        "no folder beneath the root should survive a fully-refused publish: {nodes:?}"
    );
}

/// Issue #1839, the other side of the same refused publish: when a rival
/// **adopts** the folder this publish minted in the write window, the
/// rollback must leave it standing rather than sweep the folder the rival is
/// about to write into. `AdoptParentThenRefuse` takes the lease on the task
/// folder the instant before the note create is refused — the exact race —
/// so the agent and task folders survive despite the failed publish.
#[tokio::test]
async fn a_folder_a_rival_adopted_survives_the_refused_publish() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();
    let racing = AdoptParentThenRefuse(ops.clone());

    let err = materialize(&racing, &co, target("launch.md", "# Launch"))
        .await
        .expect_err("the refused note create must still fail the publish");
    assert!(
        err.to_string().contains("over quota"),
        "the store's refusal must reach the caller: {err}"
    );

    let nodes = ws.tree(&co).await.unwrap();
    assert!(
        nodes.iter().any(|n| n.name == "cmo"),
        "the agent folder a rival adopted must survive the minter's rollback: {nodes:?}"
    );
    assert!(
        nodes
            .iter()
            .any(|n| n.name == "t-1" || task_folder_task_id(&n.name) == Some("t-1")),
        "and the adopted task folder it parents must survive too: {nodes:?}"
    );
}

/// Two workflow nodes whose raw ids differ only by underscore vs dash
/// still capture to distinct folders.
///
/// `write_up` and `write-up` both kebab-normalize to `write-up`. Workflow
/// validation only requires the RAW ids to be unique, so both are legal
/// node ids in one graph. Without a raw-id-derived suffix on the path
/// segment, the second node's capture would resolve to the same
/// destination as the first and silently overwrite its output.
#[tokio::test]
async fn run_nodes_with_colliding_kebab_ids_capture_to_distinct_folders() {
    let (_dir, ops, co) = stores();
    let ws: &dyn WorkspaceStore = ops.as_ref();

    let first = materialize_run(
        ws,
        &co,
        RunTarget {
            agent_id: "cmo",
            run_id: "run-1",
            node_id: "write_up",
            source: "notes.md",
            payload: MirrorPayload::Text("first node's body"),
        },
    )
    .await
    .expect("first node capture");

    let second = materialize_run(
        ws,
        &co,
        RunTarget {
            agent_id: "cmo",
            run_id: "run-1",
            node_id: "write-up",
            source: "notes.md",
            payload: MirrorPayload::Text("second node's body"),
        },
    )
    .await
    .expect("second node capture");

    assert_ne!(
        first.node_id, second.node_id,
        "colliding kebab node ids must not resolve to the same workspace node"
    );

    let (_, first_body) = ws
        .read(&co, &first.node_id)
        .await
        .expect("read")
        .expect("first node still present");
    assert_eq!(
        first_body, "first node's body",
        "the second node's capture must not overwrite the first node's output"
    );
}
