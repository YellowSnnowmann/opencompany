use super::tests_capped_halt::{deps, deps_with_source, tools_record, write_wf};
use super::*;

use crate::company::parse_workflow;

/// T3 — `on_error = "route"` plus an `error`-labeled edge routes the failure
/// item down the recovery branch.
#[tokio::test]
async fn t3_on_error_route_sends_failure_down_the_error_edge() {
    let src = r#"
id = "t3"
name = "T3"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "call"
kind = "tool_call"
name = "Call"
on_error = "route"
[node.config]
slug = "bogus_tool"
[[node]]
id = "recover"
kind = "output"
name = "Recover"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "call"
[[edge]]
from = "call"
to = "done"
[[edge]]
from = "call"
to = "recover"
label = "error"
"#;
    let dir = tempfile::tempdir().unwrap();
    let file = parse_workflow(src).expect("parses");
    let run = run_workflow(
        Arc::new(HarnessPool::new()),
        deps(dir.path()),
        &tools_record(),
        &file,
        serde_json::json!({ "seed": 1 }),
        &WorkflowRunContext::new(false),
    )
    .await
    .expect("run completes via the recovery route");
    let recover_items = &run.output["nodes"]["recover"]["items"];
    assert!(
        recover_items
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "the recovery node should receive the routed error item: {}",
        run.output
    );
    assert!(
        run.output.to_string().contains("bogus_tool"),
        "{}",
        run.output
    );
}

/// T4 — `requires_approval = true` pauses the node before it runs; the run
/// reports it on `pending_approvals`.
#[tokio::test]
async fn t4_requires_approval_pauses_the_run() {
    let src = r#"
id = "t4"
name = "T4"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "gate"
kind = "tool_call"
name = "Gate"
requires_approval = true
[node.config]
slug = "csv_export"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "gate"
[[edge]]
from = "gate"
to = "done"
"#;
    let dir = tempfile::tempdir().unwrap();
    let file = parse_workflow(src).expect("parses");
    let run = run_workflow(
        Arc::new(HarnessPool::new()),
        deps(dir.path()),
        &tools_record(),
        &file,
        serde_json::json!({ "seed": 1 }),
        &WorkflowRunContext::new(false),
    )
    .await
    .expect("run pauses cleanly");
    assert!(
        run.pending_approvals.iter().any(|id| id == "gate"),
        "the approval-gated node should be pending: {:?}",
        run.pending_approvals
    );
}
/// T5 — an `http_request` to a loopback address is refused by the upstream
/// `url_guard` SSRF check (the happy path is impossible offline by design, so
/// the guard-in-path is proven via the denial). `on_error` defaults to
/// `stop`, so the run fails with the guard error.
#[tokio::test]
async fn t5_http_request_to_loopback_is_ssrf_denied() {
    let src = r#"
id = "t5"
name = "T5"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "fetch"
kind = "http_request"
name = "Fetch"
[node.config]
method = "GET"
url = "http://127.0.0.1:9/"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "fetch"
[[edge]]
from = "fetch"
to = "done"
"#;
    let dir = tempfile::tempdir().unwrap();
    let file = parse_workflow(src).expect("parses");
    let err = run_workflow(
        Arc::new(HarnessPool::new()),
        deps(dir.path()),
        &tools_record(),
        &file,
        serde_json::json!({ "seed": 1 }),
        &WorkflowRunContext::new(false),
    )
    .await
    .expect_err("the SSRF guard must block the loopback request");
    assert!(
        err.to_string().contains("http_request"),
        "the failure should come from the guarded http client: {err}"
    );
}

/// A run context with the dry flag set. `WorkflowRunContext::new` takes
/// `scheduled`, not `dry_run` — the dry flag defaults to false and is
/// flipped after construction, exactly as the run route does.
fn dry_context() -> WorkflowRunContext {
    let mut ctx = WorkflowRunContext::new(false);
    ctx.dry_run = true;
    ctx
}

/// **Issue #1048.** The same graph as `t5`, dry-run: a target the real run
/// refuses must not report `ok`.
///
/// Test run is the one control an operator has for checking a graph before
/// arming it on a schedule, so a green dry run followed by a real run that
/// cannot start is worse than no dry run at all — it converts "I checked it"
/// into a false belief.
///
/// A loopback target is the lever because the upstream guard refuses
/// private/loopback addresses *regardless of the company's allowlist*
/// (see `t5_http_request_to_loopback_is_ssrf_denied`), so the verdict is
/// decidable from the URL alone — no DNS, no request, nothing performed.
#[tokio::test]
async fn a_dry_run_refuses_a_target_the_real_run_would_refuse() {
    let src = r#"
id = "dry-ssrf"
name = "Dry SSRF"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "fetch"
kind = "http_request"
name = "Fetch"
[node.config]
method = "GET"
url = "http://127.0.0.1:9/"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "fetch"
[[edge]]
from = "fetch"
to = "done"
"#;
    let dir = tempfile::tempdir().unwrap();
    let file = parse_workflow(src).expect("parses");
    let err = run_workflow(
        Arc::new(HarnessPool::new()),
        deps(dir.path()),
        &tools_record(),
        &file,
        serde_json::json!({ "seed": 1 }),
        &dry_context(),
    )
    .await
    .expect_err("a dry run must refuse a target the real run refuses");
    assert!(
        err.to_string().contains("http_request"),
        "the dry refusal should name the node, as the live one does: {err}"
    );
}

// --- P2: the six new node kinds, end to end through the engine -----------

/// Runs `src` through the full translate → compile → engine pipeline with a
/// tools-granting record and the given `input`.
async fn run_src(dir: &std::path::Path, src: &str, input: Value) -> Result<WorkflowRun> {
    let file = parse_workflow(src).expect("parses");
    run_workflow(
        Arc::new(HarnessPool::new()),
        deps(dir),
        &tools_record(),
        &file,
        input,
        &WorkflowRunContext::new(false),
    )
    .await
}

/// T-switch — each edge label is a case name; the matched case receives the
/// item and the others don't. A missing field routes to the `default` port.
#[tokio::test]
async fn t_switch_routes_each_case_and_default() {
    let src = r#"
id = "sw_wf"
name = "Switch WF"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "route"
kind = "switch"
name = "Route"
[node.config]
field = "kind"
[[node]]
id = "paid_out"
kind = "output"
name = "Paid"
[[node]]
id = "free_out"
kind = "output"
name = "Free"
[[node]]
id = "default_out"
kind = "output"
name = "Default"
[[edge]]
from = "start"
to = "route"
[[edge]]
from = "route"
to = "paid_out"
label = "paid"
[[edge]]
from = "route"
to = "free_out"
label = "free"
[[edge]]
from = "route"
to = "default_out"
label = "default"
"#;
    let dir = tempfile::tempdir().unwrap();

    // A matching case value routes to just that branch.
    let run = run_src(dir.path(), src, serde_json::json!({ "kind": "paid" }))
        .await
        .expect("matched run completes");
    assert!(
        !run.output["nodes"]["paid_out"]["items"].is_null(),
        "the `paid` case should receive the item: {}",
        run.output
    );
    assert!(
        run.output["nodes"]["free_out"].is_null(),
        "the unmatched `free` case should never run: {}",
        run.output
    );

    // A missing field falls to the engine's `default` fallback port.
    let run = run_src(dir.path(), src, serde_json::json!({ "other": 1 }))
        .await
        .expect("default run completes");
    assert!(
        !run.output["nodes"]["default_out"]["items"].is_null(),
        "a null discriminant should route to the `default` branch: {}",
        run.output
    );
}

/// T-split_out → transform → merge over a 3-element list: the list fans out
/// into three items, each transformed, then merged back into one stream.
#[tokio::test]
async fn t_split_out_transform_merge_over_a_list() {
    let src = r#"
id = "fan_wf"
name = "Fan WF"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "split"
kind = "split_out"
name = "Split"
[node.config]
path = "values"
[[node]]
id = "double"
kind = "transform"
name = "Double"
[node.config.set]
wrapped = "=item"
[[node]]
id = "join"
kind = "merge"
name = "Merge"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "split"
[[edge]]
from = "split"
to = "double"
[[edge]]
from = "double"
to = "join"
[[edge]]
from = "join"
to = "done"
"#;
    let dir = tempfile::tempdir().unwrap();
    let run = run_src(dir.path(), src, serde_json::json!({ "values": [1, 2, 3] }))
        .await
        .expect("fan-out run completes");
    let merged = run.output["nodes"]["join"]["items"]
        .as_array()
        .expect("merge emitted items");
    assert_eq!(
        merged.len(),
        3,
        "3 list elements → 3 merged items: {}",
        run.output
    );
    // Each transformed item wrapped its scalar under `wrapped`.
    let wrapped: Vec<i64> = merged
        .iter()
        .filter_map(|i| i["json"]["wrapped"].as_i64())
        .collect();
    assert_eq!(wrapped, vec![1, 2, 3], "{}", run.output);
}

/// T-transform — the REQUIRED proof that `=`-bindings resolve engine-side
/// with ZERO OpenCompany evaluation: a dotted shorthand (`=item.brief`) and a
/// jq program (`=.items | length`) both resolve against the run scope.
#[tokio::test]
async fn t_transform_resolves_expr_bindings_engine_side() {
    let src = r#"
id = "tf_wf"
name = "Transform WF"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "tf"
kind = "transform"
name = "Reshape"
[node.config.set]
topic = "=item.brief"
count = "=.items | length"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "tf"
[[edge]]
from = "tf"
to = "done"
"#;
    let dir = tempfile::tempdir().unwrap();
    let run = run_src(dir.path(), src, serde_json::json!({ "brief": "launch" }))
        .await
        .expect("transform run completes");
    let item = &run.output["nodes"]["tf"]["items"][0]["json"];
    assert_eq!(
        item["topic"], "launch",
        "dotted =item.brief: {}",
        run.output
    );
    assert_eq!(item["count"], 1, "jq =.items | length: {}", run.output);
}

/// T-output_parser — a valid item passes the schema; a malformed one with
/// `auto_fix = false` surfaces a capability error routed by `on_error =
/// continue` into a data item, so the run completes carrying the failure.
#[tokio::test]
async fn t_output_parser_validates_and_routes_failure() {
    let base = r#"
id = "op_wf"
name = "Parser WF"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "parse"
kind = "output_parser"
name = "Parse"
on_error = "continue"
[node.config]
auto_fix = false
[node.config.schema]
type = "object"
required = ["name"]
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "parse"
[[edge]]
from = "parse"
to = "done"
"#;
    let dir = tempfile::tempdir().unwrap();

    // A schema-valid item passes straight through.
    let run = run_src(dir.path(), base, serde_json::json!({ "name": "Ada" }))
        .await
        .expect("valid item passes");
    assert!(
        run.output.to_string().contains("Ada"),
        "the validated item should flow through: {}",
        run.output
    );

    // A malformed item (missing `name`) fails validation; `auto_fix = false`
    // makes it a hard error, which `on_error = continue` turns into a data
    // item so the run still completes.
    let run = run_src(dir.path(), base, serde_json::json!({ "other": 1 }))
        .await
        .expect("run completes despite the schema failure");
    assert!(
        run.output.to_string().contains("name"),
        "the continued error item should name the missing property: {}",
        run.output
    );
}

/// T-output_parser AUTO-FIX (issue #661, M4) — the vendored-engine drift
/// catcher. With `auto_fix` DEFAULTED (true) and no roster LLM wired, a
/// schema failure sends the engine to the `llm` capability to *repair* the
/// value. The unwired `llm` must surface the SCHEMA failure, so the
/// `on_error = continue` error item names the missing property — NOT the
/// generic "no roster agent" message that used to mask it.
///
/// This exercises the real request the engine builds
/// (`task = "coerce_to_schema"` with the schema `errors`), so a future
/// tinyflows pin that reshapes that request fails here rather than silently
/// reverting to the masked message.
#[tokio::test]
async fn t_output_parser_auto_fix_surfaces_schema_failure_not_no_roster_agent() {
    // Note: NO `auto_fix = false` — the default (true) is exactly the path
    // that reaches the `llm` auto-fix capability.
    let src = r#"
id = "op_af_wf"
name = "Parser Auto-fix WF"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "parse"
kind = "output_parser"
name = "Parse"
on_error = "continue"
[node.config.schema]
type = "object"
required = ["name"]
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "parse"
[[edge]]
from = "parse"
to = "done"
"#;
    let dir = tempfile::tempdir().unwrap();
    let run = run_src(dir.path(), src, serde_json::json!({ "other": 1 }))
        .await
        .expect("run completes despite the schema failure");

    let message = run.output["nodes"]["parse"]["items"][0]["json"]["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a routed error item: {}", run.output));
    assert!(
        message.contains("schema validation") && message.contains("name"),
        "the auto-fix path must surface the schema failure: {message}"
    );
    assert!(
        !message.contains("no roster agent"),
        "the schema failure must not be masked by the bare-LLM message: {message}"
    );
}

/// T-sub_workflow — a `sub_workflow` node runs a child saved on disk (depth
/// 1), resolved by id through the wired source directory.
#[tokio::test]
async fn t_sub_workflow_runs_a_disk_child() {
    let source = tempfile::tempdir().unwrap();
    // The child stamps a distinctive marker so we can prove it ran.
    write_wf(
        source.path(),
        "child",
        r#"
id = "child"
name = "Child"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "mark"
kind = "transform"
name = "Mark"
[node.config.set]
child_marker = "=42"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "mark"
[[edge]]
from = "mark"
to = "done"
"#,
    );
    let parent = r#"
id = "parent"
name = "Parent"
[[node]]
id = "start"
kind = "trigger"
name = "Start"
[[node]]
id = "sub"
kind = "sub_workflow"
name = "Sub"
[node.config]
workflow_id = "child"
[[node]]
id = "done"
kind = "output"
name = "Done"
[[edge]]
from = "start"
to = "sub"
[[edge]]
from = "sub"
to = "done"
"#;
    let home = tempfile::tempdir().unwrap();
    let file = parse_workflow(parent).expect("parent parses");
    let run = run_workflow(
        Arc::new(HarnessPool::new()),
        deps_with_source(home.path(), source.path()),
        &tools_record(),
        &file,
        serde_json::json!({ "seed": 1 }),
        &WorkflowRunContext::new(false),
    )
    .await
    .expect("sub_workflow run completes");
    assert!(
        run.output.to_string().contains("child_marker"),
        "the child workflow should have run and stamped its marker: {}",
        run.output
    );
}

/// A provider that records the last user message of every inference call and
/// **holds the `slow` node open until the operator cancels** (bounded), so a
/// cancel deterministically lands while a child `sub_workflow` node is
/// mid-flight. It distinguishes child nodes by a marker string authored into
/// each node's `prompt`: the node after `slow` must never be invoked once a
/// parent cancel has propagated into the child run.
pub(super) struct RecordingSlowProvider {
    pub(super) seen: Arc<std::sync::Mutex<Vec<String>>>,
    pub(super) entered_slow: Arc<tokio::sync::Notify>,
    pub(super) cancel: crate::ports::workflow_runner::RunCancel,
}
