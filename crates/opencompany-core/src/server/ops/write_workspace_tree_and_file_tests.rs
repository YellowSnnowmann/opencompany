//! Integration tests for the `ops` write plane: tasks, memory, workspace,
//! skills, team, inbox-read, and desk chat — exercised end-to-end over the
//! router against a real fs-backed company.

use axum::http::StatusCode;
use serde_json::json;

use super::write_test_support::*;
use crate::ports::types::CompanyId;
use crate::runtime::RuntimeBuilder;
use crate::{AppConfig, AppState};

/// The read plane the console's Workspace tab runs on (issue #177): the tree
/// `GET` reflects writes, and the file `GET` carries content plus
/// server-computed backlinks.
///
/// Before this the only workspace read was GraphQL, which the console has no
/// client for — so the tab rendered a localStorage fixture and never saw a note
/// an agent (or another browser) wrote.
#[tokio::test]
async fn workspace_tree_and_file_reads_reflect_writes() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    // A workspace with nothing seeded into it reads as a real tree, not a 404
    // and not a fixture. It is not *empty*, though: boot scaffolds the reserved
    // `agents/` root and operator-only `secrets/README.md`. The manifest here has an agent and it gets
    // no folder — a member folder is minted on first use, not on joining the
    // roster. `desks/` is absent for the same reason since issue #645: nothing
    // writes into it, so it is minted on first use rather than scaffolded.
    let (status, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        provisioned_names(&tree),
        vec!["agents", "artifacts", "readme.md", "readme.md", "secrets"],
        "a fresh company starts with its system scaffold and nothing else"
    );
    let provisioned = tree.as_array().unwrap().len();

    let (_, folder) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({"name": "standards", "kind": "folder"})),
    )
    .await;
    let folder_id = folder["id"].as_str().unwrap().to_string();

    let (_, voice) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({
            "name": "voice.md",
            "kind": "file",
            "parentId": folder_id,
            "content": "# Voice\n\nWarm and concise.",
        })),
    )
    .await;
    let voice_id = voice["id"].as_str().unwrap().to_string();

    // A second note links to the first, so it must show up as its backlink.
    let (_, brief) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({
            "name": "brief.md",
            "kind": "file",
            "content": "Follows our [[voice]].",
        })),
    )
    .await;
    let brief_id = brief["id"].as_str().unwrap().to_string();

    // The tree carries every node's metadata — and deliberately no bodies, so a
    // navigation read never grows with the size of the workspace.
    let (status, tree) = send(&state, "GET", "/api/v1/company/workspace", None).await;
    assert_eq!(status, StatusCode::OK);
    let tree = tree.as_array().unwrap();
    assert_eq!(tree.len(), provisioned + 3);
    for node in tree {
        assert!(
            node.get("content").is_none(),
            "the tree read must not ship note bodies"
        );
        assert!(node["updatedAt"].is_number());
    }
    let listed = tree
        .iter()
        .find(|node| node["id"] == json!(voice_id))
        .expect("the created note is in the tree");
    assert_eq!(listed["name"], "voice.md");
    assert_eq!(listed["kind"], "file");
    assert_eq!(listed["parentId"], json!(folder_id));
    // Authorship rides every node of the tree read (issue #326). These routes
    // are the console's, so the console is the operator.
    assert_eq!(listed["createdBy"], json!({"kind": "operator"}));
    assert_eq!(listed["updatedBy"], json!({"kind": "operator"}));
    // …and the scaffold's own nodes say what they are, so the console can tell
    // "the runtime laid this down" from "somebody wrote this".
    let root = tree
        .iter()
        .find(|node| node["name"] == json!("agents"))
        .expect("the Agents root is in the tree");
    assert_eq!(root["createdBy"], json!({"kind": "seed"}));
    assert_eq!(root["kind"], json!("folder"));
    assert!(root["parentId"].is_null());

    // The file read carries the body and the inbound backlink, computed server
    // side — the console derives neither.
    let (status, file) = send(
        &state,
        "GET",
        &format!("/api/v1/company/workspace/file/{voice_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(file["name"], "voice.md");
    assert!(
        file["content"]
            .as_str()
            .unwrap()
            .contains("Warm and concise")
    );
    assert!(file["updatedAt"].is_number());
    assert_eq!(file["createdBy"], json!({"kind": "operator"}));
    assert_eq!(file["updatedBy"], json!({"kind": "operator"}));
    let backlinks = file["backlinks"].as_array().unwrap();
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[0]["id"], json!(brief_id));
    assert_eq!(backlinks[0]["name"], "brief.md");

    // An out-of-band write (an agent, or another browser) is visible on the very
    // next read — the whole point of the tab reading the store.
    let (status, _) = send(
        &state,
        "PUT",
        &format!("/api/v1/company/workspace/file/{voice_id}"),
        Some(json!({"content": "# Voice\n\nRewritten elsewhere."})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, file) = send(
        &state,
        "GET",
        &format!("/api/v1/company/workspace/file/{voice_id}"),
        None,
    )
    .await;
    assert!(
        file["content"]
            .as_str()
            .unwrap()
            .contains("Rewritten elsewhere")
    );

    // A folder id and an unknown id are both 404 — never an empty note.
    let (status, body) = send(
        &state,
        "GET",
        &format!("/api/v1/company/workspace/file/{folder_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "company_not_found");

    let (status, _) = send(
        &state,
        "GET",
        "/api/v1/company/workspace/file/does-not-exist",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Two-company isolation over the workspace read plane: company A's notes are
/// invisible to B, and a tenant token may not address a company it does not own.
/// The store is per-company by construction — this pins that the new `GET`s do
/// not widen it.
#[tokio::test]
async fn workspace_reads_are_isolated_between_companies() {
    use crate::server::platform_auth::{
        PlatformAuthConfig, PlatformClaims, UnsignedTenantVerifier,
    };
    use std::collections::HashSet;

    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let verifier = std::sync::Arc::new(UnsignedTenantVerifier::new("plat-secret"));
    let state = AppState::new(AppConfig::default())
        .with_home(home.clone())
        .with_platform_auth(PlatformAuthConfig::new(verifier));

    for name in ["a", "b"] {
        let id = CompanyId::new(name);
        let runtime = RuntimeBuilder::new(home.clone(), manifest())
            .with_id(id.clone())
            .build()
            .await
            .unwrap();
        state
            .registry()
            .insert(id.clone(), std::sync::Arc::new(runtime));
        state.set_owner(id.clone(), format!("tenant:{name}"));
    }

    let token = |tenant: &str| {
        UnsignedTenantVerifier::tenant_token(&PlatformClaims {
            tenant: tenant.to_string(),
            scopes: HashSet::from(["operator".to_string()]),
            companies: None,
        })
    };

    let (status, note) = send_auth(
        &state,
        "POST",
        "/api/v1/companies/a/workspace",
        Some(json!({"name": "secret.md", "kind": "file", "content": "A body"})),
        Some(&token("tenant:a")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let note_id = note["id"].as_str().unwrap().to_string();

    // B's own workspace holds only its own scaffolded system root — A's note
    // is not in it.
    let (status, tree_b) = send_auth(
        &state,
        "GET",
        "/api/v1/companies/b/workspace",
        None,
        Some(&token("tenant:b")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        provisioned_names(&tree_b),
        vec!["agents", "artifacts", "readme.md", "readme.md", "secrets"]
    );

    // Even naming A's node id explicitly, B's scope does not resolve it.
    let (status, _) = send_auth(
        &state,
        "GET",
        &format!("/api/v1/companies/b/workspace/file/{note_id}"),
        None,
        Some(&token("tenant:b")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // And A's token may not address B's workspace at all — 403 (scoped auth).
    let (status, _) = send_auth(
        &state,
        "GET",
        "/api/v1/companies/b/workspace",
        None,
        Some(&token("tenant:a")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// `GET …/workspace/search` (issue #607): the hit body, both scope forms, and
/// the two refusals stated rather than guessed.
#[tokio::test]
async fn workspace_search_returns_hits_with_paths_and_excerpts() {
    let home_dir = home();
    let home = home_dir.path().to_path_buf();
    let state = state_with_company(&home).await;

    let (_, folder) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({"name": "standards", "kind": "folder"})),
    )
    .await;
    let folder_id = folder["id"].as_str().unwrap().to_string();
    let (_, note) = send(
        &state,
        "POST",
        "/api/v1/company/workspace",
        Some(json!({
            "name": "Support.md",
            "kind": "file",
            "parentId": folder_id,
            "content": "# Support\n\nEscalate a REFUND request to the CEO."
        })),
    )
    .await;
    let note_id = note["id"].as_str().unwrap().to_string();

    // A content hit carries the path the tree view would have to derive, the
    // excerpt, the origins the console badges, and what matched.
    let (status, results) = send(
        &state,
        "GET",
        "/api/v1/company/workspace/search?q=refund",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(results["total"], json!(1));
    let hits = results["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["id"], json!(note_id));
    assert_eq!(hits[0]["path"], "standards/support.md");
    assert_eq!(hits[0]["matched"], "content");
    assert_eq!(hits[0]["kind"], "file");
    assert_eq!(hits[0]["updatedBy"], json!({"kind": "operator"}));
    assert!(
        hits[0]["excerpt"].as_str().unwrap().contains("REFUND"),
        "{:?}",
        hits[0]["excerpt"]
    );

    // A folder is a hit in its own right, matched by name and with no excerpt
    // promising a body it does not have.
    let (status, results) = send(
        &state,
        "GET",
        "/api/v1/company/workspace/search?q=standards",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let hits = results["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["id"], json!(folder_id));
    assert_eq!(hits[0]["kind"], "folder");
    assert_eq!(hits[0]["matched"], "name");
    assert!(hits[0].get("excerpt").is_none(), "{:?}", hits[0]);

    // `prefix` scopes to a subtree.
    let (status, scoped) = send(
        &state,
        "GET",
        "/api/v1/company/workspace/search?q=support&prefix=standards",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(scoped["total"], json!(1));
    let (_, elsewhere) = send(
        &state,
        "GET",
        "/api/v1/company/workspace/search?q=support&prefix=Desks",
        None,
    )
    .await;
    assert_eq!(elsewhere["total"], json!(0));

    // Both refusals are 400 and say what is wrong. An empty `q` is NOT "match
    // everything" — a cleared search box must not fetch the whole tree.
    for uri in [
        "/api/v1/company/workspace/search",
        "/api/v1/company/workspace/search?q=",
        "/api/v1/company/workspace/search?q=%20%20",
        "/api/v1/company/workspace/search?q=refund&limit=0",
    ] {
        let (status, body) = send(&state, "GET", uri, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} → {body}");
        assert_eq!(body["code"], "invalid_request", "{uri} → {body}");
    }

    // The route resolves under the platform scope form too, and `search` is
    // never captured as a node id by the `…/workspace/{node_id}` route.
    let (status, results) = send(
        &state,
        "GET",
        "/api/v1/companies/acme/workspace/search?q=refund",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(results["total"], json!(1));
}
