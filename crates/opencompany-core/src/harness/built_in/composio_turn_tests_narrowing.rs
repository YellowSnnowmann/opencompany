use super::*;

/// The retry-loop guard, stated directly: an agent that repeats the identical
/// oversized listing gets the identical result *and* an explicit instruction not
/// to. Before the fix the repeated result was a silent fragment, which is why
/// the agent had no reason to stop and eventually hit the repetition guard.
#[tokio::test]
async fn a_repeated_oversized_listing_still_tells_the_agent_to_narrow_instead() {
    let (model_url, script) = spawn_script(vec![
        Turn::Call {
            tool: "composio_list_tools",
            args: json!({}),
        },
        Turn::Call {
            tool: "composio_list_tools",
            args: json!({ "toolkits": ["github", "notion"] }),
        },
        Turn::Say("I need to narrow the listing."),
    ])
    .await;
    let (composio_url, _stub) = spawn_composio_backend().await;

    let dir = tempfile::tempdir().unwrap();
    let (pool, deps, record) = harness(model_url, composio_url, dir.path()).await;
    pool.run(
        &record.id,
        "ceo",
        "What can you do?",
        &deps,
        crate::runtime::delegation::ChatTarget::default(),
    )
    .await
    .expect("turn runs");

    let results = tool_results(&script);
    assert!(
        results.len() >= 2,
        "expected two distinct listings: {results:#?}"
    );
    for result in &results {
        assert!(
            result.contains("Do NOT repeat this call unchanged"),
            "a cut listing must break the loop it would otherwise cause: {result}"
        );
        assert!(
            result.len() < HARNESS_TOOL_RESULT_BUDGET_BYTES,
            "{} bytes",
            result.len()
        );
    }
}

/// A newly-connected provider with a large catalogue works with no
/// provider-specific change: the same words that found the GitHub action find
/// the Notion one, and the toolkit slug is never hardcoded anywhere in the path.
#[tokio::test]
async fn a_narrowed_listing_on_an_unknown_toolkit_needs_no_provider_specific_code() {
    let (model_url, script) = spawn_script(vec![
        Turn::Call {
            tool: "composio_list_tools",
            args: json!({ "toolkits": ["notion"], "search": "operation number 42" }),
        },
        Turn::Say("Found it."),
    ])
    .await;
    let (composio_url, _stub) = spawn_composio_backend().await;

    let dir = tempfile::tempdir().unwrap();
    let (pool, deps, record) = harness(model_url, composio_url, dir.path()).await;
    pool.run(
        &record.id,
        "ceo",
        "Find the Notion action.",
        &deps,
        crate::runtime::delegation::ChatTarget::default(),
    )
    .await
    .expect("turn runs");

    let joined = tool_results(&script).join("\n");
    assert!(
        joined.contains("NOTION_ACTION_042"),
        "the search did not find the action on an arbitrary toolkit: {joined}"
    );
    assert!(
        !joined.contains("TRUNCATED"),
        "a single match is not a cut: {joined}"
    );
}

/// Issue #1759, wired end to end: the capability-grounding + Composio-routing
/// brief is not just a pure function — it reaches the model. This drives the
/// real harness (real `build_agent`, real `HostedProvider`) and reads the system
/// prompt off the wire, proving the agent is actually told to route GitHub /
/// connected SaaS through `composio_execute` and NOT to hand-roll `http_request`
/// against a provider API. A unit test on `composio_brief` cannot make this
/// claim; it pins the text, not whether the text was ever composed into a turn.
#[tokio::test]
async fn the_composio_routing_brief_reaches_the_model_system_prompt() {
    let (model_url, script) = spawn_script(vec![Turn::Say("Understood.")]).await;
    let (composio_url, _stub) = spawn_composio_backend().await;

    let dir = tempfile::tempdir().unwrap();
    let (pool, deps, record) = harness(model_url, composio_url, dir.path()).await;
    pool.run(
        &record.id,
        "ceo",
        "What can you do on GitHub?",
        &deps,
        crate::runtime::delegation::ChatTarget::default(),
    )
    .await
    .expect("turn runs");

    let system = system_prompts(&script).join("\n");
    // The routing rule reached the model.
    assert!(
        system.contains("composio_execute"),
        "the system prompt must name the Composio call path: {system}"
    );
    assert!(
        system.contains("http_request") && system.contains("api.github.com"),
        "the system prompt must warn off hand-rolling provider APIs: {system}"
    );
    // The grounding half reached it too.
    assert!(
        system.to_lowercase().contains("no browser"),
        "the system prompt must ground the agent against promising browser actions: {system}"
    );
}
