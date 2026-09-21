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
use crate::ports::types::{Actor, ActorKind, ToolGrantsOverride};

fn granting(rec: &CompanyRecord, namespace: &str) -> CompanyRecord {
    let mut granted = rec.clone();
    granted.overlay_tool_grants = Some(ToolGrantsOverride {
        added: vec![namespace.to_string()],
        set_by: Actor {
            kind: ActorKind::User,
            id: "user-admin".to_string(),
        },
        at_millis: crate::ports::now_millis(),
    });
    granted.manifest.tools.allow = granted.effective_tool_allow();
    granted
}

#[tokio::test]
async fn a_rebuild_that_moves_the_catalogue_owes_the_session_a_brief() {
    let dir = tempfile::tempdir().unwrap();
    let context = Arc::new(MockContext::default());
    let mut rec = capped_record();
    rec.manifest.tools.allow = vec!["*".to_string()];
    let live_store = Arc::new(LiveStore::default());
    live_store.save(&rec).await.unwrap();
    let mut deps = deps_with_plan(dir.path(), context, None, None);
    deps.store = live_store.clone();

    let pool = HarnessPool::new();
    pool.ensure(&rec, &deps).await.expect("first ensure");
    let first = pool.agent(&rec.id, "engineer").await.expect("engineer");
    assert!(
        !first.catalogue_brief_pending(),
        "a first roster owes nothing: its session opens cold on its own prompt"
    );
    assert!(
        !first.served_catalogue().iter().any(|t| t == "workspace_list"),
        "precondition: `*` confers no workspace namespace"
    );

    // A redundant ensure keeps the entry and owes nothing new.
    pool.ensure(&rec, &deps).await.expect("redundant ensure");
    let same = pool.agent(&rec.id, "engineer").await.expect("engineer");
    assert!(Arc::ptr_eq(&first, &same), "an unchanged roster is not rebuilt");

    // A console grant rebuilds the roster with more on the belt.
    live_store.save(&granting(&rec, "workspace")).await.unwrap();
    pool.ensure(&rec, &deps).await.expect("post-grant ensure");
    let granted = pool.agent(&rec.id, "engineer").await.expect("engineer");
    assert!(!Arc::ptr_eq(&first, &granted), "the grant must rebuild");
    assert!(
        granted.served_catalogue().iter().any(|t| t == "workspace_list"),
        "precondition: the grant wired the workspace tools: {:?}",
        granted.served_catalogue()
    );
    assert!(
        granted.catalogue_brief_pending(),
        "the rebuilt entry must know the resumed session's prompt predates its catalogue"
    );

    // Rebuilt again on an unrelated axis before any turn said the brief: the
    // debt carries, because the session is still on the first prompt.
    let mut renamed = granting(&rec, "workspace");
    renamed.manifest.company.name = "Acme Renamed".to_string();
    live_store.save(&renamed).await.unwrap();
    pool.ensure(&renamed, &deps).await.expect("post-rename ensure");
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
