use crate::ports::types::CompanyId;
use crate::server::router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

use super::graphql_test_group_1::query;
use super::graphql_test_support_1::*;

/// `Company.workspaceSearch` (issue #607), over the same shared helper the REST
/// route and the agent tool use.
///
/// The hit shape is what this pins: a nested `FsNode`, the logical path a flat
/// hit list cannot derive from `parentId`, what matched, and the excerpt — plus
/// `total`, so a caller can tell a full answer from a first page.
#[tokio::test]
async fn workspace_search_resolves_hits_with_paths_and_totals() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_rich_company(&home).await;

    let id = CompanyId::new("acme");
    let workspace = state.registry().get(&id).unwrap().workspace().clone();
    let folder = crate::ports::workspace::WorkspaceNode {
        id: "f-std".to_string(),
        name: "standards".to_string(),
        kind: crate::ports::workspace::NodeKind::Folder,
        parent_id: None,
        updated_at_millis: 1_000,
        created_by: crate::ports::workspace::WorkspaceOrigin::Operator,
        updated_by: crate::ports::workspace::WorkspaceOrigin::Operator,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    };
    workspace.create(&id, &folder, None).await.unwrap();
    let note = crate::ports::workspace::WorkspaceNode {
        id: "n-support".to_string(),
        name: "Support.md".to_string(),
        kind: crate::ports::workspace::NodeKind::File,
        parent_id: Some("f-std".to_string()),
        ..folder.clone()
    };
    workspace
        .create(&id, &note, Some("Escalate a REFUND request to the CEO."))
        .await
        .unwrap();

    let app = router(state);
    let value = query(
        app.clone(),
        r#"{"query":"{ company(id:\"acme\"){ workspaceSearch(query:\"refund\"){ total hits { path matched excerpt node { id name kind } } } } }"}"#,
    )
    .await;
    let results = &value["data"]["company"]["workspaceSearch"];
    assert_eq!(results["total"], 1, "{value}");
    let hits = results["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["path"], "standards/Support.md");
    assert_eq!(hits[0]["matched"], "content");
    assert_eq!(hits[0]["node"]["id"], "n-support");
    assert_eq!(hits[0]["node"]["kind"], "file");
    assert!(
        hits[0]["excerpt"].as_str().unwrap().contains("REFUND"),
        "{value}"
    );

    // A name match carries no excerpt — null, not an empty string, so a client
    // cannot mistake "no body matched" for "the body matched nothing".
    let value = query(
        app,
        r#"{"query":"{ company(id:\"acme\"){ workspaceSearch(query:\"agents\"){ total hits { matched excerpt } } } }"}"#,
    )
    .await;
    let hits = value["data"]["company"]["workspaceSearch"]["hits"]
        .as_array()
        .unwrap();
    assert!(
        !hits.is_empty(),
        "the scaffolded `Agents` root matches: {value}"
    );
    assert_eq!(hits[0]["matched"], "name");
    assert!(hits[0]["excerpt"].is_null(), "{value}");
}

/// The committed SDL snapshot freezes the read contract. Regenerate with
/// `cargo test -- --ignored regenerate_sdl_snapshot` after any schema change.
#[test]
fn sdl_snapshot_matches() {
    let expected = include_str!("schema.graphql");
    let actual = super::sdl();
    assert_eq!(
        actual, expected,
        "GraphQL SDL drifted from schema.graphql; regenerate with \
         `cargo test -- --ignored regenerate_sdl_snapshot`"
    );
}

#[test]
#[ignore = "writes the SDL snapshot; run explicitly after a schema change"]
fn regenerate_sdl_snapshot() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/graphql/schema.graphql");
    std::fs::write(&path, super::sdl()).unwrap();
}

/// The headline of #669: `workspaceFile` reported a payload as an ordinary note
/// with no content, so a 4 MB PNG and a genuinely empty note were the same
/// response — on the surface whose entire job is to be unambiguous.
///
/// Asserted against the REST twin in the same test rather than in isolation.
/// The two are documented as differing only in timestamp shape, and the bug was
/// precisely that they disagreed about something much larger than that; a test
/// that pinned only the GraphQL half would not notice them drifting apart again
/// in the other direction.
#[tokio::test]
async fn graphql_and_rest_agree_that_a_binary_node_holds_no_text() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff];
    let id = given_a_binary_node(&state, "hero.png", "image/png", png).await;

    // GraphQL: an error naming the route that does serve the bytes, and no
    // `content: ""` masquerading as an empty note.
    let value = query(
        router(state.clone()),
        &format!(
            r#"{{"query":"{{ company(id:\"acme\"){{ workspaceFile(id:\"{id}\"){{ name content }} }} }}"}}"#
        ),
    )
    .await;
    let errors = value["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("a payload must not resolve as a note: {value}"));
    let message = errors[0]["message"].as_str().unwrap();
    assert!(
        message.contains("image/png") && message.contains("workspace/blob/"),
        "the refusal must name the type and the route that works: {message}"
    );
    assert!(
        value["data"]["company"]["workspaceFile"].is_null(),
        "no half-answer alongside the error: {value}"
    );

    // REST, the twin, for the same node: the same refusal.
    let response = router(state)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/company/workspace/file/{id}"))
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let rest = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        rest.contains("image/png") && rest.contains("workspace/blob/"),
        "REST must still refuse the same way: {rest}"
    );
}

/// The other half of #669, and the reason refusing above is not merely a harder
/// `null`: the tree now carries the three fields that let a consumer discover a
/// binary exists **before** it asks for text it cannot have.
///
/// Without these the refusal would be a dead end — a GraphQL client would have
/// no way to reach a payload at all, because nothing in the schema said payloads
/// were a thing. The REST `FsNode` has carried them since #553.
#[tokio::test]
async fn the_tree_projects_a_binary_nodes_mime_size_and_digest() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let png: &[u8] = &[0x89, b'P', b'N', b'G', 0xff];
    given_a_binary_node(&state, "hero.png", "image/png", png).await;

    let value = query(
        router(state),
        r#"{"query":"{ company(id:\"acme\"){ workspaceTree { name mime size sha256 } } }"}"#,
    )
    .await;
    assert!(value["errors"].is_null(), "{value}");
    let tree = value["data"]["company"]["workspaceTree"]
        .as_array()
        .unwrap();
    let image = tree
        .iter()
        .find(|node| node["name"] == serde_json::json!("hero.png"))
        .unwrap_or_else(|| panic!("the binary node is in the tree: {value}"));

    assert_eq!(image["mime"], "image/png");
    assert_eq!(
        image["size"].as_f64().unwrap(),
        png.len() as f64,
        "the size the store computed, not the None this test sent in"
    );
    let sha = image["sha256"]
        .as_str()
        .unwrap_or_else(|| panic!("a digest is projected: {image}"));
    assert_eq!(sha.len(), 64, "the store's sha256, hex-encoded");
}

/// A folder and a prose note both leave all three null. The console reads
/// `mime`'s **presence** as "render or download this instead of editing it", so
/// a projection that invented an empty string here would put every note behind
/// a download card.
///
/// Both are asserted because only one of them is a real test of the rule: a
/// folder can never carry a payload, so its nulls are structural, whereas a note
/// is a `File` exactly like the binary above and `mime` is the single field
/// telling them apart.
#[tokio::test]
async fn a_prose_note_projects_no_binary_metadata() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;
    let id = CompanyId::new("acme");
    let workspace = state.registry().get(&id).unwrap().workspace().clone();
    crate::company::workspace_scaffold::ensure_agent_folder(workspace.as_ref(), &id, "maya")
        .await
        .unwrap();

    // A folder is the easy half. The note is the half that matters: it is a
    // `File` like the payload above, so `mime`'s absence is the *only* thing
    // separating the two, and a projection that reached for a default here
    // would put every note in the company behind a download card.
    let note = crate::ports::workspace::WorkspaceNode {
        id: crate::ports::generate_id(),
        name: "Charter.md".to_string(),
        kind: crate::ports::workspace::NodeKind::File,
        parent_id: None,
        updated_at_millis: 1_700_000_000_000,
        created_by: crate::ports::workspace::WorkspaceOrigin::Operator,
        updated_by: crate::ports::workspace::WorkspaceOrigin::Operator,
        mime: None,
        size: None,
        sha256: None,
        adopted: false,
    };
    workspace
        .create(&id, &note, Some("# Charter\n\nprose, not bytes.\n"))
        .await
        .unwrap();

    let value = query(
        router(state),
        r#"{"query":"{ company(id:\"acme\"){ workspaceTree { name kind mime size sha256 } } }"}"#,
    )
    .await;
    assert!(value["errors"].is_null(), "{value}");
    let tree = value["data"]["company"]["workspaceTree"]
        .as_array()
        .unwrap_or_else(|| panic!("the tree resolves: {value}"));
    let find = |name: &str| {
        tree.iter()
            .find(|node| node["name"] == serde_json::json!(name))
            .unwrap_or_else(|| panic!("`{name}` is in the tree: {value}"))
            .clone()
    };

    let folder = find("maya");
    assert!(folder["mime"].is_null(), "{folder}");
    assert!(folder["size"].is_null(), "{folder}");
    assert!(folder["sha256"].is_null(), "{folder}");

    let note = find("Charter.md");
    assert_eq!(
        note["kind"], "file",
        "a note is a file, not a folder: {note}"
    );
    assert!(note["mime"].is_null(), "{note}");
    assert!(note["size"].is_null(), "{note}");
    assert!(note["sha256"].is_null(), "{note}");
}

/// The join, in one request: workflow run → attempts → steps → deep detail.
///
/// This is the query that had no answer before an `agent` node minted a row —
/// its turn has neither a card nor a conversation, so nothing could name it.
#[tokio::test]
async fn agent_runs_walks_a_workflow_run_to_its_reasoning() {
    let home = tempfile::tempdir().unwrap().keep();
    let state = state_with_company(&home).await;
    given_a_workflow_node_attempt(&state).await;

    let value = query(
        router(state),
        r#"{"query":"{ company(id:\"acme\") { agentRuns(workflowRunId:\"wr-1\") { id agentId nodeId workflowRunId status stepCount steps { seq kind label result failure deep { reasoning } } } } }"}"#,
    )
    .await;

    let runs = value["data"]["company"]["agentRuns"]
        .as_array()
        .unwrap_or_else(|| panic!("agentRuns missing: {value}"));
    assert_eq!(runs.len(), 1, "one node ran under wr-1: {value}");
    let run = &runs[0];
    assert_eq!(run["id"], "att-1");
    assert_eq!(run["agentId"], "programmer");
    assert_eq!(run["nodeId"], "solve");
    assert_eq!(run["workflowRunId"], "wr-1");

    // Live, so the settled count is deliberately null — a client must count the
    // steps rather than trust a total the settle has not written yet.
    assert!(
        run["stepCount"].is_null(),
        "a running attempt has no settled count"
    );

    let steps = run["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["kind"], "thinking");
    assert_eq!(steps[1]["label"], "Shell");
    assert_eq!(steps[1]["result"], "1 line");
    assert_eq!(steps[1]["failure"], "blocked_by_policy");

    // The deep half: reasoning the scrubbed step deliberately does not carry.
    assert_eq!(steps[0]["deep"]["reasoning"], "Collatz — memoise the chain");
    assert!(
        steps[1]["deep"].is_null(),
        "a step with no detail recorded has no deep half"
    );
}

/// The deep half is a store read, not a constant: a query that does not select
/// `steps.deep` must not drag the deep store into the request at all. The
/// console's Observatory list polls every 4/30 seconds and deliberately selects
/// no deep bodies, so an eager read would materialize up to `limit` runs ×
/// hundreds of detail rows per poll for data nothing renders — the lookahead
/// keeps that read off the hot path.
#[tokio::test]
async fn a_list_query_without_deep_does_not_read_the_deep_store() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::ports::deep_trace::{DeepTraceStore, RunStepDetailRecord};
    use crate::store::fs_ops::FsOps;

    #[derive(Clone)]
    struct CountingDeepTrace {
        inner: Arc<FsOps>,
        reads: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl DeepTraceStore for CountingDeepTrace {
        async fn append_step_detail(
            &self,
            company: &CompanyId,
            record: &RunStepDetailRecord,
        ) -> crate::error::Result<()> {
            self.inner.append_step_detail(company, record).await
        }

        async fn list_step_details(
            &self,
            company: &CompanyId,
            run_id: &str,
        ) -> crate::error::Result<Vec<RunStepDetailRecord>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.list_step_details(company, run_id).await
        }

        async fn list_step_details_for_runs(
            &self,
            company: &CompanyId,
            run_ids: &[String],
        ) -> crate::error::Result<std::collections::HashMap<String, Vec<RunStepDetailRecord>>>
        {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner
                .list_step_details_for_runs(company, run_ids)
                .await
        }

        async fn purge_deep_trace(
            &self,
            company: &CompanyId,
            run_id: Option<&str>,
        ) -> crate::error::Result<u64> {
            self.inner.purge_deep_trace(company, run_id).await
        }
    }

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let reads = Arc::new(AtomicUsize::new(0));
    let deep = Arc::new(CountingDeepTrace {
        inner: Arc::new(FsOps::new(home.clone())),
        reads: reads.clone(),
    });
    let state = state_with_builder(&home, manifest(), |b| b.with_deep_trace(deep)).await;
    given_a_workflow_node_attempt(&state).await;

    // The list read selects no deep bodies…
    let value = query(
        router(state.clone()),
        r#"{"query":"{ company(id:\"acme\") { agentRuns { id steps { seq kind label result } } } }"}"#,
    )
    .await;
    let runs = value["data"]["company"]["agentRuns"]
        .as_array()
        .unwrap_or_else(|| panic!("agentRuns missing: {value}"));
    assert_eq!(runs.len(), 1, "the attempt still lists: {value}");
    assert_eq!(runs[0]["steps"][0]["kind"], "thinking");
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "a deep-less list must not read the deep store"
    );

    // …and the single-run deep read still works when it is selected.
    let value = query(
        router(state),
        r#"{"query":"{ company(id:\"acme\") { agentRun(id:\"att-1\") { steps { seq deep { reasoning } } } } }"}"#,
    )
    .await;
    let steps = value["data"]["company"]["agentRun"]["steps"]
        .as_array()
        .unwrap_or_else(|| panic!("agentRun missing: {value}"));
    assert_eq!(steps[0]["deep"]["reasoning"], "Collatz — memoise the chain");
    assert!(
        reads.load(Ordering::SeqCst) >= 1,
        "selecting deep must read the store"
    );
}

/// The unredacted half is role-gated: a member sees the scrubbed trace and no
/// `deep`, exactly as approval contents are gated (issue #618). Without this,
/// any signed-in member could read raw tool arguments and output — which may
/// carry credentials and file contents — through the Observatory.
#[tokio::test]
async fn a_member_gets_the_trace_but_not_the_deep_half() {
    let home = tempfile::tempdir().unwrap().keep();
    let state = state_with_company(&home).await;
    given_a_workflow_node_attempt(&state).await;
    crate::server::test_support::seed_fixed_member(&state, "acme").await;

    let response = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/graphql")
                .header("content-type", "application/json")
                .header(
                    "cookie",
                    crate::server::test_support::member_cookie("acme"),
                )
                .body(Body::from(
                    r#"{"query":"{ company(id:\"acme\") { agentRuns { id steps { seq kind deep { reasoning } } } } }"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();

    let runs = value["data"]["company"]["agentRuns"]
        .as_array()
        .unwrap_or_else(|| panic!("agentRuns missing: {value}"));
    assert_eq!(runs.len(), 1, "a member still lists the attempt: {value}");
    let steps = runs[0]["steps"].as_array().unwrap();
    // The scrubbed skeleton is the member's answer…
    assert_eq!(steps[0]["kind"], "thinking");
    // …and the unredacted half is withheld.
    assert!(
        steps.iter().all(|s| s["deep"].is_null()),
        "a member must not receive deep bodies: {value}"
    );
}

/// An unrelated workflow run selects nothing rather than everything — the
/// failure mode of a filter that is silently dropped.
#[tokio::test]
async fn agent_runs_filters_by_workflow_run() {
    let home = tempfile::tempdir().unwrap().keep();
    let state = state_with_company(&home).await;
    given_a_workflow_node_attempt(&state).await;

    let value = query(
        router(state),
        r#"{"query":"{ company(id:\"acme\") { agentRuns(workflowRunId:\"other\") { id } } }"}"#,
    )
    .await;
    assert_eq!(
        value["data"]["company"]["agentRuns"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "{value}"
    );
}
