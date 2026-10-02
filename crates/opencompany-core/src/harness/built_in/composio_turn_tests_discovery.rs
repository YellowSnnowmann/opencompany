use super::*;

// ---------------------------------------------------------------------------

/// The headline acceptance: an agent with live connections to two large
/// toolkits — 120 GitHub actions and 140 Notion actions, 260 in one open-mode
/// catalogue — discovers the right slug on **each** and calls it, with no human
/// supplying a slug and no provider-specific code anywhere in the path.
///
/// The script is the agent's reasoning, written out: list what exists, narrow to
/// the words in the task, read that one action's parameters, call it. Every step
/// uses only information the previous step's *result* gave it.
#[tokio::test]
async fn an_agent_discovers_and_calls_an_action_unaided_on_two_large_toolkits() {
    // `composio` implies `openhuman`, which is what compiles `crate::harness` at
    // all. Without the gate this statement breaks every feature set that builds
    // these tests without the harness -- `Rust (mail)` and `Rust (mongodb)` both
    // failed on `cannot find \`harness\` in \`crate\``.
    #[cfg(feature = "composio")]
    let _serial = crate::harness::built_in::composio_module::route_test_guard().await;
    let (model_url, script) = spawn_script(vec![
        // 1. What can I do at all? (open mode: 260 actions)
        Turn::Call {
            tool: "composio_list_tools",
            args: json!({}),
        },
        // 2. The task said "issues" — narrow to it and read the parameters.
        Turn::Call {
            tool: "composio_list_tools",
            args: json!({ "search": "list issues", "detail": "schemas" }),
        },
        // 3. Call it with the arguments the schema named.
        Turn::Call {
            tool: "composio_execute",
            args: json!({
                "tool": "GITHUB_LIST_REPOSITORY_ISSUES",
                "arguments": { "owner": "acme", "state": "open" }
            }),
        },
        // 4-5. The same two steps on a different, larger, non-GitHub toolkit.
        Turn::Call {
            tool: "composio_list_tools",
            args: json!({ "toolkits": ["notion"], "search": "search pages", "detail": "schemas" }),
        },
        Turn::Call {
            tool: "composio_execute",
            args: json!({
                "tool": "NOTION_SEARCH_NOTION_PAGE",
                "arguments": { "owner": "acme", "target": "roadmap" }
            }),
        },
        Turn::Say("Two open issues, and the roadmap page."),
    ])
    .await;
    let (composio_url, stub) = spawn_composio_backend().await;

    let dir = tempfile::tempdir().unwrap();
    let (pool, deps, record) = harness(model_url, composio_url, dir.path()).await;

    let outcome = pool
        .run(
            &record.id,
            "ceo",
            "List our open GitHub issues and find the roadmap page in Notion.",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("turn runs");
    assert!(
        outcome.reply.contains("Two open issues"),
        "the turn did not complete: {}",
        outcome.reply
    );

    let advertised = advertised_tools(&script);
    for tool in ["composio_list_tools", "composio_execute"] {
        assert!(
            advertised.contains(&tool.to_string()),
            "`{tool}` was never advertised to the model: {advertised:?}"
        );
    }

    let results = tool_results(&script);
    let joined = results.join("\n=== next tool result ===\n");

    // The discovery step told the agent the listing was incomplete AND how to
    // narrow it. Without this it has no reason to change its request.
    assert!(
        results[0].contains("260 available"),
        "the first listing must report the true catalogue size: {}",
        results[0]
    );
    assert!(
        results[0].contains("TRUNCATED") && results[0].contains("`search`"),
        "the oversized listing must describe its own cut: {}",
        results[0]
    );

    // The narrowed step delivered exactly the schema needed to call it.
    assert!(
        joined.contains("GITHUB_LIST_REPOSITORY_ISSUES"),
        "the needle slug never reached the model: {joined}"
    );
    assert!(
        joined.contains("\"state\"") && joined.contains("\"open\""),
        "the parameter schema never reached the model: {joined}"
    );
    assert!(
        joined.contains("NOTION_SEARCH_NOTION_PAGE"),
        "the second toolkit's needle never reached the model: {joined}"
    );

    // Both calls actually reached the provider — the acceptance is "calls it",
    // not "talks about calling it".
    let executed = stub.executed.lock().unwrap().clone();
    assert_eq!(
        executed,
        vec![
            "GITHUB_LIST_REPOSITORY_ISSUES".to_string(),
            "NOTION_SEARCH_NOTION_PAGE".to_string()
        ],
        "the agent did not reach both providers: {executed:?}; tool results: {results:#?}"
    );

    // The generic fix, stated as a wire fact: the second Notion listing carried
    // `toolkits=notion`, so narrowing happened server-side too rather than by
    // fetching 260 actions and throwing 259 away.
    let queries = stub.tool_queries.lock().unwrap().clone();
    assert!(
        queries.iter().any(|q| q.as_deref() == Some("notion")),
        "the toolkit narrowing never reached the backend: {queries:?}"
    );

    // Nothing the model saw was big enough for the harness's own cut to fire —
    // so every cut in this turn was one that counted itself and said how to ask
    // for less.
    for (index, result) in results.iter().enumerate() {
        assert!(
            result.len() < HARNESS_TOOL_RESULT_BUDGET_BYTES,
            "tool result {index} is {} bytes — at or past the harness budget, which cuts \
             anonymously: {result}",
            result.len()
        );
    }
}

/// A BYOK action makes it from a supervised model turn through the connector
/// module, carrying the company's own route and returning the module result to
/// the model. The module boundary is scripted so CI does not need its native
/// artifact or a live Composio key.
#[cfg(feature = "composio")]
#[tokio::test]
async fn a_supervised_byok_turn_executes_through_the_connector_module() {
    let _serial = crate::harness::built_in::composio_module::route_test_guard().await;
    let (model_url, script) = spawn_script(vec![
        Turn::Call {
            tool: "composio_execute",
            args: json!({
                "tool": "GITHUB_LIST_REPOSITORY_ISSUES",
                "arguments": { "owner": "acme", "state": "open" }
            }),
        },
        Turn::Say("The module found issue 42."),
    ])
    .await;
    let _test_responses = crate::harness::built_in::composio_module::set_test_responses([
        (
            openhuman_core::modules::connectors::methods::CONFIGURE.to_string(),
            json!({}),
        ),
        (
            openhuman_core::modules::connectors::methods::EXECUTE.to_string(),
            json!({
                "data": { "number": 42, "title": "A real issue" },
                "successful": true,
                "costUsd": 0.0
            }),
        ),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let composio = TenantComposio::from_access(
        "https://api.tinyhumans.ai",
        crate::company::composio::ComposioAccess {
            mode: crate::company::composio::ComposioMode::Byok,
            credential: Credential::from_value("ak_company_key"),
        },
        vec!["github".to_string()],
    );
    let (pool, deps, record) = harness_with_composio(model_url, composio, dir.path()).await;
    let outcome = pool
        .run(
            &record.id,
            "ceo",
            "Find the open issues in our GitHub account.",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("supervised turn runs");

    assert!(outcome.reply.contains("issue 42"), "{}", outcome.reply);
    let calls = crate::harness::built_in::composio_module::take_test_calls().await;
    assert_eq!(calls.len(), 2, "configure and execute must use the module");
    assert_eq!(
        calls[0].0,
        openhuman_core::modules::connectors::methods::CONFIGURE
    );
    assert_eq!(calls[0].1[0]["route"], "direct");
    assert_eq!(calls[0].1[0]["api_key"], "ak_company_key");
    assert_eq!(
        calls[1].0,
        openhuman_core::modules::connectors::methods::EXECUTE
    );
    assert_eq!(calls[1].1[0]["tool"], "GITHUB_LIST_REPOSITORY_ISSUES");
    assert_eq!(calls[1].1[0]["arguments"]["owner"], "acme");
    let results = tool_results(&script).join("\n");
    assert!(
        results.contains("42"),
        "module result did not reach the model: {results}"
    );
}
