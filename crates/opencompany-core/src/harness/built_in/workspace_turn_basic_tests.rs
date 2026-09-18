use serde_json::json;

use super::workspace_turn_helpers_tests::*;
use crate::ports::workspace::WorkspaceOrigin;

/// The headline proof: a model, driving a real turn, discovers the workspace,
/// reads a note, and revises it — with the revision token making the full
/// round trip through the model's own context.
#[tokio::test]
async fn a_real_turn_lists_reads_and_revises_a_workspace_note() {
    let (base_url, script) = spawn_script(vec![
        Turn::Call {
            tool: "workspace_list",
            args: json!({}),
        },
        Turn::Call {
            tool: "workspace_read",
            args: json!({ "path": "standards/engineering-standards.md" }),
        },
        Turn::WriteWithObservedRev {
            path: "standards/engineering-standards.md",
            content: "# Engineering\nReview every PR before merge.\nShip on Fridays.",
            delta: 0,
        },
        Turn::Say("Updated the engineering standards."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    // An explicit `workspace` grant — writes must be opted into by name.
    let (pool, deps, record, store) = harness(base_url, "\"workspace\"", dir.path()).await;

    let outcome = pool
        .run(
            &record.id,
            "ceo",
            "Add a Friday shipping rule to our standards.",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("turn runs");
    assert!(
        outcome.reply.contains("Updated the engineering standards."),
        "{}",
        outcome.reply
    );

    // The tools reached the wire under their real names.
    let advertised = advertised_tools(&script);
    for tool in ["workspace_list", "workspace_read", "workspace_write"] {
        assert!(
            advertised.contains(&tool.to_string()),
            "`{tool}` was never advertised to the model: {advertised:?}"
        );
    }

    // The model saw the seeded tree, the note body, and the revision token.
    let results = tool_results(&script);
    let joined = results.join("\n---\n");
    assert!(
        joined.contains("standards/engineering-standards.md"),
        "the listing never reached the model: {joined}"
    );
    assert!(
        joined.contains("Review every PR before merge."),
        "the note body never reached the model: {joined}"
    );
    assert!(
        joined.contains("BEGIN WORKSPACE NOTE"),
        "the untrusted-content fence is missing: {joined}"
    );
    assert!(
        joined.contains("Overwrote the workspace note"),
        "the write did not report success: {joined}"
    );

    // And — the point of the whole exercise — the edit is durable in the store
    // the console reads from, not just in the transcript.
    let (node, body) = store
        .read(&record.id, "n-eng")
        .await
        .unwrap()
        .expect("note still present");
    assert_eq!(
        body,
        "# Engineering\nReview every PR before merge.\nShip on Fridays."
    );
    assert_eq!(node.name, "engineering-standards.md");
}

/// The compare-and-swap guard, proven through a real turn: a model writing with
/// a revision that is not current is refused, the note is untouched, and the
/// refusal is fed back so the agent can recover rather than retry blindly.
#[tokio::test]
async fn a_real_turn_is_refused_when_it_writes_with_a_stale_revision() {
    let (base_url, script) = spawn_script(vec![
        Turn::Call {
            tool: "workspace_read",
            args: json!({ "path": "standards/engineering-standards.md" }),
        },
        // Pretend the operator edited the note between read and write.
        Turn::WriteWithObservedRev {
            path: "standards/engineering-standards.md",
            content: "clobbered",
            delta: -1,
        },
        Turn::Say("I could not apply that edit."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (pool, deps, record, store) = harness(base_url, "\"workspace\"", dir.path()).await;

    let (before, _) = store.read(&record.id, "n-eng").await.unwrap().unwrap();

    pool.run(
        &record.id,
        "ceo",
        "Rewrite the standards.",
        &deps,
        crate::runtime::delegation::ChatTarget::default(),
    )
    .await
    .expect("turn runs");

    let joined = tool_results(&script).join("\n---\n");
    // Before anything else: the write must have been built from a revision the
    // model actually saw. Otherwise the refusal below proves nothing — a
    // never-observed revision is refused with the same message.
    assert!(
        !joined.contains(&UNOBSERVED_REV.to_string()),
        "no `rev=` reached the model, so the refusal below is not evidence of a \
         stale-revision check: {joined}"
    );
    assert!(
        joined.contains("changed since you read it"),
        "the stale write was not refused: {joined}"
    );

    let (after, body) = store.read(&record.id, "n-eng").await.unwrap().unwrap();
    assert_eq!(
        body, "# Engineering\nReview every PR before merge.",
        "a stale write clobbered the note"
    );
    // A write that failed only after touching metadata would leave the body
    // intact and still bump the revision, invalidating every other agent's
    // token for no reason.
    assert_eq!(
        after.updated_at_millis, before.updated_at_millis,
        "a refused write must not bump the revision"
    );
}

/// The grant asymmetry, proven through a real turn: under a bare `*` the model
/// is offered the read tools and NOT `workspace_write`, so it cannot revise
/// operator-owned guidance even if it tries.
#[tokio::test]
async fn a_wildcard_grant_turn_can_read_but_is_never_offered_the_write_tool() {
    let (base_url, script) = spawn_script(vec![
        Turn::Call {
            tool: "workspace_read",
            args: json!({ "path": "standards/engineering-standards.md" }),
        },
        // Nothing stops a model naming a tool it was never offered. Under a
        // bare `*` the write must be *refused*, not merely left unadvertised —
        // advertisement is a hint, the grant check is the control.
        Turn::WriteWithObservedRev {
            path: "standards/engineering-standards.md",
            content: "clobbered",
            delta: 0,
        },
        Turn::Say("Our standard is to review every PR before merge."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (pool, deps, record, store) = harness(base_url, "\"*\"", dir.path()).await;

    let outcome = pool
        .run(
            &record.id,
            "ceo",
            "What is our review standard?",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("turn runs");
    assert!(
        outcome.reply.contains("review every PR"),
        "{}",
        outcome.reply
    );

    let advertised = advertised_tools(&script);
    assert!(
        advertised.contains(&"workspace_read".to_string()),
        "a `*` grant must still confer reads: {advertised:?}"
    );
    assert!(
        !advertised.contains(&"workspace_write".to_string()),
        "a bare `*` grant must NEVER offer the write tool: {advertised:?}"
    );

    let (_, body) = store.read(&record.id, "n-eng").await.unwrap().unwrap();
    assert_eq!(
        body, "# Engineering\nReview every PR before merge.",
        "the unadvertised write tool was called anyway and went through — a bare \
         `*` must refuse it at the grant check, not just omit it from the list"
    );
}

/// Freshness through a real turn: an edit landing between two turns changes
/// what the agent quotes next turn, with no agent rebuild. This is what the
/// per-call store read buys over a session-cached snapshot.
#[tokio::test]
async fn an_edit_between_turns_changes_what_the_next_turn_reads() {
    let (base_url, script) = spawn_script(vec![
        Turn::Call {
            tool: "workspace_read",
            args: json!({ "path": "standards/engineering-standards.md" }),
        },
        Turn::Say("first"),
        Turn::Call {
            tool: "workspace_read",
            args: json!({ "path": "standards/engineering-standards.md" }),
        },
        Turn::Say("second"),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (pool, deps, record, store) = harness(base_url, "\"*\"", dir.path()).await;

    pool.run(
        &record.id,
        "ceo",
        "What is our standard?",
        &deps,
        crate::runtime::delegation::ChatTarget::default(),
    )
    .await
    .expect("first turn");

    // The operator edits the note in the console — the same store handle.
    store
        .write(
            &record.id,
            "n-eng",
            "# Engineering\nDeploy on green only.",
            WorkspaceOrigin::Operator,
        )
        .await
        .unwrap();

    pool.run(
        &record.id,
        "ceo",
        "And now?",
        &deps,
        crate::runtime::delegation::ChatTarget::default(),
    )
    .await
    .expect("second turn");

    let results = tool_results(&script);
    let before = results
        .iter()
        .any(|r| r.contains("Review every PR before merge."));
    let after = results.iter().any(|r| r.contains("Deploy on green only."));
    assert!(
        before,
        "the first turn never saw the original body: {results:?}"
    );
    assert!(
        after,
        "the second turn did not pick up the operator's edit — the tools are caching: {results:?}"
    );
}

/// Issue #417, through the one path that can actually prove it: the harness's
/// own tool-result budget, applied by the real middleware.
///
/// The unit tests in [`workspace_tools`](crate::harness::workspace_tools) can
/// only assert that a read *renders* under some number. They cannot see the
/// second bound — `ToolOutputMiddleware`, fed from
/// [`TOOL_RESULT_BUDGET_BYTES`](crate::harness::build::TOOL_RESULT_BUDGET_BYTES)
/// via `AgentBuilder::context_config` — which cuts every tool result on its way
/// into the model's context. That bound is what made the old 64 KiB read cap a
/// data-loss bug: the module reported nothing dropped, and the model got the
/// first ~16 KiB and an anonymous byte marker.
///
/// So this reads a 20 KiB note through the whole pipeline and asserts on the
/// bytes the *model* received. Two properties, and the second is the one no
/// unit test can reach:
///
/// 1. The read never invites a rewrite of a note it only partly returned.
/// 2. The result arrives whole — closing fence last, and no
///    `tool_result_budget` marker, meaning the outer cut never fired at all.
///
/// (2) failing is the exact shape of the original bug: an unterminated fence
/// means the untrusted-content region was left open, and it means the module's
/// idea of what it returned and the model's idea of what it received have come
/// apart again.
#[tokio::test]
async fn an_oversized_note_reaches_the_model_whole_and_read_only() {
    let (base_url, script) = spawn_script(vec![
        Turn::Call {
            tool: "workspace_read",
            args: json!({ "path": "standards/Big standard.md" }),
        },
        Turn::Say("I read what I could of it."),
    ])
    .await;

    let dir = tempfile::tempdir().unwrap();
    let (pool, deps, record, store) = harness(base_url, "\"workspace\"", dir.path()).await;

    // Larger than the read cap, smaller than the old 64 KiB one — the window
    // in which a note used to be silently shortened and then overwritten.
    let body = "The operator wrote this and expects to keep it. ".repeat(440);
    assert!(
        body.len() > 16 * 1024 && body.len() < 64 * 1024,
        "{}",
        body.len()
    );
    store
        .create(
            &record.id,
            &note("n-big", "Big standard.md", "f-std"),
            Some(&body),
        )
        .await
        .unwrap();

    pool.run(
        &record.id,
        "ceo",
        "What does the big standard say?",
        &deps,
        crate::runtime::delegation::ChatTarget::default(),
    )
    .await
    .expect("turn runs");

    let results = tool_results(&script);
    let read = results
        .iter()
        .find(|r| r.contains("BEGIN WORKSPACE NOTE"))
        .unwrap_or_else(|| panic!("the read result never reached the model: {results:?}"));

    // (1) The model is told it may not write, and is never handed the sentence
    // that drove the overwrite.
    assert!(read.contains("CANNOT be overwritten"), "{read}");
    assert!(
        !read.contains("complete new body"),
        "the model was invited to rewrite a note it only partly received: {read}"
    );

    // (2) The result the model got is the result the module rendered.
    assert!(
        !read.contains("truncated by tool_result_budget"),
        "the harness cut the read result — the two bounds still disagree: {read}"
    );
    let at = read
        .find("--- BEGIN WORKSPACE NOTE ")
        .expect("the read is fenced");
    let nonce = read[at + "--- BEGIN WORKSPACE NOTE ".len()..]
        .split_whitespace()
        .next()
        .expect("the fence carries a nonce");
    assert!(
        read.trim_end()
            .ends_with(&format!("--- END WORKSPACE NOTE {nonce} ---")),
        "the model never received the closing fence, so the untrusted-content region it was \
         warned about was left open: {read}"
    );

    // Not vacuous: the body really did travel, and really was shortened.
    assert!(read.contains("The operator wrote this and expects to keep it."));
    assert!(
        read.contains(&format!("of {} bytes", body.len())),
        "the header should say how much of the note exists: {read}"
    );
}
