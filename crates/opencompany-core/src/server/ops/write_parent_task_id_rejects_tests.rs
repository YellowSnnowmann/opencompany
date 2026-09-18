//! Integration tests for the `ops` write plane: tasks, memory, workspace,
//! skills, team, inbox-read, and desk chat — exercised end-to-end over the
//! router against a real fs-backed company.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::write_test_support::*;

/// #185 review follow-up: the lineage forest is enforced at the write boundary.
///
/// Without this a card could be its own parent (appearing as both parent and
/// child of itself in `task_detail`), point at a card that does not exist, or
/// close a `t1 → t2 → t1` loop — all persisted silently.
#[tokio::test]
async fn parent_task_id_rejects_self_unknown_and_cycles() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let create = |title: &str| {
        let title = title.to_string();
        async move { json!({ "title": title }) }
    };
    let (_, a) = send(
        &state,
        "POST",
        "/api/v1/company/tasks",
        Some(create("A").await),
    )
    .await;
    let (_, b) = send(
        &state,
        "POST",
        "/api/v1/company/tasks",
        Some(create("B").await),
    )
    .await;
    let (a_id, b_id) = (
        a["id"].as_str().unwrap().to_string(),
        b["id"].as_str().unwrap().to_string(),
    );

    // Unknown parent on create.
    let (status, _) = send(
        &state,
        "POST",
        "/api/v1/company/tasks",
        Some(json!({ "title": "C", "parentTaskId": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Self-parenting on patch.
    let (status, _) = send(
        &state,
        "PATCH",
        &format!("/api/v1/company/tasks/{a_id}"),
        Some(json!({ "parentTaskId": a_id })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A legitimate edge: B's parent is A.
    let (status, _) = send(
        &state,
        "PATCH",
        &format!("/api/v1/company/tasks/{b_id}"),
        Some(json!({ "parentTaskId": a_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // …which makes A → B a cycle.
    let (status, _) = send(
        &state,
        "PATCH",
        &format!("/api/v1/company/tasks/{a_id}"),
        Some(json!({ "parentTaskId": b_id })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "A → B → A must be rejected"
    );
}

/// #185 review follow-up: validation is only as good as its atomicity.
///
/// Each half of `A → B` / `B → A` is individually legal against a board that
/// has neither edge yet. Read → validate → write therefore has to be one
/// critical section: without it both requests can validate against a snapshot
/// taken before the other wrote, and the pair persists the very cycle
/// `validate_parent` exists to reject.
///
/// With the writes serialized this is deterministic rather than probabilistic —
/// whichever request takes the lock second sees the first one's edge and is
/// rejected — so the assertion is *exactly* one success, not "usually one".
#[tokio::test]
async fn concurrent_reparents_cannot_race_a_cycle_onto_the_board() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = std::sync::Arc::new(state_with_company(&home).await);

    let mut ids = Vec::new();
    for title in ["A", "B"] {
        let (_, card) = send(
            &state,
            "POST",
            "/api/v1/company/tasks",
            Some(json!({ "title": title })),
        )
        .await;
        ids.push(card["id"].as_str().unwrap().to_string());
    }
    let (a_id, b_id) = (ids[0].clone(), ids[1].clone());

    // Fire both halves of the would-be cycle at once.
    let reparent = |child: String, parent: String| {
        let state = state.clone();
        tokio::spawn(async move {
            send(
                &state,
                "PATCH",
                &format!("/api/v1/company/tasks/{child}"),
                Some(json!({ "parentTaskId": parent })),
            )
            .await
            .0
        })
    };
    let first = reparent(b_id.clone(), a_id.clone());
    let second = reparent(a_id.clone(), b_id.clone());
    let (first, second) = (first.await.unwrap(), second.await.unwrap());

    let outcomes = [first, second];
    assert_eq!(
        outcomes.iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "exactly one re-parent may win: {outcomes:?}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|s| **s == StatusCode::BAD_REQUEST)
            .count(),
        1,
        "the loser must be rejected as a cycle, not silently applied: {outcomes:?}"
    );

    // And the board itself is a forest: the two cards cannot both have parents.
    let (_, board) = send(&state, "GET", "/api/v1/company/tasks", None).await;
    let parented = board
        .as_array()
        .expect("board is a list")
        .iter()
        .filter(|c| c["parentTaskId"].is_string())
        .count();
    assert_eq!(parented, 1, "a cycle reached the board: {board}");
}

// ---------------------------------------------------------------------------
// Who may decide on the company's behalf (issue #403)
// ---------------------------------------------------------------------------

/// Every route that decides what this company reaches the outside world *as* —
/// which credential it presents, which third-party account its agents act
/// through, where its mail and its model calls go — refuses a member.
///
/// One table rather than a test per module, on purpose. The gap issue #403
/// reported was not that one route forgot a check; it was that a whole plane
/// shared an extractor whose name did not suggest "any member may write". A
/// per-module test would have let the next route added to that plane be added
/// without one. This list is the plane, and a new route joins it here.
///
/// The assertion is `403` specifically, not merely "not 200": a `404` or a
/// `409` would also be non-200 while meaning the route simply did not run, and
/// that would pass a test which proves nothing.
#[tokio::test]
async fn a_member_cannot_change_what_the_company_reaches_the_world_as() {
    let home_dir = home();
    let state = state_with_company(home_dir.path()).await;
    let member =
        crate::server::test_support::seed_session(&state, "acme", crate::ports::UserRole::Member)
            .await;

    let cases: Vec<(&str, &str, Option<Value>)> = vec![
        // The company's Composio identity, and the accounts its agents use.
        (
            "PUT",
            "/api/v1/company/composio/token",
            Some(json!({ "token": "x" })),
        ),
        (
            "POST",
            "/api/v1/company/composio/authorize",
            Some(json!({ "toolkit": "gmail" })),
        ),
        // Revoking one of those accounts is the same decision as choosing it
        // (issue #404) — a member who cannot connect must not be able to
        // disconnect either.
        (
            "DELETE",
            "/api/v1/company/composio/connections/conn-1",
            None,
        ),
        // And choosing WHICH of two accounts every agent acts as (issue #820) —
        // the same decision again, one step finer: it does not change what the
        // company is connected to, only what it sends as, which is precisely
        // the kind of company-wide answer this plane exists to hold.
        (
            "PUT",
            "/api/v1/company/composio/connections/conn-1/default",
            None,
        ),
        (
            "DELETE",
            "/api/v1/company/composio/connections/conn-1/default",
            None,
        ),
        // The model every agent thinks with, and the key it is billed against.
        (
            "PUT",
            "/api/v1/company/inference",
            Some(json!({ "provider": "openai_compatible", "baseUrl": "https://example.test" })),
        ),
        ("DELETE", "/api/v1/company/inference", None),
        // The company's outbound mail identity — and a send from its address.
        (
            "PUT",
            "/api/v1/company/smtp",
            Some(
                json!({ "provider": "smtp", "host": "mail.example.test", "port": 587,
                         "username": "u", "password": "p", "from_email": "a@example.test" }),
            ),
        ),
        (
            "POST",
            "/api/v1/company/smtp/test",
            Some(json!({ "to": "elsewhere@example.test" })),
        ),
        (
            "PUT",
            "/api/v1/company/domain",
            Some(json!({"domain": "x.test"})),
        ),
        // Which tool servers exist, and the credentials they carry.
        (
            "POST",
            "/api/v1/company/mcp/servers",
            Some(json!({ "name": "evil", "endpoint": "https://example.test" })),
        ),
        (
            "PUT",
            "/api/v1/company/mcp/servers/anything",
            Some(json!({ "endpoint": "https://example.test" })),
        ),
        ("DELETE", "/api/v1/company/mcp/servers/anything", None),
        (
            "POST",
            "/api/v1/company/mcp/servers/anything/oauth/start",
            None,
        ),
    ];

    for (method, uri, body) in cases {
        let (status, response) = send_cookie(&state, method, uri, body, &member).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} let a member through: {response}"
        );
        assert_eq!(
            response["code"], "forbidden",
            "{method} {uri} refused without saying why: {response}"
        );
    }
}

/// The counterpart to the table above, on the same two surfaces: a member is
/// let *through* the reads.
///
/// `docs/modules/server/authority.md` asserts in prose that reads on these
/// surfaces stay open to any member — they carry non-secret routing and never a
/// credential — and until now nothing pinned it. `GET …/domain` and
/// `GET …/smtp` are new (issue #1460), and the easy mistake when adding a route
/// to a module whose every other handler takes `AdminScopedCompany` is to reach
/// for the same extractor: the console's Settings screen would then `403` for
/// every member while the identical data stayed readable to them over GraphQL
/// as `Company.domain` and `Company.smtp`.
///
/// `200` specifically, not merely "not 403": these read stored config that may
/// be absent, and both answer that case with a body rather than a status, so
/// anything else would mean the route did not run.
#[tokio::test]
async fn a_member_may_read_what_the_company_reaches_the_world_as() {
    let home_dir = home();
    let state = state_with_company(home_dir.path()).await;
    let member =
        crate::server::test_support::seed_session(&state, "acme", crate::ports::UserRole::Member)
            .await;

    for uri in ["/api/v1/company/domain", "/api/v1/company/smtp"] {
        let (status, response) = send_cookie(&state, "GET", uri, None, &member).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "GET {uri} refused a member: {response}"
        );
    }
}

/// The other side, on the same table: the harness admin is refused by none of
/// them on role grounds.
///
/// Several answer `409`/`404`/`502` for their own reasons — no feature in this
/// build, no such server, no reachable host — and that is the point. What must
/// never appear is `403`, which would mean the guard caught the wrong person
/// and the fix had quietly removed the capability instead of assigning it.
#[tokio::test]
async fn an_admin_is_refused_by_none_of_them() {
    let home_dir = home();
    let state = state_with_company(home_dir.path()).await;

    let cases: Vec<(&str, &str, Option<Value>)> = vec![
        (
            "PUT",
            "/api/v1/company/composio/token",
            Some(json!({ "token": "x" })),
        ),
        (
            "PUT",
            "/api/v1/company/domain",
            Some(json!({"domain": "x.test"})),
        ),
        (
            "POST",
            "/api/v1/company/mcp/servers",
            Some(json!({ "name": "svc", "endpoint": "https://example.test" })),
        ),
    ];

    for (method, uri, body) in cases {
        let (status, response) = send(&state, method, uri, body).await;
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} refused an admin: {response}"
        );
    }
}

/// Issue #552: a published deliverable lives on two surfaces, and the console's
/// workspace `PUT` is where an operator edits one. Saving that note must record
/// an **operator version** on the artifact chain, because that edit is exactly
/// the datum `human_edit_diff` exists to answer — and overwriting only the node
/// would leave the history claiming the agent's draft shipped unchanged.
///
/// The ordering is asserted too, by refusing the node write: an artifact
/// stamped with a node id the tree does not have makes the chain append
/// succeed and the node write fail, and the version must still be there
/// afterwards. Chain-ahead-of-node is the survivable direction and
/// node-ahead-of-chain is the silent one, so a failed save must land on the
/// first.
#[tokio::test]
async fn saving_a_published_note_records_the_operators_edit_on_the_artifact() {
    use crate::ports::artifacts::{ArtifactKind, ArtifactRecord, ArtifactStore};
    use crate::ports::workspace::WorkspaceStore;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let company = state.registry().list()[0].clone();
    let runtime = state.registry().get(&company).expect("company");

    // A note in the tree…
    let (status, note) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({"name": "launch.md", "kind": "file", "content": "the agent's draft"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let node_id = note["id"].as_str().expect("node id").to_string();

    // …that is the projection of a published artifact.
    let mut published = ArtifactRecord::new(
        "art-1",
        "t-1",
        "Launch spec",
        ArtifactKind::Markdown,
        "the agent's draft",
        "ceo",
        1,
    )
    .with_source("launch.md");
    published.stamp_workspace_node(&node_id);
    ArtifactStore::upsert(runtime.artifacts().as_ref(), &company, &published)
        .await
        .expect("seed");

    let (status, _) = send(
        &state,
        "PUT",
        &format!("/api/v1/company/workspace/file/{node_id}"),
        Some(json!({"content": "the operator's rewrite"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, artifact) = send(&state, "GET", "/api/v1/company/artifacts/art-1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(artifact["versions"].as_array().unwrap().len(), 2);
    assert_eq!(artifact["versions"][1]["body"], "the operator's rewrite");
    assert_eq!(artifact["versions"][1]["author"], "operator");
    assert_eq!(
        artifact["versions"][1]["note"], "operator edit before approval",
        "the wording the console recognises, shared with the append route"
    );
    assert_eq!(
        artifact["versions"][1]["workspaceNodeId"], node_id,
        "the appended version must inherit the node, or the NEXT save mirrors nothing"
    );
    assert!(
        artifact["humanEditDiff"].is_object(),
        "the whole point: a console edit of a deliverable is now diffable"
    );

    // And the node itself carries the operator's text.
    let (node, body) = WorkspaceStore::read(runtime.workspace().as_ref(), &company, &node_id)
        .await
        .unwrap()
        .expect("the note still exists");
    assert_eq!(body, "the operator's rewrite");
    assert_eq!(
        node.updated_by,
        crate::ports::workspace::WorkspaceOrigin::Operator
    );

    // -- and now the ordering, with the node write refused ------------------
    //
    // A deliverable whose node the operator deleted still carries that node's
    // id on its latest version, so the reverse lookup matches and the append
    // runs — then the write fails, because the node is gone. That is the
    // failure this route's ordering was chosen for, and it is reachable
    // without a mock: the refusal comes from the real store.
    let mut orphaned = ArtifactRecord::new(
        "art-2",
        "t-1",
        "Retired spec",
        ArtifactKind::Markdown,
        "the agent's draft",
        "ceo",
        1,
    )
    .with_source("retired.md");
    orphaned.stamp_workspace_node("node-the-operator-deleted");
    ArtifactStore::upsert(runtime.artifacts().as_ref(), &company, &orphaned)
        .await
        .expect("seed");

    let (status, _) = send(
        &state,
        "PUT",
        "/api/v1/company/workspace/file/node-the-operator-deleted",
        Some(json!({"content": "an edit the tree cannot take"})),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "the node write must fail — there is no such node"
    );

    let (status, artifact) = send(&state, "GET", "/api/v1/company/artifacts/art-2", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        artifact["versions"].as_array().unwrap().len(),
        2,
        "the version must survive the refused node write: chain-ahead-of-node is \
         the direction that heals, and this is the ordering that guarantees it"
    );
    assert_eq!(
        artifact["versions"][1]["body"],
        "an edit the tree cannot take"
    );
}
