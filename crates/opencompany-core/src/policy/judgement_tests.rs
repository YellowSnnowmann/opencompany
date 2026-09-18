use super::*;
use serde_json::json;

/// Every test below that does not name a path is about the **agent** path —
/// the one #338's rules govern. The authored-node path has its own section
/// at the end of this module.
fn judge_bare(tool: &str) -> Judgement {
    judge_agent(tool, &json!({}))
}

fn judge_agent(tool: &str, args: &Value) -> Judgement {
    judge(tool, args, CallPath::Agent)
}

fn judge_node(tool: &str, args: &Value) -> Judgement {
    judge(tool, args, CallPath::AuthoredWorkflowNode)
}

/// The agent-path half of #338's acceptance after #658's ruling: sends,
/// payments and deletes stop, and an undeclared publish-shaped call fails
/// closed. The declared `publish_artifact` exception is pinned separately.
///
/// `delete` has no declared [`EffectGroup`] of its own — the taxonomy names
/// six consequence classes and destruction is not one — so it arrives here
/// as the unbounded-reach case. `shell` is how a run deletes.
#[test]
fn agent_calls_that_send_pay_delete_or_use_an_undeclared_publish_shape_stop() {
    // Declared, so the verdict names the consequence the table declares.
    assert_eq!(
        judge_bare("media_generate_image"),
        Judgement::Stop(StopReason::Irreversible(EffectGroup::Spend)),
    );
    assert_eq!(
        judge_agent("composio_execute", &json!({ "tool": "GMAIL_SEND_EMAIL" })),
        Judgement::Stop(StopReason::Irreversible(EffectGroup::Send)),
    );
    // Undeclared, so they stop on the fail-closed ground instead — see
    // `an_undeclared_tool_stops_without_borrowing_a_guessed_reason`.
    for tool in ["send_email", "publish_post", "pay_invoice"] {
        assert!(
            matches!(judge_bare(tool), Judgement::Stop(_)),
            "`{tool}` must stop"
        );
    }
    assert_eq!(
        judge_bare("shell"),
        Judgement::Stop(StopReason::UnboundedReach),
        "deleting is reached through arbitrary code, not a declared group"
    );
}

/// The other half of the acceptance, and the half that makes the feature
/// usable rather than merely safe.
#[test]
fn reads_searches_and_drafts_do_not_stop() {
    for tool in [
        "file_read",
        "glob",
        "grep",
        "list",
        "read_workspace_state",
        "memory_recall",
        "image_info",
        "query_company",
    ] {
        assert_eq!(judge_bare(tool), Judgement::Silent, "`{tool}` reads");
    }
}

/// The agent's own scratch space. These mutate, so any tier that parks or
/// denies a mutating call keeps its own answer — but they are the
/// low-consequence writes #444 found safe enough to hand over for a week,
/// so the judgement arm adds no stop of its own where a tier allowed them.
#[test]
fn the_agents_own_drafts_do_not_stop() {
    for tool in [
        "file_write",
        "edit",
        "apply_patch",
        "csv_export",
        "memory_store",
        "memory_forget",
    ] {
        assert_eq!(judge_bare(tool), Judgement::Silent, "`{tool}` is a draft");
    }
}

/// A metered call must never stop here: openhuman resolves a
/// `RequireApproval` inline, so a parked search is a search that never
/// happens.
///
/// This is the sharp edge of the whole module, and the pair below is why
/// the rule reads reach as well as group.
///
/// Both are declared [`EffectGroup::Spend`], so a rule keyed on the group
/// alone parked every search in the company. They differ in reach, and the
/// reach is the part that matters: `web_search` is [`Reach::Money`] — the
/// spend buys the call, nothing changes and nothing leaves, and the daily
/// cap governs it — while media generation is [`Reach::Consequence`],
/// because it "moves real money on submit" (issue #109).
#[test]
fn a_metered_read_never_stops_but_a_real_spend_does() {
    for tool in ["web_search", "media_generate_image", "media_generate_video"] {
        assert_eq!(
            consequence_of(tool, &json!({})).group,
            EffectGroup::Spend,
            "`{tool}` is declared Spend — that is the trap this test guards"
        );
    }
    assert_eq!(
        judge_bare("web_search"),
        Judgement::Silent,
        "billed, but nothing changes and nothing leaves"
    );
    for tool in ["media_generate_image", "media_generate_video"] {
        assert_eq!(
            judge_bare(tool),
            Judgement::Stop(StopReason::Irreversible(EffectGroup::Spend)),
            "`{tool}` moves real money on submit"
        );
    }
}

/// Arbitrary code and arbitrary addresses, which the table already refuses
/// a standing grant for the same reason.
///
#[test]
fn unbounded_tools_stop() {
    // The filter is retained rather than dropped: nothing in `UNBOUNDED` is
    // deferred today, but `DEFERRED` is what is withheld from the rule and
    // this test is about the rule. The carve-outs have their own tests.
    for tool in UNBOUNDED.iter().filter(|t| !DEFERRED.contains(t)) {
        assert_eq!(
            judge_bare(tool),
            Judgement::Stop(StopReason::UnboundedReach),
            "`{tool}` is unbounded"
        );
    }
}

/// The company writing to its own note tree is not an unbounded act, and
/// stopping it broke publishing (13 `publish_turn_test` cases at the
/// time; the suite now stands at 19, all green). They are
/// `Standing::PerCall` — issue #444 refuses them a standing grant because
/// they overwrite guidance the operator authored — and that is a different
/// reason from "what this reaches cannot be bounded". A tier that parks or
/// denies them keeps its own answer; this arm adds nothing on top.
#[test]
fn writing_the_companys_own_notes_is_not_unbounded() {
    for tool in ["workspace_write", "workspace_create"] {
        assert!(
            !consequence_of(tool, &json!({})).standing.is_grantable(),
            "`{tool}` is PerCall — the trap this test guards"
        );
        assert_eq!(judge_bare(tool), Judgement::Silent, "`{tool}` is internal");
    }
}

/// The carve-out, pinned so it is a decision rather than a drift.
///
/// `publish_artifact` satisfies rule 1 outright — declared `Publish` and
/// `Consequence` — and is silent anyway. Issue #658 **ruled** that this is
/// the behaviour it wants rather than a stopgap: under `full` a company
/// publishes without asking, and an operator who wants to be asked names it
/// in `always_approve`, which is read before this arm.
///
/// Asserted on **both** paths: the carve-out is not part of #674's split, so
/// a rework of that split must not accidentally make one path speak.
#[test]
fn publish_artifact_is_excluded_by_the_658_ruling() {
    let c = consequence_of("publish_artifact", &json!({}));
    assert_eq!(c.group, EffectGroup::Publish);
    assert_eq!(c.reach, Reach::Consequence);
    assert!(
        is_irreversible_group(c.group),
        "it qualifies — the exclusion is what keeps it silent"
    );
    for path in [CallPath::Agent, CallPath::AuthoredWorkflowNode] {
        assert_eq!(
            judge("publish_artifact", &json!({}), path),
            Judgement::Silent,
            "{path:?}: #658 ruled `full` publishes without asking; \
             `always_approve` is the override"
        );
    }
    // Templated arguments return an authored node to the agent rule — and
    // the agent rule is silent here too, so the carve-out survives the one
    // condition that reopens every other node.
    assert_eq!(
        judge_node("publish_artifact", &json!({ "body": "=previous.output" })),
        Judgement::Silent,
    );
}

/// The arm adds nothing under `auto` (issue #560) — pinned as a rule over
/// the whole declaration table, not as a snapshot of what it holds today.
///
/// `auto` parks `parks_under_supervision() && !is_grantable()`, so the only
/// calls it allows that `supervised` would park are the `Grantable` ones.
/// Today every `d_grantable` row is `EffectGroup::Other`, none is in
/// `UNBOUNDED` and none carries an amount, so no rule fires — but that is a
/// property of the table's current contents, and nothing pinned it.
///
/// The day somebody adds `d_grantable("send_invoice", EffectGroup::Send,
/// Reach::Consequence)`, `auto` would shift silently under operators who
/// chose it — exactly the failure the placement argument exists to prevent.
/// This walks every declared tool and fails on that day instead.
///
/// Judged on [`CallPath::Agent`] deliberately: that is the strict path, so
/// this asserts the strongest form of the claim. If it held only on the
/// authored-node path it would be asserting #674's split, not `auto`.
#[test]
fn the_arm_adds_nothing_under_auto() {
    let mut checked = 0;
    for tool in declared_tools() {
        let c = consequence_of(tool, &json!({}));
        if c.parks_under_auto() {
            continue; // `auto` already stops it; this arm is never reached.
        }
        checked += 1;
        assert_eq!(
            judge_agent(tool, &json!({})),
            Judgement::Silent,
            "`{tool}` runs under `auto` but this arm would stop it — `auto` \
             has shifted under operators who chose it"
        );
    }
    assert!(
        checked > 0,
        "no declared tool runs under `auto` — the walk asserted nothing"
    );
}

/// Every carve-out must be a tool a rule would otherwise have stopped.
///
/// The point of the list is to hold back calls this gate *would* gate. An
/// entry that no rule fires on is not a deferral, it is a misunderstanding —
/// and it would sit there looking like a decision. This fails if the
/// declaration table moves under a carve-out, so a deferral cannot quietly
/// become a no-op.
///
/// It also fails if `DEFERRED` is emptied. Without that, deleting the
/// `publish_artifact` carve-out would make this loop over nothing and pass —
/// a test that scores green having asserted zero things, which is the
/// failure mode it exists to prevent.
///
/// `http_request` / `curl` / `web_fetch` were here and are not any more:
/// #674 ruled them **scoped by path**, not deferred, so they are governed by
/// `CallPath` and are asserted in the authored-node section below.
#[test]
fn every_deferred_tool_would_otherwise_be_stopped() {
    assert!(
        !DEFERRED.is_empty(),
        "DEFERRED is empty — this test would assert nothing. #658 ruled the \
         `publish_artifact` carve-out correct; read it before removing it"
    );
    for tool in DEFERRED {
        let c = consequence_of(tool, &json!({}));
        let would_stop = (is_irreversible_group(c.group) && c.reach == Reach::Consequence)
            || is_unbounded(tool, c);
        assert!(
            would_stop,
            "`{tool}` is deferred but no rule would stop it — the entry is inert"
        );
        assert_eq!(
            judge_bare(tool),
            Judgement::Silent,
            "`{tool}` is deferred; do not 'fix' this without reading #658"
        );
    }
}

/// Everything held back must be a declared tool, or the exclusion silently
/// stops matching anything.
#[test]
fn every_deferred_tool_is_declared() {
    for tool in DEFERRED {
        assert!(
            declared_tools().any(|d| d == *tool),
            "`{tool}` is in DEFERRED but not declared"
        );
    }
}

/// A rename in the declaration table must fail loudly here rather than
/// silently dropping a tool out of `UNBOUNDED`.
#[test]
fn every_unbounded_tool_is_declared() {
    for tool in UNBOUNDED {
        assert!(
            declared_tools().any(|d| d == *tool),
            "`{tool}` is in UNBOUNDED but not declared — it would never match"
        );
    }
}

/// Fail closed: a tool nobody declared stops rather than passing.
#[test]
fn an_undeclared_tool_stops() {
    assert_eq!(
        judge_bare("frobnicate_the_widget"),
        Judgement::Stop(StopReason::Undeclared)
    );
}

/// ...and it says so, rather than repeating a guess as if it were a fact.
///
/// `write_file` is undeclared and `consequence_of` labels it `Sign`, purely
/// because the name contains "file". Both verdicts stop the call; only one
/// of them tells the operator something true. This is the regression guard
/// for that: the reason must be the honest one, not the borrowed one.
#[test]
fn an_undeclared_tool_stops_without_borrowing_a_guessed_reason() {
    assert_eq!(
        consequence_of("write_file", &json!({})).group,
        EffectGroup::Sign,
        "the name heuristic guesses Sign — that is what must not be quoted"
    );
    assert_eq!(
        judge_bare("write_file"),
        Judgement::Stop(StopReason::Undeclared)
    );
}

/// ...but an undeclared tool that only reads still does not stop, so
/// fail-closed does not collapse into stop-everything.
#[test]
fn an_undeclared_read_does_not_stop() {
    assert_eq!(judge_bare("get_the_weather"), Judgement::Silent);
}

/// A Composio slug nobody has classified is treated as a send by
/// `consequence_of`, and a send stops.
#[test]
fn an_unclassified_composio_action_stops() {
    let args = json!({ "tool": "SOME_TOOLKIT_ACTION_NOBODY_CLASSIFIED" });
    assert_eq!(
        judge_agent("composio_execute", &args),
        Judgement::Stop(StopReason::Irreversible(EffectGroup::Send))
    );
}

/// `judge` computes `consequence_of` once for its own fail-closed arm and
/// must not ask `floor::evaluate` to compute it a second time — for an
/// uncatalogued `composio_execute` slug that call is not free of side
/// effect, and running it twice doubles the `catalogue_miss` warning it
/// logs (issues #754, #1818). Counts every `WARN` emitted by one `judge`
/// call rather than reading a captured field: under this fixture the
/// only `WARN` either run produces is that one line, so a count of 2 is
/// the regression and 1 is the fix.
///
/// `openhuman`-gated: the catalogue-miss path only exists once a catalogue
/// is linked in to miss against — see `CatalogLookup::CatalogueAbsent`.
#[cfg(feature = "openhuman")]
#[test]
fn an_uncatalogued_composio_call_logs_the_catalogue_miss_once_not_twice() {
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt;

    struct CountWarnings(Arc<Mutex<usize>>);
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountWarnings {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() == tracing::Level::WARN {
                *self.0.lock().expect("count lock") += 1;
            }
        }
    }

    let warnings = Arc::new(Mutex::new(0usize));
    let subscriber = tracing_subscriber::registry().with(CountWarnings(Arc::clone(&warnings)));

    tracing::subscriber::with_default(subscriber, || {
        let args = json!({ "tool": "SOME_TOOLKIT_ACTION_NOBODY_CLASSIFIED" });
        judge_agent("composio_execute", &args);
    });

    assert_eq!(
        *warnings.lock().expect("count lock"),
        1,
        "consequence_of must run once per judge call, not twice"
    );
}

/// Money named in the arguments stops even when the tool's own class is the
/// unclassified bucket.
#[test]
fn a_declared_amount_stops_an_otherwise_silent_tool() {
    // `list` is a declared pure read: silent on its own.
    assert_eq!(judge_bare("list"), Judgement::Silent);
    assert_eq!(
        judge_agent("list", &json!({ "amount_usd": 12.5 })),
        Judgement::Stop(StopReason::MoneyLeaves)
    );
}

/// Zero is not money leaving, and neither is a negative.
#[test]
fn a_zero_or_negative_amount_is_not_money_leaving() {
    assert_eq!(
        judge_agent("list", &json!({ "amount_usd": 0 })),
        Judgement::Silent
    );
    assert_eq!(
        judge_agent("list", &json!({ "amount_usd": -5 })),
        Judgement::Silent
    );
}

/// The gate holds no state, so it cannot learn: the sixth identical call is
/// judged exactly as the first. #563 is where consent lives, and consent is
/// an operator writing a rule — not this noticing a habit.
#[test]
fn the_gate_does_not_learn_from_repetition() {
    let args = json!({ "to": "customer@example.com" });
    let first = judge_agent("send_email", &args);
    for _ in 0..5 {
        assert_eq!(
            judge_agent("send_email", &args),
            first,
            "repetition must not soften the verdict"
        );
    }
}

/// Same call, same answer — the property that makes a stop explainable from
/// the trace months later, and the reason this is not a model call.
#[test]
fn the_verdict_is_deterministic() {
    let args = json!({ "body": "hello", "to": "a@example.com" });
    assert_eq!(
        judge_agent("send_email", &args),
        judge_agent("send_email", &args)
    );
}

/// Every stop can say why, in the operator's terms rather than this
/// module's.
#[test]
fn every_stop_reason_describes_itself() {
    let groups = [
        EffectGroup::Spend,
        EffectGroup::Send,
        EffectGroup::Sign,
        EffectGroup::Publish,
        EffectGroup::Hire,
        EffectGroup::Identity,
        EffectGroup::Other,
    ];
    for group in groups {
        assert!(!StopReason::Irreversible(group).describe().is_empty());
    }
    assert!(!StopReason::UnboundedReach.describe().is_empty());
    assert!(!StopReason::MoneyLeaves.describe().is_empty());
    assert!(!StopReason::Undeclared.describe().is_empty());
}

// ---- #674: the split by path ------------------------------------------
//
// Everything above is the agent path. These are the authored-node path and
// the boundary condition that governs when a node leaves it.

/// #614's position, stated as a rule over the whole strict set rather than
/// as the three tools whose tests forced the question.
///
/// A node an operator authored has passed the manifest grant and the
/// authoring refusal, and they saw the call. This module is silent on it.
#[test]
fn an_authored_node_with_literal_arguments_is_not_judged() {
    // Every tool the agent path stops for a reason other than a carve-out,
    // so this is the rule and not a sample of it.
    let cases: &[(&str, Value)] = &[
        ("shell", json!({ "command": "echo hello > out.txt" })),
        (
            "http_request",
            json!({ "method": "POST", "url": "https://api.example.com/x" }),
        ),
        ("git_operations", json!({ "op": "push" })),
        ("run_workflow", json!({ "workflow_id": "nightly" })),
        ("media_generate_image", json!({ "prompt": "a cat" })),
        (
            "composio_execute",
            json!({ "tool": "GMAIL_SEND_EMAIL", "to": "a@example.com" }),
        ),
        ("frobnicate_the_widget", json!({})),
        ("list", json!({ "amount_usd": 400.0 })),
    ];
    for (tool, args) in cases {
        assert!(
            matches!(judge_agent(tool, args), Judgement::Stop(_)),
            "`{tool}` must stop on the agent path — otherwise this case \
             proves nothing about the split"
        );
        assert_eq!(
            judge_node(tool, args),
            Judgement::Silent,
            "`{tool}` was authored into a saved workflow: two operator gates \
             and the operator saw the call (#674 / #614)"
        );
    }
}

/// **The boundary condition.** An authored node whose arguments come from an
/// upstream node's output was never pre-declared, so it is judged as an
/// agent call.
///
/// This is the test that stops the split being a hole. Delete it and one
/// `shell` node whose `command` is `=previous.output` walks through a gate
/// justified by an operator having seen a command they never saw.
#[test]
fn an_authored_node_templated_from_upstream_output_follows_the_agent_rule() {
    assert_eq!(
        judge_node("shell", &json!({ "command": "=previous.output" })),
        Judgement::Stop(StopReason::UnboundedReach),
        "the operator declared the shape; the content arrives at run time"
    );
    // The same node with the command actually written out is silent — which
    // is what makes the assertion above about templating rather than about
    // `shell`.
    assert_eq!(
        judge_node("shell", &json!({ "command": "echo hi" })),
        Judgement::Silent,
    );
    // Money, and the jq binding form as well as the dotted shorthand.
    assert_eq!(
        judge_node(
            "list",
            &json!({ "amount_usd": 400.0, "note": "=.items[0]" })
        ),
        Judgement::Stop(StopReason::MoneyLeaves),
    );
}

/// An expression is a *value* anywhere in the descriptor, not a top-level
/// key — so the check recurses. A node that buried `=previous.output` one
/// object deep would otherwise read as fully authored.
#[test]
fn a_templated_value_is_found_at_any_depth() {
    for args in [
        json!("=item.everything"),
        json!({ "cmd": "=item.x" }),
        json!({ "body": { "cmd": "=item.x" } }),
        json!({ "argv": ["bash", "-c", "=item.x"] }),
        json!({ "body": { "steps": [{ "cmd": "=item.x" }] } }),
    ] {
        assert!(
            any_argument_is_templated(&args),
            "{args} is templated and must be judged as an agent call"
        );
        assert_eq!(
            judge_node("shell", &args),
            Judgement::Stop(StopReason::UnboundedReach),
            "{args}"
        );
    }
}

/// ...and a descriptor with nothing templated in it is not dragged back to
/// the agent rule by a value that merely looks structured.
#[test]
fn a_fully_authored_descriptor_is_not_templated() {
    for args in [
        json!({}),
        json!({ "command": "echo hi", "timeout": 30, "quiet": true }),
        json!({ "url": "https://example.com/a=b?c=d" }),
        json!({ "argv": ["bash", "-c", "ls"], "env": { "A": "1" } }),
        json!({ "body": null }),
    ] {
        assert!(!any_argument_is_templated(&args), "{args}");
        assert_eq!(judge_node("shell", &args), Judgement::Silent, "{args}");
    }
}

/// The two paths are not two names for one behaviour. If a refactor
/// collapsed them — passing `Agent` everywhere, or `AuthoredWorkflowNode`
/// everywhere — every test above could still pass while the split was gone.
#[test]
fn the_two_paths_actually_differ() {
    // An ACTING command. Since issue #875 the agent path reads the command
    // rather than the tool name, so a read is judged silent on BOTH paths
    // and would collapse this assertion without the split having changed.
    let args = json!({ "command": "rm -rf ." });
    assert_ne!(
        judge_agent("shell", &args),
        judge_node("shell", &args),
        "the paths have collapsed into one — #674's split is not implemented"
    );
}
