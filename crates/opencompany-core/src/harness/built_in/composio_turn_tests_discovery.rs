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
