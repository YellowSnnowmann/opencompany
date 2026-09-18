use super::*;

/// Codex (PR #1883 review comment 3892522591): a node under `on_error =
/// "continue"`/`"route"` settles the run `Degraded`, and `runner.rs`
/// already writes a per-node notice for it — but `summarize_run` never
/// read `run.nodes`, so an agent-started run through this exact case
/// summarized as "reached its terminal node(s) without pausing for
/// approval" with no hint anything went wrong. This pins the fix: a row
/// still `Error` after settle must show up in the tool result.
#[test]
fn the_summary_says_when_a_node_errored_and_the_run_continued() {
    let file = crate::company::parse_workflow(DEMO_WF).unwrap();
    let degraded = WorkflowRun {
        output: json!({ "nodes": { "worker": { "items": ["partial"] } } }),
        pending_approvals: Vec::new(),
        deliveries: Vec::new(),
        cancelled: false,
        nodes: vec![crate::ports::WorkflowRunNodeRow {
            node_id: "worker".into(),
            status: WorkflowNodeStatus::Error,
            elapsed_ms: 12,
            diagnostics: Vec::new(),
        }],
        notices: Vec::new(),
        board: Vec::new(),
        blocked_nodes: Vec::new(),
        approvals: Vec::new(),
    };
    let md = summarize_run(&file, &degraded, "run-degraded", RunOutputStored::Stored);
    assert!(
        md.contains("did not finish cleanly, and the run continued past it"),
        "{md}"
    );
    assert!(md.contains("worker"), "{md}");
    assert!(
        md.contains("NOT a clean run"),
        "an agent skimming for the happy-path sentence must not miss this: {md}"
    );

    // A node that finished clean says nothing about it — an ordinary
    // summary is unchanged.
    let clean = WorkflowRun {
        nodes: vec![crate::ports::WorkflowRunNodeRow {
            node_id: "worker".into(),
            status: WorkflowNodeStatus::Ok,
            elapsed_ms: 12,
            diagnostics: Vec::new(),
        }],
        ..degraded.clone()
    };
    let md = summarize_run(&file, &clean, "run-clean", RunOutputStored::Stored);
    assert!(!md.contains("did not finish cleanly"), "{md}");
    assert!(!md.contains("NOT a clean run"), "{md}");

    // A blocked node must not ALSO print here — it is already named by the
    // "Blocked, waiting on a person" paragraph above, sourced from
    // `blocked_nodes`, not from a node row's own status (the host never
    // leaves a blocked row `Error`; see `WorkflowNodeStatus::Blocked`'s doc).
    let blocked = WorkflowRun {
        nodes: vec![crate::ports::WorkflowRunNodeRow {
            node_id: "worker".into(),
            status: WorkflowNodeStatus::Blocked,
            elapsed_ms: 12,
            diagnostics: Vec::new(),
        }],
        blocked_nodes: vec![crate::ports::WorkflowBlockedNode {
            node_id: "worker".into(),
            tools: vec!["send_email".into()],
            approval_ids: vec!["appr-1".into()],
            unparkable: 0,
            stranded: 0,
            blockers: 0,
        }],
        ..degraded.clone()
    };
    let md = summarize_run(&file, &blocked, "run-blocked", RunOutputStored::Stored);
    assert!(md.contains("Blocked, waiting on a person"), "{md}");
    assert!(
        !md.contains("did not finish cleanly"),
        "a blocked row must not double up with the degraded paragraph: {md}"
    );
}

/// T3: a successful run populates the cache and the tool's JSON payload
/// carries the run id; a cancelled or failed run stores nothing.
#[tokio::test]
async fn success_populates_cache_and_payload_carries_run_id() {
    let dir = tempfile::tempdir().unwrap();
    seed_demo_workflow(dir.path());

    let cache = RunOutputCache::default();
    let (ok, _runner) = run_tool_over(
        dir.path(),
        WorkflowRun {
            output: json!({ "nodes": { "worker": { "items": ["did the thing"] } } }),
            pending_approvals: Vec::new(),
            deliveries: Vec::new(),
            cancelled: false,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        },
        WorkflowRefQueue::default(),
        cache.clone(),
    );
    let result = ok.execute(json!({ "id": "demo" })).await.expect("execute");
    assert!(!result.is_error, "{result:?}");
    assert_eq!(cache.len(), 1, "a successful run must be cached");
    // The payload the brain sees carries a run id string.
    let payload = match &result.content[0] {
        openhuman_core::skills::types::ToolContent::Json { data } => data.clone(),
        other => panic!("expected JSON payload, got {other:?}"),
    };
    assert!(
        payload.get("run_id").and_then(Value::as_str).is_some(),
        "{payload}"
    );

    // A cancelled run caches nothing.
    let cancel_cache = RunOutputCache::default();
    let (cancelled, _cancel_runner) = run_tool_over(
        dir.path(),
        WorkflowRun {
            output: json!({ "nodes": {} }),
            pending_approvals: Vec::new(),
            deliveries: Vec::new(),
            cancelled: true,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        },
        WorkflowRefQueue::default(),
        cancel_cache.clone(),
    );
    assert!(
        cancelled
            .execute(json!({ "id": "demo" }))
            .await
            .expect("execute")
            .is_error
    );
    assert_eq!(cancel_cache.len(), 0, "a cancelled run stores nothing");
}

/// T4: the full agent round-trip. A run whose node holds multiple >120-char
/// items is summarised (clipping the preview), then `read_run_output` over
/// the shared cache returns every item — including every character the
/// preview dropped — verbatim.
#[tokio::test]
async fn read_run_output_returns_every_dropped_char_after_a_run() {
    let dir = tempfile::tempdir().unwrap();
    seed_demo_workflow(dir.path());

    let long_a = "A".repeat(400);
    let long_b = format!("B{}", "b".repeat(500));
    let cache = RunOutputCache::default();
    let (run, _runner) = run_tool_over(
        dir.path(),
        WorkflowRun {
            output: json!({ "nodes": { "worker": { "items": [long_a, long_b] } } }),
            pending_approvals: Vec::new(),
            deliveries: Vec::new(),
            cancelled: false,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        },
        WorkflowRefQueue::default(),
        cache.clone(),
    );
    let summary = run.execute(json!({ "id": "demo" })).await.expect("execute");
    // The summary only previews the last item, clipped.
    assert!(
        summary.output_for_llm(true).contains("last of 2 items"),
        "{summary:?}"
    );

    let reader = ReadRunOutputTool::new(CompanyId::new("acme"), cache);
    // Pass the display name "Worker" — the case-insensitive fallback resolves
    // it to id `worker`.
    let read = reader
        .execute(json!({ "run_id": "", "node": "Worker" }))
        .await
        .expect("execute");
    // Empty run_id is rejected before lookup.
    assert!(read.is_error, "empty run_id must be rejected");

    // Read with the real run id (recover it from the payload).
    let payload = match &summary.content[0] {
        openhuman_core::skills::types::ToolContent::Json { data } => data.clone(),
        other => panic!("{other:?}"),
    };
    let run_id = payload.get("run_id").and_then(Value::as_str).unwrap();
    let read = reader
        .execute(json!({ "run_id": run_id, "node": "Worker" }))
        .await
        .expect("execute");
    assert!(!read.is_error, "{read:?}");
    let text = read.output_for_llm(false);
    assert!(text.contains("Item 1 of 2:"), "{text}");
    assert!(text.contains("Item 2 of 2:"), "{text}");
    assert!(text.contains(&"A".repeat(400)), "item 1 must be verbatim");
    assert!(text.contains(&"b".repeat(500)), "item 2 must be verbatim");
}

/// T4b: the run summary lists a node by its display name but the cache is
/// keyed by id, and in `DEMO_WF` the two genuinely differ — `id = "done"`,
/// `name = "Report"`. Both the display name the summary prints ("Report")
/// and the raw id ("done") must resolve through `read_run_output`. This is
/// the non-degenerate name/id pair the case-only fallback never covered.
#[tokio::test]
async fn read_run_output_resolves_display_name_and_id() {
    let dir = tempfile::tempdir().unwrap();
    seed_demo_workflow(dir.path());

    let cache = RunOutputCache::default();
    let (run, _runner) = run_tool_over(
        dir.path(),
        WorkflowRun {
            // The terminal node's id is `done`; the summary shows its name,
            // "Report".
            output: json!({ "nodes": { "done": { "items": ["the report body"] } } }),
            pending_approvals: Vec::new(),
            deliveries: Vec::new(),
            cancelled: false,
            nodes: Vec::new(),
            notices: Vec::new(),
            board: Vec::new(),
            blocked_nodes: Vec::new(),
            approvals: Vec::new(),
        },
        WorkflowRefQueue::default(),
        cache.clone(),
    );
    let summary = run.execute(json!({ "id": "demo" })).await.expect("execute");
    // The summary prints the display name and now the id alongside it.
    let md = summary.output_for_llm(true);
    assert!(md.contains("**Report**"), "{md}");
    assert!(md.contains("`done`"), "{md}");
    let run_id = match &summary.content[0] {
        openhuman_core::skills::types::ToolContent::Json { data } => data
            .get("run_id")
            .and_then(Value::as_str)
            .unwrap()
            .to_string(),
        other => panic!("{other:?}"),
    };
    let reader = ReadRunOutputTool::new(CompanyId::new("acme"), cache);

    // The display name from the summary resolves to id `done`.
    let by_name = reader
        .execute(json!({ "run_id": run_id, "node": "Report" }))
        .await
        .expect("execute");
    assert!(!by_name.is_error, "display name must resolve: {by_name:?}");
    assert!(
        by_name.output_for_llm(false).contains("the report body"),
        "{by_name:?}"
    );

    // The raw id resolves too, so both paths are live.
    let by_id = reader
        .execute(json!({ "run_id": run_id, "node": "done" }))
        .await
        .expect("execute");
    assert!(!by_id.is_error, "id must resolve: {by_id:?}");
    assert!(
        by_id.output_for_llm(false).contains("the report body"),
        "{by_id:?}"
    );
}

/// T5: paging. Two windows concatenate to the original, each page stays
/// within budget, and a boundary that lands on a multibyte char never
/// splits a codepoint.
#[test]
fn paging_reassembles_and_never_splits_a_codepoint() {
    // 300 'é' (2 bytes each) = 600 bytes. A 401-byte budget forces a break
    // right where the next 'é' would cross it — proving whole-char taking.
    let full: String = "é".repeat(300);
    let budget = 401;
    let (p1, next) = page_run_output(&full, 0, budget);
    let n1 = next.expect("more remains");
    assert!(p1.len() <= budget, "page 1 is {} bytes", p1.len());
    // A page of whole 'é' has even byte length — never an odd split.
    assert_eq!(p1.len() % 2, 0, "a split codepoint would make this odd");
    let (p2, next2) = page_run_output(&full, n1, budget);
    assert!(p2.len() <= budget, "page 2 is {} bytes", p2.len());
    // Continue to the end and prove the concatenation reconstructs the whole.
    let mut assembled = p1.clone();
    assembled.push_str(&p2);
    let mut off = next2;
    while let Some(o) = off {
        let (p, nxt) = page_run_output(&full, o, budget);
        assert!(p.len() <= budget);
        assembled.push_str(&p);
        off = nxt;
    }
    assert_eq!(assembled, full, "the pages must reassemble the original");

    // Every char is valid UTF-8 by construction (String), so decoding the
    // reassembly back is lossless.
    assert_eq!(assembled.chars().count(), 300);
}

/// T5b: the tool's own paging clips a huge single item under the budget and
/// hands back an offset that reads the remainder.
#[tokio::test]
async fn read_run_output_pages_a_huge_item_under_budget() {
    let dir = tempfile::tempdir().unwrap();
    // One item far larger than the 16 KiB tool-result budget.
    let huge = "z".repeat(40_000);
    let cache = RunOutputCache::default();
    cache.store(
        "run-huge",
        "demo",
        json!({ "worker": { "items": [huge] } }),
        Vec::new(),
    );
    let _ = dir;
    let reader = ReadRunOutputTool::new(CompanyId::new("acme"), cache);

    let first = reader
        .execute(json!({ "run_id": "run-huge", "node": "worker" }))
        .await
        .expect("execute");
    assert!(!first.is_error, "{first:?}");
    let text = first.output_for_llm(false);
    assert!(
        text.len() <= crate::harness::build::TOOL_RESULT_BUDGET_BYTES,
        "page too big"
    );
    assert!(text.contains("Continue with offset="), "{text}");
    // Pull the offset out and read the next page.
    let off: usize = text
        .rsplit("offset=")
        .next()
        .and_then(|t| t.trim_end_matches('.').parse().ok())
        .expect("an offset to continue from");
    let second = reader
        .execute(json!({ "run_id": "run-huge", "node": "worker", "offset": off }))
        .await
        .expect("execute");
    assert!(!second.is_error, "{second:?}");
    assert!(
        off > 0 && off < 40_100,
        "offset {off} advances into the item"
    );
}

/// T6: the error arms are actionable — unknown run names the console
/// fallback, unknown node lists the valid ids with item counts, and an empty
/// node says so rather than returning nothing.
#[tokio::test]
async fn read_run_output_error_arms_are_actionable() {
    let cache = RunOutputCache::default();
    cache.store(
        "run-1",
        "demo",
        json!({
            "worker": { "items": ["one", "two"] },
            "done": { "items": [] }
        }),
        Vec::new(),
    );
    let reader = ReadRunOutputTool::new(CompanyId::new("acme"), cache);

    // Unknown run → names the cache scope + the console fallback.
    let unknown_run = reader
        .execute(json!({ "run_id": "ghost", "node": "worker" }))
        .await
        .expect("execute");
    assert!(unknown_run.is_error);
    let t = unknown_run.output_for_llm(false);
    assert!(t.contains("console"), "{t}");

    // Unknown node → lists valid ids + counts.
    let unknown_node = reader
        .execute(json!({ "run_id": "run-1", "node": "nope" }))
        .await
        .expect("execute");
    assert!(unknown_node.is_error);
    let t = unknown_node.output_for_llm(false);
    assert!(t.contains("`worker` (2 item(s))"), "{t}");
    assert!(t.contains("`done` (0 item(s))"), "{t}");

    // Empty node → a success that says it is empty, not an error, not silence.
    let empty = reader
        .execute(json!({ "run_id": "run-1", "node": "done" }))
        .await
        .expect("execute");
    assert!(!empty.is_error, "{empty:?}");
    assert!(
        empty.output_for_llm(false).contains("no items"),
        "{empty:?}"
    );
}

/// T7: eviction (oldest run drops past the run-count bound) and the
/// oversized-run announce (a run over the hard per-run ceiling is refused,
/// reported as `Oversized`, and never cached).
#[test]
fn cache_evicts_oldest_and_refuses_an_oversized_run() {
    let cache = RunOutputCache::default();
    for i in 0..(RUN_OUTPUT_CACHE_RUNS + 3) {
        let outcome = cache.store(
            &format!("run-{i}"),
            "demo",
            json!({ "worker": { "items": [format!("item-{i}")] } }),
            Vec::new(),
        );
        assert!(matches!(outcome, RunOutputStored::Stored));
    }
    assert_eq!(cache.len(), RUN_OUTPUT_CACHE_RUNS, "bounded to the run cap");
    // The three oldest runs were evicted; the newest survive.
    assert!(cache.get("run-0").is_none(), "oldest must be evicted");
    assert!(
        cache
            .get(&format!("run-{}", RUN_OUTPUT_CACHE_RUNS + 2))
            .is_some(),
        "newest must survive"
    );

    // A run whose node map serializes past the hard per-run ceiling is
    // refused, announced (not silently dropped), and never cached.
    let fresh = RunOutputCache::default();
    let giant = "g".repeat(RUN_OUTPUT_ENTRY_MAX_BYTES + 1);
    let outcome = fresh.store(
        "run-giant",
        "demo",
        json!({ "worker": { "items": [giant] } }),
        Vec::new(),
    );
    assert!(
        matches!(outcome, RunOutputStored::Oversized { .. }),
        "must refuse"
    );
    assert_eq!(fresh.len(), 0, "an oversized run must not be cached");
    assert!(fresh.get("run-giant").is_none());
}

/// **The regression this whole change exists for.**
///
/// A workflow run taking a claim while the chat cycle has work staged must
/// leave that work alone. Before the scoping, `claim_as` opened with a
/// global `clear()`, so this exact interleaving destroyed a chat turn's
/// staged card and its refusal — and the turn had already told the operator
/// the card was opened.
///
/// Runs are `tokio::spawn`ed and are not under the cycle lock (#401 allows
/// several at once), so this interleaving is reachable rather than
/// theoretical.
#[tokio::test]
async fn a_workflow_claim_leaves_a_concurrent_chat_turns_staged_work_intact() {
    let queue = DelegationQueue::default();

    // A chat turn is mid-flight with a card staged and a refusal recorded.
    let _chat = queue.claim();
    assert_eq!(stage(&queue, card("chat")), Staged::Queued);
    queue.push_refusal("nonexistent-desk".to_string());

    // A workflow run claims, concurrently. This is the moment that used to
    // wipe the chat's bucket.
    let run = queue.claim_board("run-1");
    run.scoped(async { assert_eq!(stage(&queue, card("run")), Staged::Queued) })
        .await;

    // The chat's staged card and refusal are both still there…
    assert_eq!(queue.queued(), 1);
    assert_eq!(queue.refusals_queued(), 1);
    assert_eq!(titles(queue.drain(MAX_DELEGATIONS_PER_TURN)), ["chat"]);
    assert_eq!(
        queue.drain_refusals(MAX_DELEGATIONS_PER_TURN),
        ["nonexistent-desk"]
    );

    // …and the run still has its own, drained separately.
    let run_drained = run
        .scoped(async { queue.drain(MAX_DELEGATIONS_PER_TURN) })
        .await;
    assert_eq!(titles(run_drained), ["run"]);
}

/// The half of the defect that is **live today**, with no drain wired and
/// nothing else changed.
///
/// `DelegateToDeskTool` calls [`DelegationQueue::push_refusal`] on the
/// ungrounded path *before* it consults the claim, so an invented desk named
/// by a workflow node already reaches the shared vector. A chat turn's
/// `drain_refusals` would then take it, record it on that turn's card, and
/// clear it — a hand-off nobody on that turn attempted.
#[tokio::test]
async fn a_runs_ungrounded_hand_off_is_not_recorded_on_a_chat_turns_card() {
    let queue = DelegationQueue::default();
    let _chat = queue.claim();

    let run = queue.claim_board("run-1");
    run.scoped(async { queue.push_refusal("marketing".to_string()) })
        .await;

    // Nothing to report on the chat turn's card: it attempted no hand-off.
    assert_eq!(queue.refusals_queued(), 0);
    assert!(queue.drain_refusals(MAX_DELEGATIONS_PER_TURN).is_empty());

    // The run's own refusal is intact and still its own to read.
    let seen = run
        .scoped(async { queue.drain_refusals(MAX_DELEGATIONS_PER_TURN) })
        .await;
    assert_eq!(seen, ["marketing"]);
}

/// Two runs and the chat cycle interleaved: each sees only its own, and
/// neither draining nor claiming reaches across.
#[tokio::test]
async fn two_runs_and_the_chat_cycle_neither_drain_nor_clear_each_other() {
    let queue = DelegationQueue::default();

    let _chat = queue.claim();
    assert_eq!(stage(&queue, card("chat")), Staged::Queued);

    let run_a = queue.claim_board("run-a");
    run_a
        .scoped(async { assert_eq!(stage(&queue, card("a")), Staged::Queued) })
        .await;

    // B claims *after* A staged — the acquire-time clear must not reach A.
    let run_b = queue.claim_board("run-b");
    run_b
        .scoped(async { assert_eq!(stage(&queue, card("b")), Staged::Queued) })
        .await;

    assert_eq!(queue.queued(), 1, "the chat cycle sees only its own");
    assert_eq!(run_a.scoped(async { queue.queued() }).await, 1);
    assert_eq!(run_b.scoped(async { queue.queued() }).await, 1);

    // Draining A takes A's and only A's.
    let drained_a = run_a
        .scoped(async { queue.drain(MAX_DELEGATIONS_PER_TURN) })
        .await;
    assert_eq!(titles(drained_a), ["a"]);
    assert_eq!(queue.queued(), 1);
    assert_eq!(run_b.scoped(async { queue.queued() }).await, 1);

    assert_eq!(titles(queue.drain(MAX_DELEGATIONS_PER_TURN)), ["chat"]);
    let drained_b = run_b
        .scoped(async { queue.drain(MAX_DELEGATIONS_PER_TURN) })
        .await;
    assert_eq!(titles(drained_b), ["b"]);
}

/// A claim's `Drop` discards its own bucket and un-claims its own scope —
/// and reaches nothing else. A cancelled run's staged writes dying with the
/// run is the intended semantics; a chat turn's surviving it is the point.
#[tokio::test]
async fn dropping_a_claim_discards_only_its_own_bucket() {
    let queue = DelegationQueue::default();

    let _chat = queue.claim();
    assert_eq!(stage(&queue, card("chat")), Staged::Queued);

    {
        let run = queue.claim_board("run-1");
        run.scoped(async { assert_eq!(stage(&queue, card("run")), Staged::Queued) })
            .await;
        run.scoped(async { queue.push_refusal("ghost".to_string()) })
            .await;
    } // the run is cancelled here

    // Its bucket went with it, and its scope is claimable again from
    // scratch rather than left committed.
    let after = CURRENT_SCOPE
        .scope(DelegationScope::Run("run-1".to_string()), async {
            (queue.queued(), queue.refusals_queued(), queue.claim_state())
        })
        .await;
    assert_eq!(after, (0, 0, DrainClaim::Unclaimed));

    // The chat turn is untouched — still claimed, still holding its card.
    assert_eq!(queue.claim_state(), DrainClaim::Full);
    assert_eq!(titles(queue.drain(MAX_DELEGATIONS_PER_TURN)), ["chat"]);
}

/// The #176 scope chain is per claimant, and its depth accounting is
/// unchanged by that.
///
/// Depth is still exactly `chain.len()` and still gates a hand-off at the
/// bound; what it no longer does is count another claimant's nesting.
#[tokio::test]
async fn a_scope_chain_is_per_claimant_and_depth_is_unchanged() {
    let queue = DelegationQueue::default();
    let _chat = queue.claim();

    let _outer = queue.enter_scope("design".to_string());
    assert_eq!(queue.scope_depth(), 1);
    assert_eq!(queue.scope_chain(), ["design"]);

    let run = queue.claim_board("run-1");
    run.scoped(async {
        // A run opens its own chain at depth 0 however deep the chat is.
        assert_eq!(queue.scope_depth(), 0);
        assert!(queue.scope_chain().is_empty());

        let _a = queue.enter_scope("eng".to_string());
        let _b = queue.enter_scope("qa".to_string());
        assert_eq!(queue.scope_depth(), 2);
        assert_eq!(queue.scope_chain(), ["eng", "qa"]);
    })
    .await;

    // The chat's chain is exactly as deep as it was left, and its guard
    // popped from its own chain rather than the run's.
    assert_eq!(queue.scope_depth(), 1);
    assert_eq!(queue.scope_chain(), ["design"]);

    // Depth still gates at the bound, counting this claimant's chain only:
    // one level deep against a bound of 1 refuses…
    assert_eq!(
        queue.push_within_cap(hand_off(), MAX_DELEGATIONS_PER_TURN, 1),
        Staged::NoDrain(NoDrainReason::Depth)
    );
    // …and against a bound of 2 it stages, which a run's two levels would
    // have blocked had they been counted here.
    assert_eq!(
        queue.push_within_cap(hand_off(), MAX_DELEGATIONS_PER_TURN, 2),
        Staged::Queued
    );
}
