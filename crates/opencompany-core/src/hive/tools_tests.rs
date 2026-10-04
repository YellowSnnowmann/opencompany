//! Unit tests for the in-flight registry and the tool adapter.

use super::*;
use async_trait::async_trait;
use tinyhivemind_core::embed::ConversationKind;

fn desk_surface(id: &str) -> ConversationRef {
    ConversationRef {
        id: id.to_string(),
        kind: ConversationKind::Desk,
        thread_root: None,
    }
}

fn desk_turn(agent: &str) -> InFlight {
    InFlight::new(
        CompanyId::new("acme"),
        format!("acme--{agent}"),
        agent,
        desk_surface("engineering"),
    )
    .with_hive(HiveScope {
        hive_id: Some("engineering".to_string()),
        episode_id: Some("ep-1".to_string()),
        thread: None,
    })
}

#[test]
fn the_registry_holds_one_turn_per_agent() {
    let registry = Arc::new(InFlightRegistry::new());
    let ticket = registry
        .begin(desk_turn("ceo"))
        .expect("first turn registers");
    assert!(registry.is_in_flight("acme--ceo"));
    let second = registry.begin(desk_turn("ceo"));
    assert_eq!(
        second.err(),
        Some(InFlightError::AlreadyInFlight {
            runtime_agent_id: "acme--ceo".to_string()
        })
    );
    let other = registry
        .begin(desk_turn("engineer"))
        .expect("a different agent runs concurrently");
    assert_eq!(registry.in_flight(), vec!["acme--ceo", "acme--engineer"]);
    let finished = ticket.finish();
    assert_eq!(
        finished.hive.and_then(|hive| hive.episode_id).as_deref(),
        Some("ep-1"),
        "the turn's hive scope survives to its finish"
    );
    assert!(!registry.is_in_flight("acme--ceo"));
    assert!(registry.is_in_flight("acme--engineer"));
    drop(other);
    assert!(
        registry.in_flight().is_empty(),
        "dropping a ticket deregisters"
    );
}

#[test]
fn finishing_a_turn_frees_the_agent_for_the_next_one() {
    let registry = Arc::new(InFlightRegistry::new());
    let first = registry.begin(desk_turn("ceo")).unwrap();
    let _ = first.finish();
    let second = registry
        .begin(desk_turn("ceo"))
        .expect("the agent is free again");
    assert!(registry.is_in_flight("acme--ceo"));
    assert_eq!(second.runtime_agent_id(), "acme--ceo");
    assert_eq!(second.snapshot().agent_id, "ceo");
    drop(second);
    assert!(!registry.is_in_flight("acme--ceo"));
}

/// A tool that reports the context it ran under.
struct ContextEcho;

#[async_trait]
impl Tool for ContextEcho {
    fn name(&self) -> &str {
        "context_echo"
    }
    fn description(&self) -> &str {
        "echoes the run context"
    }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn execute(&self, _args: Value) -> anyhow::Result<ToolResult> {
        Ok(ToolResult::error("executed without a context"))
    }
    async fn execute_with_context(
        &self,
        _args: Value,
        _options: ToolCallOptions,
        context: Option<&dyn ToolRunContext>,
    ) -> anyhow::Result<ToolResult> {
        let Some(context) = context else {
            return self.execute(Value::Null).await;
        };
        let turn = context
            .host_extension()
            .and_then(|ext| ext.downcast_ref::<InFlightContext>())
            .and_then(|ctx| ctx.turn.as_ref());
        Ok(ToolResult::success(format!(
            "thread={} agent={} workspace={}",
            context.thread_id().unwrap_or("-"),
            turn.map_or("-", |t| t.agent_id.as_str()),
            context
                .workspace_root()
                .map_or("-".to_string(), |p| p.display().to_string())
        )))
    }
}

#[tokio::test]
async fn the_adapter_runs_a_tool_under_the_in_flight_context() {
    let adapter = McpToolAdapter::new(Arc::new(ContextEcho));
    let descriptor = adapter.descriptor();
    assert_eq!(descriptor["name"], "context_echo");
    assert_eq!(descriptor["inputSchema"]["type"], "object");

    let context = InFlightContext::new(
        Some(desk_turn("ceo", &["ceo"])),
        Some(PathBuf::from("/tmp/acme/ceo")),
    );
    let result = adapter.execute(json!({}), &context).await;
    assert!(!result.is_error);
    assert_eq!(
        result.output(),
        "thread=engineering agent=ceo workspace=/tmp/acme/ceo"
    );
}

#[tokio::test]
async fn the_adapter_runs_a_tool_under_the_in_flight_context() {
    let adapter = McpToolAdapter::new(Arc::new(ContextEcho));
    let descriptor = adapter.descriptor();
    assert_eq!(descriptor["name"], "context_echo");
    assert_eq!(descriptor["inputSchema"]["type"], "object");

    let context = InFlightContext::new(
        Some(desk_turn("ceo")),
        Some(PathBuf::from("/tmp/acme/ceo")),
    );
    let result = adapter.execute(json!({}), &context).await;
    assert!(!result.is_error);
    assert_eq!(
        result.output(),
        "thread=engineering agent=ceo workspace=/tmp/acme/ceo"
    );
}

/// Every company tool is reached by its bare name: nothing is wrapped in
/// `mcp_call_tool` any more, now that the served speech tools are gone.
#[test]
fn every_tool_is_reached_by_its_bare_name() {
    let (name, args) = via_opencompany_mcp("publish_artifact", json!({ "path": "a.md" }));
    assert_eq!(name, "publish_artifact");
    assert_eq!(args, json!({ "path": "a.md" }));
}
