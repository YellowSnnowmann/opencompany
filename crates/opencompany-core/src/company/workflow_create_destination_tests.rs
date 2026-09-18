//! workflow_create: output destinations (issue #981) and issue #813's required config.args.

use super::test_support::*;
#[cfg(feature = "openhuman")]
use super::tests_problems::problems_of;
use super::tests_tool_call::tool_call_draft;
#[cfg(feature = "openhuman")]
use super::tests_tool_call::{manifest_with_allow, tool_call_draft_args};
use super::*;

// --- output destinations (issue #981) ------------------------------------

/// [`valid_draft`] with the `output` node routed to `kind` / `target`.
pub(super) fn draft_with_destination(
    id: &str,
    name: &str,
    kind: &str,
    target: Option<&str>,
) -> RawWorkflow {
    let mut draft = valid_draft(id, name);
    let output = draft
        .nodes
        .iter_mut()
        .find(|node| node.kind == "output")
        .expect("valid_draft has an output node");
    output.destination = Some(WorkflowDestinationDef {
        kind: kind.to_string(),
        target: target.map(str::to_string),
    });
    draft
}

/// Issue #1191, the core of the fix: an unwired `channel` target is refused
/// by the shared authoring core — so EVERY caller of it is held to the rule,
/// not just the two write routes that used to run it themselves.
///
/// The refusal is a located `WorkflowInvalid`, which is the second half of
/// the defect: it used to be a bare `InvalidRequest` with no `problems`
/// array, so the console got a flat banner for exactly the class of error
/// #836 asked for a highlight on.
#[tokio::test]
async fn an_unwired_channel_target_is_refused_by_the_authoring_core() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        draft_with_destination("wf", "WF", "channel", Some("engineering-desk")),
        Some(&["engineering".to_string()]),
        None,
    )
    .await
    .expect_err("a channel nobody wired must not persist");

    let OpenCompanyError::WorkflowInvalid { problems } = &err else {
        panic!("expected a located `WorkflowInvalid`, got: {err}");
    };
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].node_id.as_deref(), Some("done"));
    assert_eq!(problems[0].field.as_deref(), Some("destination.target"));
    assert!(
        problems[0]
            .message
            .contains("is not an automation delivery channel"),
        "{:?}",
        problems[0]
    );
    // The live set rides in the sentence, so the fix is legible from the
    // refusal alone — the same message a failed delivery would have carried.
    assert!(
        problems[0].message.contains("engineering"),
        "{:?}",
        problems[0]
    );
}

/// A wired target on the same company saves. The rule refuses what delivery
/// would refuse and nothing more.
#[tokio::test]
async fn a_wired_channel_target_saves() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    create_company_workflow(
        &company,
        None,
        &store,
        None,
        draft_with_destination("wf", "WF", "channel", Some("engineering")),
        Some(&["engineering".to_string()]),
        None,
    )
    .await
    .expect("a wired channel is a destination this runtime can deliver to");
}

/// `None` is "the caller cannot see this deployment's wiring", not "nothing
/// is wired" — the same meaning `workflow_effective_tool_slugs` gives its
/// `wired` argument. The agent tool surfaces pass it, and their behaviour is
/// deliberately unchanged by #1191.
#[tokio::test]
async fn an_unseen_wiring_skips_the_channel_rule() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    create_company_workflow(
        &company,
        None,
        &store,
        None,
        draft_with_destination("wf", "WF", "channel", Some("engineering-desk")),
        None,
        None,
    )
    .await
    .expect("with no deliverable set in hand the rule is skipped, not guessed");
}

/// `Some(&[])` is a real answer, not a missing one: a company with no desk
/// and no provider channel can deliver nowhere, so every channel target is
/// refused and the sentence says so.
#[tokio::test]
async fn an_empty_deliverable_set_refuses_every_channel_target() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        draft_with_destination("wf", "WF", "channel", Some("engineering")),
        Some(&[]),
        None,
    )
    .await
    .expect_err("nowhere to deliver means no channel target is honourable");
    assert!(err.to_string().contains("no durable channels"), "{err}");
}

/// The update path runs the same rule through the same helper, so an edit
/// cannot introduce a destination a create would have refused.
#[tokio::test]
async fn update_refuses_an_unwired_channel_target_too() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    create_company_workflow(
        &company,
        None,
        &store,
        None,
        valid_draft("wf", "WF"),
        None,
        None,
    )
    .await
    .expect("the base graph has no destination at all");

    let err = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        draft_with_destination("wf", "WF", "channel", Some("engineering-desk")),
        None,
        Some(&["engineering".to_string()]),
    )
    .await
    .expect_err("an edit must be held to the create rule");
    assert!(
        matches!(err, OpenCompanyError::WorkflowInvalid { .. }),
        "{err}"
    );
}

/// The builder's courtesy pass runs the rule too (issue #1191), so a
/// proposal naming a channel nobody wired never reaches In Review — it
/// settles the card back to To-do with the reason instead.
#[cfg(feature = "openhuman")]
#[test]
fn courtesy_validation_refuses_an_unwired_channel_target() {
    let company = CompanyId::new("acme");
    let record = record(&company, manifest_with_assistant());

    let err = courtesy_validate_draft(
        &draft_with_destination("wf", "WF", "channel", Some("engineering-desk")),
        &record,
        None,
        Some(&["engineering".to_string()]),
        None,
    )
    .expect_err("the courtesy pass must refuse what apply would refuse");
    assert!(
        err.to_string()
            .contains("is not an automation delivery channel"),
        "{err}"
    );

    courtesy_validate_draft(
        &draft_with_destination("wf", "WF", "channel", Some("engineering")),
        &record,
        None,
        Some(&["engineering".to_string()]),
        None,
    )
    .expect("a wired target passes the same pass");
}

/// **Regression, issue #1882 review (PR #1882 bot finding, comment
/// 3879878907).** The courtesy pre-flight now takes the caller's stored
/// owning desk, so a caller that holds the saved body — the fix-from-run
/// copilot — gets the SAME grandfathering `update_company_workflow` applies:
/// a desk that went stale under an untouched field is carried, not refused.
///
/// RED-FIRST: pre-fix this function took no such argument and always
/// validated as a create, so the unchanged arm below was a `400`.
#[cfg(feature = "openhuman")]
#[test]
fn courtesy_validation_grandfathers_an_unchanged_stale_owner_desk() {
    let company = CompanyId::new("acme");
    let record = record(&company, manifest_with_assistant());
    let mut draft = valid_draft("wf", "WF");
    draft.owner_desk = Some("ghost-desk".to_string());

    courtesy_validate_draft(&draft, &record, None, None, Some("ghost-desk"))
        .expect("an unchanged owning desk is carried, not refused");

    // The create-shaped caller keeps the old, stricter verdict: with no
    // stored body to grandfather against, a desk naming nothing is a refusal.
    let err = courtesy_validate_draft(&draft, &record, None, None, None)
        .expect_err("a create-shaped pre-flight still refuses an unknown desk");
    let problems = problems_of(&err);
    assert_eq!(problems[0].field.as_deref(), Some("owner_desk"));

    // And grandfathering is scoped to the value that was already on file: a
    // DIFFERENT bad desk on the same edit is still refused.
    let err = courtesy_validate_draft(&draft, &record, None, None, Some("some-other-desk"))
        .expect_err("a newly named bad desk is not grandfathered by an edit");
    let problems = problems_of(&err);
    assert_eq!(problems[0].field.as_deref(), Some("owner_desk"));
}

/// Issue #1191: a `channel` destination with no `target` is refused with a
/// LOCATED problem — the node id and the config field — not a bare sentence.
///
/// The rule itself is old and lived only on the load path, where it is built
/// as a flat `String` and converted with `node_id: None, field: None`. The
/// console reads `problems` to highlight the offending node, so the one class
/// of error #836 was filed about rendered as prose it could not anchor.
#[tokio::test]
async fn a_channel_destination_with_no_target_names_the_node_and_the_field() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));

    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        draft_with_destination("wf", "WF", "channel", None),
        None,
        None,
    )
    .await
    .expect_err("a channel destination with no target must not persist");

    let OpenCompanyError::WorkflowInvalid { problems } = &err else {
        panic!("expected a located `WorkflowInvalid`, got: {err}");
    };
    let problem = problems
        .iter()
        .find(|p| p.message.contains("name the channel to post the report to"))
        .unwrap_or_else(|| panic!("no channel-target problem in {problems:?}"));
    assert_eq!(problem.node_id.as_deref(), Some("done"));
    assert_eq!(problem.field.as_deref(), Some("destination.target"));
}

/// Issue #981: an `email` destination on a company whose `[tools].allow`
/// does not grant `email` is refused at SAVE, not silently accepted and then
/// denied on every run. Granting `email` lets the same graph through, which
/// is what proves the refusal is about the grant rather than about the kind.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_email_destination_needs_the_email_grant() {
    let company = CompanyId::new("acme");
    // `web.*` grants something, just not `email` — so a bare "no grants at
    // all" is not what the refusal is keying off.
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["web.*"]),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        draft_with_destination("wf", "WF", "email", Some("ops@example.com")),
        None,
        None,
    )
    .await
    .expect_err("email destination without the grant");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("does not grant"), "{err}");
    assert!(err.to_string().contains("`done`"), "{err}");

    // Positive control: grant `email` and the same graph saves.
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["email"]),
    )));
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        draft_with_destination("wf2", "WF2", "email", Some("ops@example.com")),
        None,
        None,
    )
    .await
    .expect("email is granted");
}

/// The grant gate is scoped to `email`. An `owner` destination resolves
/// through the company's own directory and never sends to a named address,
/// so it must not be caught by the `email` rule — otherwise the fix for
/// #981 would refuse graphs that deliver perfectly well.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn an_owner_destination_is_not_gated_on_the_email_grant() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["web.*"]),
    )));
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        draft_with_destination("wf", "WF", "owner", None),
        None,
        None,
    )
    .await
    .expect("owner delivery needs no `email` grant");
}

/// The shared helper gates BOTH surfaces: an update that introduces an
/// ungranted `email` destination is refused the same way a create is.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn update_gates_email_destinations_through_the_shared_helper() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["web.*"]),
    )));
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        valid_draft("wf", "WF"),
        None,
        None,
    )
    .await
    .expect("seed create");

    let err = update_company_workflow(
        &company,
        None,
        &store,
        &revs(),
        None,
        draft_with_destination("wf", "WF", "email", Some("ops@example.com")),
        None,
        None,
    )
    .await
    .expect_err("update must gate email destinations too");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("does not grant"), "{err}");
}

/// A slug padded with leading/trailing whitespace is rejected outright rather
/// than silently trimmed: the persisted config and the run-time lookup are
/// literal, so a padded slug that "passed" a trim-normalized check would halt
/// the run on the very lookup this save-time gate promised to catch.
/// (Regression, #540.)
#[tokio::test]
async fn tool_call_with_a_whitespace_padded_slug_is_rejected() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_assistant(),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft("wf", "WF", Some(" csv_export ")),
        None,
        None,
    )
    .await
    .expect_err("padded slug");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("whitespace"), "{err}");
}

/// `media` and `composio` are agent-turn tool families the workflow invoker
/// never wires (see `WORKFLOW_TOOL_NAMESPACES`), so a `tool_call` naming one
/// would clear the run-time grant gate and then ALWAYS miss the lookup.
/// Author-time validation rejects it up front even when the namespace is
/// explicitly granted, so the save mirrors the run. (Regression, #540.)
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn tool_call_in_an_agent_turn_only_family_is_rejected() {
    let company = CompanyId::new("acme");
    // Grant BOTH families explicitly, so the rejection is about the workflow
    // surface — not a missing grant.
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["media", "composio"]),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft("wf", "WF", Some("media_generate_image")),
        None,
        None,
    )
    .await
    .expect_err("media is not a workflow tool family");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("cannot run"), "{err}");
}

// --- #813: required `config.args` on a workflow tool_call -----------------

/// A granted `tool_call` whose required `config.args` are absent is refused at
/// author time, naming the missing args — the same gate the create-time
/// copilot hears via courtesy validation. `csv_export` needs `data` and
/// `filename`; the run would otherwise export nothing.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn tool_call_missing_required_args_is_invalid() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["code"]),
    )));
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft_args("wf", "WF", Some("csv_export"), None),
        None,
        None,
    )
    .await
    .expect_err("missing required args");
    assert!(
        matches!(err, OpenCompanyError::InvalidRequest(_)),
        "{err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("config.args") && msg.contains("data") && msg.contains("filename"),
        "the missing args are named: {msg}"
    );
}

/// The same slug WITH its required args under `config.args` is accepted — the
/// arm gates the absence, not the tool. A `=`-expression counts as present.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn tool_call_with_required_args_is_accepted() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["code"]),
    )));
    let mut args = toml::map::Map::new();
    args.insert(
        "data".to_string(),
        toml::Value::String("=nodes.pick.items".to_string()),
    );
    args.insert(
        "filename".to_string(),
        toml::Value::String("out.csv".to_string()),
    );
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft_args(
            "wf",
            "WF",
            Some("csv_export"),
            Some(toml::Value::Table(args)),
        ),
        None,
        None,
    )
    .await
    .expect("required args present");
}

/// `read_workspace_state` has NO required args, so an empty-args node is not
/// blocked by the arm — its inability to read a file is a grounding concern
/// (the copilot's honest capability line), not an author-time gate.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn tool_call_with_no_required_args_is_accepted_empty() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["shell"]),
    )));
    create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft_args("wf", "WF", Some("read_workspace_state"), None),
        None,
        None,
    )
    .await
    .expect("read_workspace_state needs no args");
}

/// A required arg that is PRESENT but blank (a whitespace-only string or an
/// empty array/table) counts as missing — presence alone is not enough, since
/// a `""` filename would export to nowhere. This is the branch that carries
/// the real difference from a plain `contains_key` check.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn tool_call_with_a_blank_required_arg_is_invalid() {
    let company = CompanyId::new("acme");
    let store = store_of(MemStore::seeded(record(
        &company,
        manifest_with_allow(&["code"]),
    )));
    let mut args = toml::map::Map::new();
    // Empty array and a whitespace-only string: both present, both unusable.
    args.insert("data".to_string(), toml::Value::Array(Vec::new()));
    args.insert(
        "filename".to_string(),
        toml::Value::String("   ".to_string()),
    );
    let err = create_company_workflow(
        &company,
        None,
        &store,
        None,
        tool_call_draft_args(
            "wf",
            "WF",
            Some("csv_export"),
            Some(toml::Value::Table(args)),
        ),
        None,
        None,
    )
    .await
    .expect_err("blank required args count as missing");
    let msg = err.to_string();
    assert!(
        msg.contains("data") && msg.contains("filename"),
        "both blank args are named: {msg}"
    );
}
