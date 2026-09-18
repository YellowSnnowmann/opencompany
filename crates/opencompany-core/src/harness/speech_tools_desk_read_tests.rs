use super::speech_tools_test_fixtures::*;
use super::*;
// `desk_read` — tinysweeper: the only speech tool with no coverage of
// its scan, its channel/audience filtering, or its truncation footer.
// -------------------------------------------------------------------

/// [`context_with_desks`], but wired to a [`HistoryLog`] instead of a
/// [`RecordingLog`] — `desk_read` is the one tool that actually reads
/// history back, and `RecordingLog::read_from` hardcodes an empty page.
async fn context_with_desks_history() -> (SpeechContext, Arc<HistoryLog>, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("speech-tools-")
        .tempdir()
        .expect("tempdir");
    let store: Arc<dyn crate::ports::store::CompanyStore> =
        Arc::new(crate::store::FsCompanyStore::new(dir.path()));
    let events = Arc::new(HistoryLog::default());
    let company = CompanyId::new("acme");
    let context = SpeechContext::new(
        company.clone(),
        "designer".to_string(),
        events.clone() as Arc<dyn EventLog>,
        store.clone(),
    );
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
[company]
name = "Acme"

[policy]
mode = "full"

[[agent]]
id = "designer"
role = "Designer"
description = "Draws things."

[[agent]]
id = "engineer"
role = "Engineer"
description = "Builds things."

[[group_chat]]
id = "brand"
name = "Brand"
members = ["designer"]

[[group_chat]]
id = "platform"
name = "Platform"
members = ["engineer"]
"#,
    )
    .expect("valid manifest");
    let record = crate::ports::types::CompanyRecord {
        id: company,
        manifest,
        ledger: Vec::new(),
        lifecycle: "running".to_string(),
        setup: None,
        name_confirmed: false,
        activation_completed_at: None,
        created_at_millis: None,
        overlay_agents: Vec::new(),
        overlay_desk_members: Vec::new(),
        overlay_desk_order: Vec::new(),
        overlay_desks: Vec::new(),
        overlay_desk_hive: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        overlay_tool_grants: None,
        overlay_workflows: Vec::new(),
        overlay_budgets: Vec::new(),
        overlay_policy: None,
        overlay_desk_tools: Default::default(),
        disabled_workflows: Vec::new(),
        template_provenance: None,
    };
    store.save(&record).await.expect("the record saves");
    (context, events, dir)
}

fn brand_reply(agent_id: &str, text: &str, audience: Vec<String>) -> CompanyEvent {
    CompanyEvent::AgentReply {
        chat_id: "brand".to_string(),
        agent_id: agent_id.to_string(),
        text: text.to_string(),
        steps: Vec::new(),
        task_id: None,
        outputs: Vec::new(),
        parent: None,
        mentions: Vec::new(),
        mention_depth: 0,
        audience,
    }
}

/// The scan returns the channel's own rows, oldest first, and leaves
/// another channel's rows out.
#[tokio::test]
async fn desk_read_returns_this_channels_recent_messages_in_order() {
    let (context, events, _dir) = context_with_desks_history().await;
    events
        .append(
            &context.company,
            CompanyEvent::AgentReply {
                chat_id: "platform".to_string(),
                agent_id: "engineer".to_string(),
                text: "not brand's business".to_string(),
                steps: Vec::new(),
                task_id: None,
                outputs: Vec::new(),
                parent: None,
                mentions: Vec::new(),
                mention_depth: 0,
                audience: Vec::new(),
            },
        )
        .await
        .expect("seeded");
    for text in ["first", "second", "third"] {
        events
            .append(&context.company, brand_reply("designer", text, Vec::new()))
            .await
            .expect("seeded");
    }
    let result = crate::runtime::delegation::with_turn_conversation(
        Some("brand".to_string()),
        ReadTool(context).execute(serde_json::json!({})),
    )
    .await
    .expect("the tool runs");
    assert!(!result.is_error, "{result:?}");
    let text = tool_result_text(&result);
    let first = text.find("first").expect("first present");
    let second = text.find("second").expect("second present");
    let third = text.find("third").expect("third present");
    assert!(
        first < second && second < third,
        "rows must read oldest-first: {text}"
    );
    assert!(
        !text.contains("not brand's business"),
        "another channel's row must not leak into this one's read: {text}"
    );
}

/// A private aside this agent was not addressed on is narrowed out —
/// the same audience rule the session delta applies.
#[tokio::test]
async fn desk_read_narrows_an_aside_by_audience() {
    let (context, events, _dir) = context_with_desks_history().await;
    events
        .append(
            &context.company,
            brand_reply("designer", "public line", Vec::new()),
        )
        .await
        .expect("seeded");
    events
        .append(
            &context.company,
            brand_reply(
                "engineer",
                "a private aside between us, not designer",
                vec!["someone_else".to_string()],
            ),
        )
        .await
        .expect("seeded");
    let result = crate::runtime::delegation::with_turn_conversation(
        Some("brand".to_string()),
        ReadTool(context).execute(serde_json::json!({})),
    )
    .await
    .expect("the tool runs");
    assert!(!result.is_error, "{result:?}");
    let text = tool_result_text(&result);
    assert!(text.contains("public line"), "{text}");
    assert!(
        !text.contains("a private aside between us"),
        "an aside this agent is not in the audience of must not be readable by asking \
         for more of the channel: {text}"
    );
}

/// tinysweeper: `truncated` must fire when the scan exhausts its budget
/// before `limit` matching messages are found, not only when it finds
/// `limit` of them — a partial list that reads as complete is worse than
/// an honest "did not look far enough".
#[tokio::test]
async fn desk_read_reports_truncation_when_the_scan_budget_is_exhausted() {
    let (context, events, _dir) = context_with_desks_history().await;
    // Filler on a channel `desk_read` never matches, so every one of these
    // is scanned and none is returned — exhausting `SEARCH_BUDGET` (2048)
    // well before the default `limit` of matching `brand` rows is found.
    for _ in 0..2100 {
        events
            .append(
                &context.company,
                CompanyEvent::AgentReply {
                    chat_id: "platform".to_string(),
                    agent_id: "engineer".to_string(),
                    text: "filler".to_string(),
                    steps: Vec::new(),
                    task_id: None,
                    outputs: Vec::new(),
                    parent: None,
                    mentions: Vec::new(),
                    mention_depth: 0,
                    audience: Vec::new(),
                },
            )
            .await
            .expect("seeded");
    }
    events
        .append(
            &context.company,
            brand_reply("designer", "buried under the filler", Vec::new()),
        )
        .await
        .expect("seeded");
    let result = crate::runtime::delegation::with_turn_conversation(
        Some("brand".to_string()),
        ReadTool(context).execute(serde_json::json!({})),
    )
    .await
    .expect("the tool runs");
    assert!(!result.is_error, "{result:?}");
    let text = tool_result_text(&result);
    assert!(
        text.contains("Older messages are not in this reply"),
        "budget exhaustion must be reported as truncation, not read as a complete \
         (and in this case entirely empty) channel: {text}"
    );
}
