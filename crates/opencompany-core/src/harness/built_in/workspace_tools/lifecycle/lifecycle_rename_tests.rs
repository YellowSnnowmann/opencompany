use super::lifecycle_fixtures_tests::*;
use super::*;
use crate::harness::workspace_tools::tests::{TEST_AGENT, agent_origin, file, folder, text, ws};
use crate::ports::types::CompanyId;

// ---------------------------------------------------------------------------
// workspace_rename
// ---------------------------------------------------------------------------

/// A rename is content-, id- and authorship-preserving. All three matter: the
/// id is what every artifact record points at, and the origins are what keep an
/// agent's work attributed after somebody tidies it.
#[tokio::test]
async fn renaming_your_own_note_keeps_its_body_id_and_authorship() {
    let home = own_home("acme").await;

    let out = home
        .renamer()
        .execute(json!({ "path": "agents/ceo/Draft.md", "new_name": "Q3 launch brief.md" }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    let message = text(&out);
    assert!(
        message.contains("agents/ceo/q3-launch-brief.md"),
        "{message}"
    );
    assert!(
        message.contains("rev="),
        "the move restamps the revision, so it has to hand the new one back: {message}"
    );

    let (node, body) = home.read("n-draft").await;
    assert_eq!(node.name, "q3-launch-brief.md");
    assert_eq!(body, "# Draft");
    assert_eq!(node.created_by, agent_origin());
    assert_eq!(node.updated_by, agent_origin());
}

/// `workspace_rename` takes no `expected_updated_at` at all — unlike
/// `workspace_delete` and `workspace_write`, which both refuse a call whose
/// revision has gone stale. Two renames back to back on the same node prove
/// the absence: the second runs against a node the first has already
/// restamped, and nothing about that intervening change is surfaced to it —
/// it neither refuses nor is told the node moved since whatever the caller
/// last read.
#[tokio::test]
async fn a_second_rename_proceeds_with_no_staleness_signal_from_the_first() {
    let home = own_home("acme").await;

    let first = home
        .renamer()
        .execute(json!({ "path": "agents/ceo/Draft.md", "new_name": "First rename.md" }))
        .await
        .unwrap();
    assert!(!first.is_error, "{}", text(&first));
    let stamped_after_first = home.node("n-draft").await.updated_at_millis;

    // Nothing here carries the revision the first rename just stamped — the
    // schema has no field for it — so this call cannot even express "as of
    // what I last saw".
    let second = home
        .renamer()
        .execute(json!({ "id": "n-draft", "new_name": "Second rename.md" }))
        .await
        .unwrap();
    assert!(
        !second.is_error,
        "a rename immediately following another must not be silently blocked either: {}",
        text(&second)
    );

    let (node, _body) = home.read("n-draft").await;
    assert_eq!(node.name, "second-rename.md");
    assert!(
        node.updated_at_millis >= stamped_after_first,
        "the second rename ran unconditionally against whatever the node had become, with no \
         check against what the caller believed it still was"
    );
}

/// Filing a note under a subfolder you made is the other half of tidying, and
/// the destination may be either the home itself or anything inside it.
#[tokio::test]
async fn a_note_can_be_moved_into_and_back_out_of_a_subfolder_of_your_own() {
    let home = own_home("acme").await;
    let tool = home.renamer();

    let out = tool
        .execute(json!({ "path": "agents/ceo/Draft.md", "new_parent": "agents/ceo/archive" }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(
        home.node("n-draft").await.parent_id.as_deref(),
        Some("f-archive")
    );

    // …and back up into the home itself, which is a legal *destination* even
    // though it is never a legal target for a delete or a rename.
    let out = tool
        .execute(json!({ "id": "n-draft", "new_parent": "agents/ceo" }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    assert_eq!(
        home.node("n-draft").await.parent_id,
        Some(home.home_id().await)
    );
}

/// A rename and a move in one call, since the tool advertises both.
#[tokio::test]
async fn a_name_and_a_parent_can_change_in_one_call() {
    let home = own_home("acme").await;

    let out = home
        .renamer()
        .execute(json!({
            "path": "agents/ceo/Draft.md",
            "new_name": "superseded.md",
            "new_parent": "agents/ceo/archive",
        }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    assert!(
        text(&out).contains("agents/ceo/archive/superseded.md"),
        "{}",
        text(&out)
    );
    let node = home.node("n-draft").await;
    assert_eq!(node.name, "superseded.md");
    assert_eq!(node.parent_id.as_deref(), Some("f-archive"));
}

/// A folder must never land inside its own subtree: `archive` → `archive/deep`
/// would make the tree unreadable for every agent from then on. Refused before
/// the store is asked to create the cycle.
#[tokio::test]
async fn a_folder_cannot_be_moved_into_its_own_subfolder() {
    let home = own_home("acme").await;
    let mine = home.home_id().await;
    home.store
        .create(
            &home.company,
            &folder("f-deep", "deep", Some("f-archive")),
            None,
        )
        .await
        .unwrap();

    // By path, into the descendant.
    let out = home
        .renamer()
        .execute(json!({
            "path": "agents/ceo/archive",
            "new_parent": "agents/ceo/archive/deep",
        }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(
        text(&out).contains("unreadable"),
        "the refusal says the tree would be unreadable: {}",
        text(&out)
    );

    // By id, into itself — the same guard at the end of the ancestry walk.
    let out = home
        .renamer()
        .execute(json!({ "id": "f-archive", "new_parent": "agents/ceo/archive" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));

    // The tree is untouched.
    assert_eq!(
        home.node("f-archive").await.parent_id.as_deref(),
        Some(mine.as_str())
    );
}

/// A binary node renames like any other — the port moves the payload with it.
#[tokio::test]
async fn a_binary_node_can_be_renamed_and_keeps_its_payload() {
    let home = own_home("acme").await;
    home.add_own_binary("n-own-img", "chart.png", &[0x89, b'P', b'N', b'G'])
        .await;

    let out = home
        .renamer()
        .execute(json!({ "path": "agents/ceo/chart.png", "new_name": "q3-chart.png" }))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", text(&out));
    let (node, _) = home
        .store
        .read_bytes(&home.company, "n-own-img")
        .await
        .unwrap()
        .expect("the payload survived the rename");
    assert_eq!(node.name, "q3-chart.png");
    assert_eq!(node.size, Some(4));
}

/// The scope gate is the one delete uses, checked on the resolved node — so
/// both argument forms refuse identically outside the agent's folder.
#[tokio::test]
async fn renaming_anything_outside_your_own_folder_is_refused() {
    let home = own_home("acme").await;
    let tool = home.renamer();

    for args in [
        json!({ "path": "standards/engineering-standards.md", "new_name": "mine.md" }),
        json!({ "id": "n-eng", "new_name": "mine.md" }),
        json!({ "path": "agents/cmo/Plan.md", "new_name": "mine.md" }),
        json!({ "id": "n-mate", "new_parent": "agents/ceo" }),
        json!({ "path": "readme.md", "new_name": "mine.md" }),
    ] {
        let out = tool.execute(args.clone()).await.unwrap();
        assert!(out.is_error, "{args} was allowed: {}", text(&out));
        assert!(
            text(&out).contains("outside your own folder"),
            "{args}: {}",
            text(&out)
        );
    }
    assert_eq!(home.node("n-eng").await.name, "Engineering standards.md");
    assert_eq!(home.node("n-mate").await.name, "Plan.md");
}

/// The home folder is the agent's identity anchor for renames too, and for the
/// sharper reason: its name **is** the lookup key.
#[tokio::test]
async fn your_own_home_folder_cannot_be_renamed() {
    let home = own_home("acme").await;

    let out = home
        .renamer()
        .execute(json!({ "path": "agents/ceo", "new_name": "chief-exec" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(
        text(&out).contains("your own folder itself"),
        "{}",
        text(&out)
    );
    assert!(home.tree().await.iter().any(|n| n.name == TEST_AGENT));
}

/// Moving to the workspace root is always refused, and gets the scope answer
/// rather than the parse error an empty path would otherwise produce — the
/// reason is that the root is not the agent's, not that `""` is malformed.
#[tokio::test]
async fn moving_to_the_workspace_root_is_refused_with_the_scope_reason() {
    let home = own_home("acme").await;
    let tool = home.renamer();

    for root in ["", "/", "  ", "//"] {
        let out = tool
            .execute(json!({ "path": "agents/ceo/Draft.md", "new_parent": root }))
            .await
            .unwrap();
        assert!(out.is_error, "`{root}` was allowed: {}", text(&out));
        assert!(
            text(&out).contains("workspace root is outside your own folder"),
            "`{root}`: {}",
            text(&out)
        );
    }
    assert_eq!(
        home.node("n-draft").await.parent_id,
        Some(home.home_id().await),
        "a refused move relocated the note anyway"
    );
}

/// A destination outside the agent's folder is refused even when the node
/// itself is inside it — otherwise "confined to your own folder" would only be
/// half true, and the escape would be a one-call move.
#[tokio::test]
async fn moving_your_own_note_out_of_your_folder_is_refused() {
    let home = own_home("acme").await;
    let tool = home.renamer();

    for parent in ["standards", "agents", "agents/cmo"] {
        let out = tool
            .execute(json!({ "path": "agents/ceo/Draft.md", "new_parent": parent }))
            .await
            .unwrap();
        assert!(out.is_error, "`{parent}` was allowed: {}", text(&out));
        assert!(
            text(&out).contains("outside your own folder"),
            "`{parent}`: {}",
            text(&out)
        );
    }
    assert_eq!(
        home.node("n-draft").await.parent_id,
        Some(home.home_id().await),
        "a refused move relocated the note anyway"
    );
}

/// The `fs` backend rejects an unsafe name, but sqlite and mongodb do not — so
/// the tool layer validates rather than relying on whichever backend is wired.
#[tokio::test]
async fn a_new_name_that_is_not_a_single_segment_is_refused() {
    let home = own_home("acme").await;
    let tool = home.renamer();

    for name in ["..", ".", "a/b", "a\\b", "sub/dir/note.md"] {
        let out = tool
            .execute(json!({ "path": "agents/ceo/Draft.md", "new_name": name }))
            .await
            .unwrap();
        assert!(out.is_error, "`{name}` was allowed: {}", text(&out));
        assert!(
            text(&out).contains("single path segment"),
            "`{name}`: {}",
            text(&out)
        );
    }
    assert_eq!(home.node("n-draft").await.name, "Draft.md");
}

/// Two nodes at one path make it ambiguous for every agent from then on — the
/// argument `workspace_create` refuses on, applied to the other way of reaching
/// the same state.
#[tokio::test]
async fn a_rename_onto_an_occupied_path_is_refused_and_changes_nothing() {
    let home = own_home("acme").await;
    home.add_own(file("n-other", "Notes.md", None), "notes")
        .await;

    let out = home
        .renamer()
        .execute(json!({ "path": "agents/ceo/Draft.md", "new_name": "Notes.md" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(text(&out).contains("already exists"), "{}", text(&out));
    assert_eq!(home.node("n-draft").await.name, "Draft.md");
    assert_eq!(
        home.read("n-other").await.1,
        "notes",
        "the note that was already there must be untouched"
    );
}

/// A rename that names no change is a refusal rather than a silent success —
/// the store would answer `Ok` and announce nothing, which reads to the agent
/// as "the move happened" when nothing moved.
#[tokio::test]
async fn a_rename_that_changes_nothing_says_so() {
    let home = own_home("acme").await;
    let tool = home.renamer();

    for args in [
        json!({ "path": "agents/ceo/Draft.md", "new_name": "Draft.md" }),
        json!({ "path": "agents/ceo/Draft.md", "new_parent": "agents/ceo" }),
    ] {
        let out = tool.execute(args.clone()).await.unwrap();
        assert!(out.is_error, "{args}: {}", text(&out));
        assert!(
            text(&out).contains("already exactly where you asked to put it"),
            "{args}: {}",
            text(&out)
        );
    }
}

#[tokio::test]
async fn a_destination_that_is_missing_or_is_a_note_is_refused_with_what_to_do() {
    let home = own_home("acme").await;
    let tool = home.renamer();

    let out = tool
        .execute(json!({ "path": "agents/ceo/Draft.md", "new_parent": "agents/ceo/nope" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(text(&out).contains("does not exist"), "{}", text(&out));
    assert!(
        text(&out).contains(WORKSPACE_CREATE_TOOL),
        "the refusal must name the tool that fixes it: {}",
        text(&out)
    );

    home.add_own(file("n-other", "Notes.md", None), "notes")
        .await;
    let out = tool
        .execute(json!({ "path": "agents/ceo/Draft.md", "new_parent": "agents/ceo/Notes.md" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(
        text(&out).contains("is a note, not a folder"),
        "{}",
        text(&out)
    );
}

#[tokio::test]
async fn a_rename_needs_at_least_one_of_new_name_and_new_parent() {
    let home = own_home("acme").await;

    let out = home
        .renamer()
        .execute(json!({ "path": "agents/ceo/Draft.md" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(text(&out).contains("new_name"), "{}", text(&out));
    assert!(text(&out).contains("new_parent"), "{}", text(&out));
}

#[tokio::test]
async fn tenancy_a_borrowed_node_id_cannot_be_renamed_by_another_company() {
    let home = own_home("acme").await;
    let intruder = WorkspaceRenameTool::new(ws(home.store.clone(), CompanyId::new("other")));

    let out = intruder
        .execute(json!({ "id": "n-draft", "new_name": "mine.md" }))
        .await
        .unwrap();
    assert!(out.is_error, "{}", text(&out));
    assert!(
        text(&out).contains("No workspace note matches"),
        "{}",
        text(&out)
    );
    assert_eq!(home.node("n-draft").await.name, "Draft.md");
}
