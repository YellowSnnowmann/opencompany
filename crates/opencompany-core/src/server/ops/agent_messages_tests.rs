//! Tests for `GET {scope}/agents/{agent_id}/messages`: the fold that picks a
//! teammate's direct lines out of the journal, and the route over it.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::*;
use crate::company::CompanyManifest;
use crate::ports::types::CompanyId;
use crate::runtime::RuntimeBuilder;
use crate::server::router;
use crate::{AppConfig, AppState};

fn line(seq: u64, sender: &str, destination: HiveDestination, text: &str) -> StoredEvent {
    StoredEvent {
        seq: EventSeq::new(seq),
        company: CompanyId::new("acme"),
        event: CompanyEvent::HiveMessage {
            sequence: seq * 10,
            sender: sender.to_string(),
            destination,
            text: text.to_string(),
            thread: None,
            episode_id: None,
            only_for: Vec::new(),
        },
        at_millis: seq,
    }
}

fn to(agent: &str) -> HiveDestination {
    HiveDestination::Agent(agent.to_string())
}

#[test]
fn the_fold_keeps_what_the_agent_sent_or_was_sent_directly() {
    let rows = vec![
        line(1, "writer", to("engineer"), "draft?"),
        line(2, "engineer", to("writer"), "here"),
        line(3, "ceo", to("engineer"), "not the writer's"),
        line(4, "writer", HiveDestination::Hive("content".into()), "a hive line"),
    ];
    let got = direct_messages(&rows, "writer", MAX_MESSAGES);
    let texts: Vec<&str> = got.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, ["draft?", "here"]);
    assert_eq!(got[1].recipient, "writer");
    assert_eq!(got[1].seq, 2);
    assert_eq!(got[1].sequence, 20);
}

#[test]
fn the_fold_stops_at_its_limit() {
    let rows: Vec<StoredEvent> = (1..=5)
        .map(|seq| line(seq, "writer", to("ceo"), "hi"))
        .collect();
    assert_eq!(direct_messages(&rows, "writer", 2).len(), 2);
}

async fn state(home: &std::path::Path) -> (AppState, Arc<crate::CompanyRuntime>) {
    let manifest: CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [[agent]]\nid = \"writer\"\nrole = \"Writer\"\n\
         [[agent]]\nid = \"engineer\"\nrole = \"Engineer\"\n",
    )
    .unwrap();
    let id = CompanyId::new("acme");
    let runtime = Arc::new(
        RuntimeBuilder::new(home.to_path_buf(), manifest)
            .with_id(id.clone())
            .build()
            .await
            .unwrap(),
    );
    let state = AppState::new(AppConfig::default()).with_home(home.to_path_buf());
    state.registry().insert(id, Arc::clone(&runtime));
    crate::server::test_support::seed_fixed_admin(&state, "acme").await;
    (state, runtime)
}

async fn get(state: &AppState, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = router(state.clone())
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", crate::server::test_support::fixed_cookie("acme"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

#[tokio::test]
async fn the_route_pages_an_agents_direct_lines_after_a_cursor() {
    let home = tempfile::tempdir().unwrap();
    let (state, runtime) = state(home.path()).await;
    let company = CompanyId::new("acme");
    for (sender, recipient, text) in [
        ("writer", "engineer", "first"),
        ("engineer", "writer", "second"),
    ] {
        runtime
            .events()
            .append(
                &company,
                CompanyEvent::HiveMessage {
                    sequence: 0,
                    sender: sender.to_string(),
                    destination: to(recipient),
                    text: text.to_string(),
                    thread: None,
                    episode_id: None,
                    only_for: Vec::new(),
                },
            )
            .await
            .unwrap();
    }

    let (status, body) = get(&state, "/api/v1/company/agents/writer/messages").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body.as_array().expect("an array");
    assert_eq!(rows.len(), 2, "{body}");
    assert_eq!(rows[0]["text"], "first");
    assert_eq!(rows[0]["recipient"], "engineer");

    let cursor = rows[0]["seq"].as_u64().unwrap();
    let (_, after) = get(
        &state,
        &format!("/api/v1/company/agents/writer/messages?after={cursor}"),
    )
    .await;
    let texts: Vec<&str> = after
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["text"].as_str())
        .collect();
    assert_eq!(texts, ["second"], "the cursor is exclusive");
}

#[tokio::test]
async fn an_unknown_agent_is_404_not_an_empty_conversation() {
    let home = tempfile::tempdir().unwrap();
    let (state, _runtime) = state(home.path()).await;
    let (status, _) = get(&state, "/api/v1/company/agents/nobody/messages").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
