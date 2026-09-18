//! Integration tests for the `ops` write plane: tasks, memory, workspace,
//! skills, team, inbox-read, and desk chat — exercised end-to-end over the
//! router against a real fs-backed company.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::write_test_support::*;
use crate::AppState;
use crate::ports::types::CompanyId;

/// `POST …/workspace/sweep-empty-agent-folders` (issue #700): the operator's
/// one-time tidy of the empty `agents/<id>/` folders a pre-#570 company still
/// carries.
///
/// The whole route in one test, because the halves only mean something together:
/// the dry run has to name every folder *and* leave the tree alone, or the
/// confirm dialog it feeds is either uninformative or a lie; the real run has to
/// remove exactly those folders, leave the occupied one, and announce each
/// removal so a console watching the feed sees the tree change rather than
/// discovering it on the next refetch.
#[tokio::test]
async fn workspace_sweep_previews_then_removes_only_the_empty_agent_folders() {
    use crate::ports::types::CompanyEvent;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();

    // Boot already scaffolded `agents/`; find it rather than making a rival.
    let (_, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    let agents_id = tree
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == "agents")
        .expect("boot scaffolds the Agents root")["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Two strays from the #551 era, one folder that actually holds a
    // deliverable, and a note filed directly under the root by an operator.
    let mut empty = Vec::new();
    for id in ["ceo", "cto"] {
        let (_, folder) = send(
            &state,
            "POST",
            "/api/v1/company/workspace",
            Some(json!({"name": id, "kind": "folder", "parentId": agents_id})),
        )
        .await;
        empty.push(folder["id"].as_str().unwrap().to_string());
    }
    let (_, cmo) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({"name": "cmo", "kind": "folder", "parentId": agents_id})),
    )
    .await;
    send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({
            "name": "launch-brief.md",
            "kind": "file",
            "parentId": cmo["id"].as_str().unwrap(),
            "content": "# Launch",
        })),
    )
    .await;
    send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({
            "name": "README.md",
            "kind": "file",
            "parentId": agents_id,
            "content": "# who is who",
        })),
    )
    .await;

    let before = {
        let (_, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
        provisioned_names(&tree)
    };
    let events_before = journal_len(&runtime).await;

    // -- the preview ------------------------------------------------------
    let (status, preview) = send(
        &state,
        "POST",
        "/api/v1/company/workspace/sweep-empty-agent-folders?dry_run=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        swept_names(&preview["wouldRemove"]),
        vec!["ceo", "cto"],
        "the confirm dialog needs every folder named, not a count: {preview}"
    );
    assert!(
        preview.get("removed").is_none(),
        "a preview must not claim it removed anything: {preview}"
    );
    let (_, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    assert_eq!(
        provisioned_names(&tree),
        before,
        "a dry run must leave the tree exactly as it found it"
    );
    assert_eq!(
        journal_len(&runtime).await,
        events_before,
        "a dry run must not announce anything either"
    );

    // -- the real thing ---------------------------------------------------
    let (status, done) = send(
        &state,
        "POST",
        "/api/v1/company/workspace/sweep-empty-agent-folders",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        swept_names(&done["removed"]),
        vec!["ceo", "cto"],
        "an operator who disagrees needs to know what went: {done}"
    );
    assert!(
        done.get("wouldRemove").is_none(),
        "a real run must not answer in the preview's field: {done}"
    );

    let (_, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    assert_eq!(
        provisioned_names(&tree),
        vec![
            "agents".to_string(),
            "artifacts".to_string(),
            "cmo".to_string(),
            "launch-brief.md".to_string(),
            "readme.md".to_string(),
            "readme.md".to_string(),
            "readme.md".to_string(),
            "secrets".to_string(),
        ],
        "the folder holding a deliverable, the operator's note and the root all stay"
    );

    // One `WorkspaceChanged{removed}` per folder — the announcer is reached
    // because the handler deletes through `runtime.workspace()`, the same
    // wrapped handle the per-node delete uses (issue #327).
    let journal = runtime
        .events()
        .read_from(
            runtime.id(),
            crate::ports::types::EventSeq::new(0),
            usize::MAX,
        )
        .await
        .unwrap();
    //
    // Sorted on both sides rather than compared in creation order: the sweep
    // walks whatever order `tree()` returns, and the port promises none.
    let mut announced: Vec<&str> = journal
        .iter()
        .filter_map(|stored| match &stored.event {
            CompanyEvent::WorkspaceChanged { node_id, change } if change == "removed" => {
                Some(node_id.as_str())
            }
            _ => None,
        })
        .collect();
    announced.sort_unstable();
    let mut expected: Vec<&str> = empty.iter().map(String::as_str).collect();
    expected.sort_unstable();
    assert_eq!(
        announced, expected,
        "each removal announces itself, exactly once, and nothing else was announced removed"
    );

    // -- and again, which must be a no-op ---------------------------------
    let (status, again) = send(
        &state,
        "POST",
        "/api/v1/company/workspace/sweep-empty-agent-folders",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        again["removed"],
        json!([]),
        "running it twice must remove nothing the second time: {again}"
    );

    // The route resolves under the platform scope form too, and is never
    // captured as a node id by the `…/workspace/{node_id}` route.
    let (status, scoped) = send(
        &state,
        "POST",
        "/api/v1/companies/acme/workspace/sweep-empty-agent-folders?dry_run=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(scoped["wouldRemove"], json!([]));
}

/// `POST …/workspace/merge-duplicate-folders` (issue #759): the operator's
/// repair for a tree a publish race already left ambiguous.
///
/// The whole route in one test, because the halves only mean anything together.
/// A preview that did not name the relocations is a confirm dialog nobody can
/// agree to; a real run that reported only its successes would call a tree fixed
/// while two rival documents still sit on one path; and a repair that could not
/// be run twice would be useless precisely on the tenant that needs it, since
/// the first pass deliberately leaves the file collision behind.
///
/// The workspace here is the permissive double, not `FsOps`: the `fs` backend
/// refuses to create the duplicate in the first place (issue #665), so this
/// state is only reachable on the sqlite and mongodb backends hosted tenants
/// actually run.
#[tokio::test]
async fn workspace_merge_folds_duplicate_folders_and_reports_the_file_collision() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_workspace(
        &home,
        std::sync::Arc::new(
            crate::company::workspace_repair::loose_store::LooseWorkspace::default(),
        ),
    )
    .await;
    let runtime = state.registry().get(&CompanyId::new("acme")).unwrap();

    async fn make(state: &AppState, body: Value) -> String {
        let (status, node) = send(state, "POST", "/api/v1/company/workspace", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{node}");
        node["id"].as_str().expect("an id").to_string()
    }

    // The raced state: one deliverable folder published twice, each copy
    // holding a different note — and both holding a `summary.md`, which is two
    // documents on one path and the thing no merge may decide.
    let a = make(&state, json!({"name": "reports", "kind": "folder"})).await;
    let b = make(&state, json!({"name": "reports", "kind": "folder"})).await;
    let a_note = make(
        &state,
        json!({"name": "q1.md", "kind": "file", "parentId": a, "content": "# Q1"}),
    )
    .await;
    let b_note = make(
        &state,
        json!({"name": "q2.md", "kind": "file", "parentId": b, "content": "# Q2"}),
    )
    .await;
    let a_summary = make(
        &state,
        json!({"name": "summary.md", "kind": "file", "parentId": a, "content": "# Mine"}),
    )
    .await;
    let b_summary = make(
        &state,
        json!({"name": "summary.md", "kind": "file", "parentId": b, "content": "# Theirs"}),
    )
    .await;

    let before = {
        let (_, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
        provisioned_names(&tree)
    };
    let events_before = journal_len(&runtime).await;

    // -- the preview --------------------------------------------------------
    let (status, preview) = send(
        &state,
        "POST",
        "/api/v1/company/workspace/merge-duplicate-folders?dry_run=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let folds = preview["wouldMerge"]
        .as_array()
        .unwrap_or_else(|| panic!("a preview answers with a list of folds, got {preview}"));
    assert_eq!(folds.len(), 1, "{preview}");
    let fold = &folds[0];

    // Which twin survives is derived rather than hard-coded: two ULIDs minted
    // in the same millisecond order by their random half, so the harness cannot
    // know in advance which folder is older. The rule itself — oldest wins, node
    // id breaks the tie — is pinned in `company::workspace_repair`'s own tests,
    // where the timestamps are given.
    let loser = fold["id"]
        .as_str()
        .expect("the fold names its loser")
        .to_string();
    let winner = fold["intoId"]
        .as_str()
        .expect("and its survivor")
        .to_string();
    assert!(
        (loser == a && winner == b) || (loser == b && winner == a),
        "the fold must be between the two `reports` folders, got {fold}"
    );
    let (moved, residual) = if loser == a {
        (a_note.clone(), a_summary.clone())
    } else {
        (b_note.clone(), b_summary.clone())
    };

    assert_eq!(
        fold["moved"].as_array().map(|m| m
            .iter()
            .map(|n| n["id"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()),
        Some(vec![moved.as_str()]),
        "the operator is shown every note that would change hands: {preview}"
    );
    assert_eq!(
        fold["removed"],
        json!(false),
        "the duplicate still holds a rival document, so it cannot go: {preview}"
    );
    assert_eq!(
        preview["residuals"].as_array().map(|r| r
            .iter()
            .map(|n| (
                n["id"].as_str().unwrap_or_default(),
                n["cause"].as_str().unwrap_or_default()
            ))
            .collect::<Vec<_>>()),
        Some(vec![(residual.as_str(), "fileInTheWay")]),
        "and told exactly which document is still theirs to settle: {preview}"
    );
    assert!(
        preview.get("merged").is_none(),
        "a preview must not claim it changed anything: {preview}"
    );
    let (_, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    assert_eq!(
        provisioned_names(&tree),
        before,
        "a dry run must leave the tree exactly as it found it"
    );
    assert_eq!(
        journal_len(&runtime).await,
        events_before,
        "a dry run must not announce anything either"
    );

    // -- the real thing -----------------------------------------------------
    let (status, done) = send(
        &state,
        "POST",
        "/api/v1/company/workspace/merge-duplicate-folders",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        done.get("wouldMerge").is_none(),
        "a real run must not answer in the preview's field: {done}"
    );
    assert_eq!(done["merged"][0]["id"], json!(loser));
    assert_eq!(done["merged"][0]["removed"], json!(false));
    assert_eq!(done["residuals"][0]["id"], json!(residual));

    let (_, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    let parents: std::collections::HashMap<&str, &str> = tree
        .as_array()
        .unwrap()
        .iter()
        .map(|node| {
            (
                node["id"].as_str().unwrap_or_default(),
                node["parentId"].as_str().unwrap_or("-"),
            )
        })
        .collect();
    assert_eq!(
        parents.get(moved.as_str()),
        Some(&winner.as_str()),
        "the note moved into the surviving folder, under the id it was published as"
    );
    assert_eq!(
        parents.get(residual.as_str()),
        Some(&loser.as_str()),
        "and the rival document did not move at all"
    );
    assert!(
        parents.contains_key(loser.as_str()),
        "the duplicate folder still holds something, so it must still be there"
    );

    // The move announces itself, because the repair runs through
    // `runtime.workspace()` — the same announcer-wrapped handle the per-node
    // routes use (issue #327).
    assert!(
        workspace_changes(&runtime)
            .await
            .contains(&(moved.clone(), "updated".to_string())),
        "an open console must see the note change hands"
    );

    // -- the operator settles the collision, and runs it again --------------
    let (status, _) = send(
        &state,
        "DELETE",
        &format!("/api/v1/company/workspace/{residual}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, again) = send(
        &state,
        "POST",
        "/api/v1/company/workspace/merge-duplicate-folders",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        again["merged"][0]["removed"],
        json!(true),
        "with nothing left in it, the duplicate finally goes: {again}"
    );
    assert_eq!(again["residuals"], json!([]));
    let (_, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    assert!(
        !tree
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["id"] == json!(loser)),
        "the duplicate is gone from the tree: {tree}"
    );
    assert!(
        workspace_changes(&runtime)
            .await
            .contains(&(loser.clone(), "removed".to_string())),
        "and its removal was announced too"
    );

    // -- and once more, which must be a no-op -------------------------------
    let (status, third) = send(
        &state,
        "POST",
        "/api/v1/company/workspace/merge-duplicate-folders",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(third["merged"], json!([]), "nothing left to merge: {third}");
    assert_eq!(third["residuals"], json!([]));

    // The route resolves under the platform scope form too, and is never
    // captured as a node id by the `…/workspace/{node_id}` route.
    let (status, scoped) = send(
        &state,
        "POST",
        "/api/v1/companies/acme/workspace/merge-duplicate-folders?dry_run=true",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(scoped["wouldMerge"], json!([]));
}
