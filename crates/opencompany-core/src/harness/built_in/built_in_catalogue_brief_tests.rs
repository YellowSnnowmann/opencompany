//! A roster rebuilt under a resumed session re-announces its tool catalogue
//! (`CompanyAgent::catalogue_brief_stale`).
//!
//! The embedded runtime pins a session's system prompt at its first committed
//! turn, and every conversational turn resumes the agent's one stable session
//! — so a rebuild that changes the served catalogue reaches the MCP host but
//! not the prompt the model reads the catalogue off. The rebuilt entry has to
//! know it owes the session the current brief, and has to keep knowing it
//! across further rebuilds until a turn actually says it.

use super::built_in_test_fixtures::*;
use super::built_in_test_fixtures_2::*;
use super::*;
use crate::harness::build::{opencompany_mcp_rebrief, tools_named_in_mcp_brief};

/// `rec` with `namespace` on the company allow-list — a seed edit, which the
/// grant axis reads off the record `ensure` is handed.
fn granting(rec: &CompanyRecord, namespace: &str) -> CompanyRecord {
    let mut granted = rec.clone();
    granted.manifest.tools.allow.push(namespace.to_string());
    granted
}

#[tokio::test]
async fn a_rebuild_that_moves_the_catalogue_owes_the_session_a_brief() {
    let dir = tempfile::tempdir().unwrap();
    let context = Arc::new(MockContext::default());
    let mut rec = capped_record();
    rec.manifest.tools.allow = vec!["*".to_string()];
    let mut deps = deps_with_plan(dir.path(), context, None, None);
    // The workspace tools are what the `workspace` grant below wires; with no
    // store they fail closed and the grant would move nothing.
    deps.workspace = Some(Arc::new(crate::store::FsOps::new(dir.path().to_path_buf())));

    let pool = HarnessPool::new();
    pool.ensure(&rec, &deps).await.expect("first ensure");
    let first = pool.agent(&rec.id, "engineer").await.expect("engineer");
    assert!(
        !first.catalogue_brief_pending(),
        "a first roster owes nothing: its session opens cold on its own prompt"
    );
    assert!(
        !first
            .served_catalogue()
            .iter()
            .any(|t| t == "workspace_create"),
        "precondition: `*` confers no explicit workspace write"
    );

    // A redundant ensure keeps the entry and owes nothing new.
    pool.ensure(&rec, &deps).await.expect("redundant ensure");
    let same = pool.agent(&rec.id, "engineer").await.expect("engineer");
    assert!(
        Arc::ptr_eq(&first, &same),
        "an unchanged roster is not rebuilt"
    );

    // A console grant rebuilds the roster with more on the belt.
    let with_workspace = granting(&rec, "workspace.write");
    pool.ensure(&with_workspace, &deps)
        .await
        .expect("post-grant ensure");
    let granted = pool.agent(&rec.id, "engineer").await.expect("engineer");
    assert!(!Arc::ptr_eq(&first, &granted), "the grant must rebuild");
    assert!(
        granted
            .served_catalogue()
            .iter()
            .any(|t| t == "workspace_create"),
        "precondition: the grant wired the workspace write tools: {:?}",
        granted.served_catalogue()
    );
    assert!(
        granted.catalogue_brief_pending(),
        "the rebuilt entry must know the resumed session's prompt predates its catalogue"
    );

    // Rebuilt again on an unrelated axis before any turn said the brief: the
    // debt carries, because the session is still on the first prompt.
    let mut renamed = with_workspace.clone();
    renamed.manifest.company.name = "Acme Renamed".to_string();
    pool.ensure(&renamed, &deps)
        .await
        .expect("post-rename ensure");
    let carried = pool.agent(&rec.id, "engineer").await.expect("engineer");
    assert!(!Arc::ptr_eq(&granted, &carried), "the rename must rebuild");
    assert_eq!(carried.served_catalogue(), granted.served_catalogue());
    assert!(
        carried.catalogue_brief_pending(),
        "a pending brief survives a rebuild that keeps the catalogue"
    );
}

#[test]
fn the_rebrief_is_read_back_like_the_prompt_brief_and_keeps_the_turn_text() {
    let tools = vec!["post".to_string(), "composio_execute".to_string()];
    let text = opencompany_mcp_rebrief(&tools, "[conversation: engineering]\nsend it");
    assert_eq!(tools_named_in_mcp_brief(&text), tools);
    assert!(
        text.ends_with("[conversation: engineering]\nsend it"),
        "the turn text follows the brief untouched: {text}"
    );
}
