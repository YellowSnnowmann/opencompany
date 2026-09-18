//! End-to-end tests through a real [`CompanyRuntime`](crate::company::runtime::CompanyRuntime)
//! with [`HostedMedullaBrain`] wired in through the builder. See
//! `hosted_offline_tests.rs` for the offline half over [`MockTransport`].

use super::tests_offline::{effect_frame, tool_call_frame, usage_frame};
use super::*;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::brain::medulla::MockTransport;
use crate::brain::medulla::wire::{self};
use crate::ports::types::CompanyEvent;

// ---------------------------------------------------------------------------
// End-to-end tests through a real CompanyRuntime
// ---------------------------------------------------------------------------

use crate::app::config::BrainMode;
use crate::company::CompanyManifest;
use crate::runtime::RuntimeBuilder;

fn tmp_home() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("opencompany-hosted-")
        .tempdir()
        .expect("tempdir")
}

fn manifest(policy_mode: &str) -> CompanyManifest {
    let toml_src = format!(
        r#"
        [company]
        name = "Acme"

        [brain]
        mode = "hosted"

        [tools]
        allow = ["noop"]

        [policy]
        mode = "{policy_mode}"
        "#
    );
    toml::from_str(&toml_src).expect("valid manifest")
}

/// How many events a fresh company already has in its journal by the time it
/// finishes booting (issue #327).
///
/// Boot lays down the reserved workspace roots, and since #327 the workspace
/// store announces its own writes — one `WorkspaceChanged` per root, plus one
/// per explanatory note the scaffold provisions (`secrets/readme.md` and
/// `artifacts/readme.md`), is journalled before any operator message.
const BOOT_JOURNAL_EVENTS: u64 = crate::company::workspace_scaffold::SYSTEM_ROOTS.len() as u64 + 2;

/// The deterministic first-cycle id a real runtime for `Acme` produces: the
/// company id slugs to `acme`, and the first *cycle* event lands at the first
/// sequence boot did not already use.
fn runtime_cid() -> String {
    wire::cycle_id("opencompany:acme", "acme", BOOT_JOURNAL_EVENTS)
}

#[tokio::test]
async fn e2e_operator_message_drives_tool_call_and_gated_send_dm() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let transport = Arc::new(MockTransport::new());
    transport.script_cycle(
        runtime_cid(),
        vec![
            tool_call_frame("noop", 0, json!({ "q": "status" })),
            effect_frame("send_dm", 0, json!({ "to": "operator", "body": "on it" })),
        ],
    );

    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain_mode(BrainMode::Hosted)
        .with_credential(SecretValue("th_live".into()))
        .with_transport(transport.clone())
        .build()
        .await
        .unwrap();

    let report = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "how are we doing".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await
        .unwrap();

    // The gated send_dm produced a channel response routed to the operator.
    assert_eq!(report.responses.len(), 1);
    assert_eq!(report.responses[0].channel, "operator");
    assert_eq!(report.responses[0].text, "on it");

    // The effect flowed through the gate and acked ok:true.
    let acks = transport.acks();
    assert_eq!(acks.len(), 1);
    assert!(acks[0].ok);

    // The device tool was serviced and answered.
    assert_eq!(transport.tool_answers().len(), 1);

    // Exactly one event was posted for the operator message.
    assert_eq!(transport.posted_events().len(), 1);

    // A compressed trace was persisted to the fs-backed MemoryStore.
    let traces = rt.memory.recent_traces(rt.id(), 10).await.unwrap();
    assert!(!traces.is_empty());
}

#[tokio::test]
async fn e2e_supervised_effect_runs_without_policy_hitl() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let transport = Arc::new(MockTransport::new());
    transport.script_cycle(
        runtime_cid(),
        // Policy HITL is disabled even for a formerly-gated Sign effect.
        vec![effect_frame("filing.submit", 0, Value::Null)],
    );

    let rt = Arc::new(
        RuntimeBuilder::new(home.clone(), manifest("supervised"))
            .with_brain_mode(BrainMode::Hosted)
            .with_credential(SecretValue("th_live".into()))
            .with_transport(transport.clone())
            .build()
            .await
            .unwrap(),
    );

    let report = rt
        .run_cycle(vec![CompanyEvent::OperatorMessage {
            mentions: Vec::new(),
            parent: None,
            text: "file it".into(),
            by: None,
            chat: None,
            deliverable: None,
            attachments: Vec::new(),
        }])
        .await
        .unwrap();

    assert!(report.parked.is_empty());
    assert!(rt.pending_approvals().is_empty());
    assert!(report.responses.is_empty());

    // Medulla is told the effect completed instead of waiting on policy HITL.
    let acks = transport.acks();
    assert_eq!(acks.len(), 1);
    assert!(acks[0].ok);
    assert!(acks[0].error.is_none());
}

/// Issue #174 end to end: a real runtime on the hosted brain records the tokens
/// and cost the wire reported, so the console's Usage view stops reading zero.
#[tokio::test]
async fn e2e_reported_usage_lands_on_the_usage_meter() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let transport = Arc::new(MockTransport::new());
    // `cid()` and `runtime_cid()` are the same deterministic id: company `acme`,
    // first event at seq 0.
    transport.script_cycle(
        runtime_cid(),
        vec![
            usage_frame(0, 1_500, 260, Some(0.042)),
            effect_frame("send_dm", 0, json!({ "body": "on it" })),
        ],
    );

    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain_mode(BrainMode::Hosted)
        .with_credential(SecretValue("th_live".into()))
        .with_transport(transport.clone())
        .build()
        .await
        .unwrap();

    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "how are we doing".into(),
        by: None,
        chat: None,
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    let samples = rt.usage().query(rt.id(), 0).await.unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].input_tokens, 1_500);
    assert_eq!(samples[0].output_tokens, 260);
    assert_eq!(samples[0].cost_usd, 0.042);
    assert_eq!(samples[0].provider, crate::metering::MEDULLA_PROVIDER);
    assert_eq!(
        samples[0].kind,
        crate::ports::usage::SampleKind::Inference,
        "hosted cycles meter as inference, not as an OAuth call"
    );

    // The spend also reaches Finances as an `inference.spend` ledger entry.
    let record = rt.store().load(rt.id()).await.unwrap().unwrap();
    assert!(
        record
            .ledger
            .iter()
            .any(|e| e.kind == crate::metering::INFERENCE_SPEND_KIND && e.amount_usd == -0.042)
    );
}

// ---------------------------------------------------------------------------
// Issue #176: hosted-path delegation (durable async hand-off, no local
// cognition) + handed-task awareness.
// ---------------------------------------------------------------------------

/// A manifest with an Engineering desk (`eng`) whose lead is `eng1`, plus a
/// hosted brain. Used to prove hosted `delegate_to_desk` resolves the desk and
/// records the hand-off against it.
fn desk_manifest() -> CompanyManifest {
    let toml_src = r#"
        [company]
        name = "Acme"

        [brain]
        mode = "hosted"

        [tools]
        allow = ["noop"]

        [policy]
        mode = "full"

        [[agent]]
        id = "chief"
        role = "Chief"
        tier = "orchestrator"

        [[agent]]
        id = "eng1"
        role = "Engineer"

        [[group_chat]]
        id = "eng"
        name = "Engineering"
        members = ["eng1"]
        "#;
    toml::from_str(toml_src).expect("valid manifest")
}

/// The hosted catalog registered with Medulla must advertise the delegation
/// tools on top of the manifest's own `tools.allow`, so a hosted company's
/// orchestrator can actually delegate.
#[tokio::test]
async fn e2e_hosted_catalog_advertises_delegation_tools() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let transport = Arc::new(MockTransport::new());
    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain_mode(BrainMode::Hosted)
        .with_credential(SecretValue("th_live".into()))
        .with_transport(transport.clone())
        .build()
        .await
        .unwrap();

    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        // Issue #1725: not "hi". A bare pleasantry is answered by the runtime
        // without reaching a brain, so no catalog would be registered at all.
        text: "ship the landing page".into(),
        by: None,
        chat: None,
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    let registered = transport.registered_tools();
    assert_eq!(registered.len(), 1, "tools register exactly once");
    let names: Vec<&str> = registered[0].iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"noop"), "manifest tool kept: {names:?}");
    assert!(
        names.contains(&"spawn_task"),
        "spawn_task advertised: {names:?}"
    );
    assert!(
        names.contains(&"delegate_to_desk"),
        "delegate_to_desk advertised: {names:?}"
    );
}

/// Medulla emitting a `spawn_task` tool-call on the hosted path opens a durable
/// board card device-side and answers ok — no local cognition needed.
#[tokio::test]
async fn e2e_spawn_task_tool_call_opens_a_board_card() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let transport = Arc::new(MockTransport::new());
    transport.script_cycle(
        runtime_cid(),
        vec![tool_call_frame(
            "spawn_task",
            0,
            json!({ "title": "Ship the invoice flow", "assignee": "eng" }),
        )],
    );

    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain_mode(BrainMode::Hosted)
        .with_credential(SecretValue("th_live".into()))
        .with_transport(transport.clone())
        .build()
        .await
        .unwrap();

    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "open a task to ship invoicing".into(),
        by: None,
        chat: None,
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    // The tool was answered ok.
    let answers = transport.tool_answers();
    assert_eq!(answers.len(), 1);
    assert!(answers[0].ok, "spawn_task answered ok: {:?}", answers[0]);

    // A durable card landed on the board.
    let cards = rt.tasks().list(rt.id()).await.unwrap();
    assert_eq!(cards.len(), 1, "one card opened: {cards:?}");
    assert_eq!(cards[0].title, "Ship the invoice flow");
    assert_eq!(cards[0].assignee, "eng");
    assert_eq!(cards[0].column, "todo");
}

/// Medulla emitting a `delegate_to_desk` tool-call resolves the desk and records
/// a durable hand-off card assigned to that desk (so it surfaces when the desk
/// is asked directly). An unknown desk is a clean tool error, not a lost card.
#[tokio::test]
async fn e2e_delegate_to_desk_tool_call_writes_a_handoff_card() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let transport = Arc::new(MockTransport::new());
    transport.script_cycle(
        runtime_cid(),
        vec![
            tool_call_frame(
                "delegate_to_desk",
                0,
                json!({ "desk": "Engineering", "instruction": "build the invoice importer" }),
            ),
            tool_call_frame(
                "delegate_to_desk",
                1,
                json!({ "desk": "Nonexistent", "instruction": "do a thing" }),
            ),
        ],
    );

    let rt = RuntimeBuilder::new(home.clone(), desk_manifest())
        .with_brain_mode(BrainMode::Hosted)
        .with_credential(SecretValue("th_live".into()))
        .with_transport(transport.clone())
        .build()
        .await
        .unwrap();

    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "have engineering build invoicing".into(),
        by: None,
        chat: None,
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    let answers = transport.tool_answers();
    assert_eq!(answers.len(), 2);
    // First hand-off resolved the desk by name and succeeded.
    assert!(answers[0].ok, "known desk hands off ok: {:?}", answers[0]);
    // Unknown desk answered ok:false (clean error) and wrote no card.
    assert!(
        !answers[1].ok,
        "unknown desk is a clean error: {:?}",
        answers[1]
    );

    let cards = rt.tasks().list(rt.id()).await.unwrap();
    assert_eq!(cards.len(), 1, "only the known desk got a card: {cards:?}");
    // Assigned to the resolved desk id, with the lead recorded in the note.
    assert_eq!(cards[0].assignee, "eng");
    let note = cards[0].note.as_deref().unwrap_or_default();
    assert!(note.contains("eng1"), "note records the lead: {note}");
    assert!(
        note.contains("build the invoice importer"),
        "note carries the instruction"
    );
}

/// The same company with no usage frame on the wire: an honest zero, and no
/// fabricated sample.
#[tokio::test]
async fn e2e_a_cycle_without_usage_frames_meters_nothing() {
    let home_dir = tmp_home();
    let home = home_dir.path().to_path_buf();
    let transport = Arc::new(MockTransport::new());
    transport.script_cycle(
        runtime_cid(),
        vec![effect_frame("send_dm", 0, json!({ "body": "on it" }))],
    );

    let rt = RuntimeBuilder::new(home.clone(), manifest("full"))
        .with_brain_mode(BrainMode::Hosted)
        .with_credential(SecretValue("th_live".into()))
        .with_transport(transport.clone())
        .build()
        .await
        .unwrap();

    rt.run_cycle(vec![CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: "hello".into(),
        by: None,
        chat: None,
        deliverable: None,
        attachments: Vec::new(),
    }])
    .await
    .unwrap();

    assert!(rt.usage().query(rt.id(), 0).await.unwrap().is_empty());
}
