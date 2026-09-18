use super::consequence_hosting_tests::*;
use super::*;
use serde_json::json;

/// The two halves of #457's scoping, exercised **together and directly**
/// (issue #610).
///
/// [`standing_scope_of`] mints the scope and
/// [`StandingGrant::admits_scope`] spends it, and since #559 no tier routes
/// a Composio read through both — see the retention note on
/// `standing_scope_of`. Each half is pinned on its own elsewhere, and each
/// of those tests spells the toolkit as its own `"github"` literal. Two
/// literals in two files are not an agreement: change what
/// `standing_scope_of` returns and both suites can be made green
/// separately while the pairing they describe is broken, with no live
/// caller left to notice.
///
/// So nothing here is written down. Every scope comes out of
/// `standing_scope_of` and goes straight into a grant or into
/// `admits_scope`, which makes this a test of whether the two functions
/// still agree rather than of what either one says.
#[test]
#[cfg(feature = "openhuman")]
pub(super) fn the_minted_scope_is_the_scope_a_grant_admits() {
    use crate::runtime::grants::{GrantId, StandingGrant};

    let scope_of = |slug: &str| standing_scope_of(COMPOSIO_EXECUTE, &json!({ "tool": slug }));

    let minted = scope_of("GITHUB_LIST_BRANCHES");
    assert!(
        minted.is_some(),
        "a catalogued action must resolve a toolkit, or this test proves nothing"
    );
    // Minted the way the cycle mints one: from the parked effect's payload.
    let grant = StandingGrant {
        id: GrantId::new("g610"),
        agent: "ops".to_string(),
        workflow: None,
        tool: COMPOSIO_EXECUTE.to_string(),
        verdict: crate::ports::types::Verdict::Approve,
        granted_by: crate::ports::types::Actor {
            kind: crate::ports::types::ActorKind::User,
            id: "user-1".to_string(),
        },
        approval_id: crate::ports::types::ApprovalId::new("a610"),
        at_millis: 1_000,
        expires_at_millis: u64::MAX,
        origin_thread: None,
        origin_parent: None,
        origin_task: None,
        scope: minted,
    };

    // A second read from the provider the operator named. Scoped by
    // toolkit and not by slug, so a *different* GitHub action passes.
    assert!(
        grant.admits_scope(scope_of("GITHUB_LIST_PULL_REQUESTS").as_deref()),
        "the operator consented to a provider, not to one action slug"
    );
    // Another provider's read: every other dimension matches and the scope
    // is the one thing that says no.
    assert!(
        !grant.admits_scope(scope_of("GMAIL_FETCH_EMAILS").as_deref()),
        "'read from GitHub' is not consent to read the company's mail"
    );
    // An action the catalogue cannot place resolves to `None`, and a scoped
    // grant refuses `None` rather than guessing permissively.
    assert_eq!(
        scope_of("NOT_A_REAL_TOOLKIT_DO_SOMETHING"),
        None,
        "an unplaceable action must not resolve a toolkit"
    );
    assert!(
        !grant.admits_scope(scope_of("NOT_A_REAL_TOOLKIT_DO_SOMETHING").as_deref()),
        "unknown is a send here too"
    );
    // And the unscoped grant a pre-#457 journal line replays into still
    // admits whatever its `(agent, tool)` pair already admitted.
    let unscoped = StandingGrant {
        scope: None,
        ..grant
    };
    assert!(
        unscoped.admits_scope(scope_of("GMAIL_FETCH_EMAILS").as_deref()),
        "an unscoped grant must keep behaving as it did before scopes existed"
    );
}

/// Issue #457: a standing grant on `composio_execute` has to record *which
/// provider*, because the card said "read from GitHub" and the tool name
/// says nothing at all.
#[test]
#[cfg(feature = "openhuman")]
pub(super) fn a_composio_call_is_scoped_to_its_toolkit() {
    assert_eq!(
        standing_scope_of(COMPOSIO_EXECUTE, &json!({ "tool": "GITHUB_LIST_BRANCHES" })),
        Some("github".to_string())
    );
    // A different action in the same toolkit is the same scope — the
    // operator agreed to a provider, not to one slug.
    assert_eq!(
        standing_scope_of(
            COMPOSIO_EXECUTE,
            &json!({ "tool": "GITHUB_LIST_PULL_REQUESTS" })
        ),
        Some("github".to_string())
    );
    // A different provider is a different scope, which is the whole point.
    assert_eq!(
        standing_scope_of(COMPOSIO_EXECUTE, &json!({ "tool": "GMAIL_FETCH_EMAILS" })),
        Some("gmail".to_string())
    );
    // The tool name is matched the same lowercasing way the rest of the
    // gate matches it.
    assert_eq!(
        standing_scope_of(
            "COMPOSIO_EXECUTE",
            &json!({ "tool": "github_list_branches" })
        ),
        Some("github".to_string())
    );
}

/// Nothing the catalogue can place is `None`, and `None` is what a scoped
/// grant refuses — so an unplaceable slug can never ride somebody else's
/// permission.
///
/// Unchanged by issue #1818, and worth saying why: the verb fallback moved
/// `..._LIST_...` off the parking path, but it did not give it a scope. A
/// grant is minted against a *toolkit*, and a toolkit the catalogue has
/// never heard of is still not one an operator consented to.
#[test]
pub(super) fn an_unplaceable_composio_call_has_no_scope() {
    for args in [
        json!({ "tool": "NOTAREALTOOLKIT_LIST_THINGS" }),
        json!({ "tool": "" }),
        json!({ "tool": 7 }),
        json!({ "arguments": { "owner": "acme" } }),
        json!({}),
    ] {
        assert_eq!(
            standing_scope_of(COMPOSIO_EXECUTE, &args),
            None,
            "nothing to narrow to: {args}"
        );
    }
}

/// Every other tool has no scope, and must not grow one by accident: the
/// name of `file_write` already is the whole of what it can do.
#[test]
pub(super) fn a_tool_whose_name_says_everything_has_no_scope() {
    for tool in [
        "file_write",
        "memory_forget",
        "memory_store",
        "shell",
        "workspace_write",
        "workspace_create",
    ] {
        assert_eq!(
            standing_scope_of(tool, &json!({ "tool": "GITHUB_LIST_BRANCHES" })),
            None,
            "`{tool}` is not a Composio call whatever its arguments say"
        );
    }
}

/// The other side of the seam. A default build cannot mint a Composio
/// standing grant at all (see
/// `without_the_catalogue_every_composio_action_is_a_send`), so answering
/// "no scope" here widens nothing — there is no scoped grant to widen.
#[test]
#[cfg(not(feature = "openhuman"))]
pub(super) fn without_the_catalogue_nothing_carries_a_scope() {
    for slug in ["GITHUB_LIST_BRANCHES", "GMAIL_SEND_EMAIL"] {
        assert_eq!(
            standing_scope_of(COMPOSIO_EXECUTE, &json!({ "tool": slug })),
            None,
            "{slug}"
        );
    }
}

/// The literals above and the constants the tools themselves return are two
/// copies of the same string. This is the test that keeps them one.
#[test]
#[cfg(feature = "openhuman")]
pub(super) fn the_declared_names_are_the_names_the_tools_return() {
    use crate::harness::{orchestrator, publish, search, workflow_admin, workspace_tools};
    for name in [
        workflow_admin::READ_WORKFLOW_TOOL,
        workflow_admin::UPDATE_WORKFLOW_TOOL,
        workflow_admin::DELETE_WORKFLOW_TOOL,
        orchestrator::QUERY_COMPANY_TOOL,
        orchestrator::SPAWN_TASK_TOOL,
        orchestrator::DELEGATE_TO_DESK_TOOL,
        orchestrator::DELEGATE_TO_TEAMMATE_TOOL,
        orchestrator::ADD_AGENT_TOOL,
        orchestrator::CREATE_WORKFLOW_TOOL,
        orchestrator::ASSIGN_TASK_TOOL,
        orchestrator::REVIEW_TASK_TOOL,
        orchestrator::RUN_WORKFLOW_TOOL,
        publish::PUBLISH_ARTIFACT_TOOL,
        search::WEB_SEARCH_TOOL,
        workspace_tools::WORKSPACE_LIST_TOOL,
        workspace_tools::WORKSPACE_READ_TOOL,
        workspace_tools::WORKSPACE_SEARCH_TOOL,
        workspace_tools::WORKSPACE_CREATE_TOOL,
        workspace_tools::WORKSPACE_WRITE_TOOL,
        workspace_tools::WORKSPACE_RENAME_TOOL,
        workspace_tools::WORKSPACE_DELETE_TOOL,
        crate::harness::composio_catalog::LIST_TOOLS_TOOL,
        crate::harness::composio_catalog::LIST_TOOLKITS_TOOL,
    ] {
        assert!(
            DECLARED.iter().any(|d| d.tool == name),
            "`{name}` is a live tool constant with no declaration"
        );
    }
}

/// Where the console keeps the words an operator reads instead of a tool
/// name. Named once so both the parser and every failure message point at
/// the same file.
pub(super) const LANGUAGE_TS: &str = "frontend/src/lib/language.ts";

/// One object literal in [`LANGUAGE_TS`], read as `key -> sentence`.
///
/// A line parser rather than the two alternatives, and the reasons are the
/// same ones that make this a `cargo test` at all:
///
/// * a **checked-in generated manifest** of the declared set would give the
///   contract two failure sites and a window between the declaration commit
///   and the regenerate commit where nothing is wrong;
/// * a **CI grep** would have no local signal for the Rust contributor who
///   adds the next `Reach::Consequence` line — and that is who introduced
///   all three instances of this defect (#372, #551 → #671, now #701).
///
/// It is deliberately literal about the shape it accepts: an object literal
/// opened by `const <NAME>` on a line ending in `{` and closed by a `};`
/// line. Anything else panics rather than returning a short list, because a
/// parser that silently reads nothing turns this test into a green light
/// for the exact regression it exists to catch — see the vacuity guards in
/// [`every_consequence_tool_has_a_console_label`].
///
/// It returns pairs rather than keys (issue #743) because the distinctness
/// half needs the sentences, and one parse feeding both halves is the point:
/// this test exists because a hand-maintained restatement of the declared
/// set drifts from it, and a second parser over the same file would be that
/// same mistake one level down.
///
/// Values are taken literally — the text between the first `:` and the
/// trailing comma, unquoted. Every entry in both tables is a plain string
/// literal on one line today; a template literal or a concatenation would
/// arrive here as its own source text and, being unequal to any other
/// entry, would pass the distinctness check without asserting anything about
/// what an operator reads. That is the one shape to reject rather than
/// tolerate, and the `>=` floors below are what would catch a table that
/// reshaped into it wholesale.
pub(super) fn label_pairs(source: &str, decl: &str) -> Vec<(String, String)> {
    let mut lines = source.lines();
    let opened = lines.any(|line| {
        let line = line.trim_start();
        line.starts_with(&format!("const {decl}")) && line.ends_with('{')
    });
    assert!(
        opened,
        "no `const {decl} … {{` line in {LANGUAGE_TS}. If the table was \
         renamed or reshaped, update this parser — do not delete the test"
    );

    let mut pairs = Vec::new();
    for line in lines {
        let line = line.trim();
        if line == "};" {
            return pairs;
        }
        if line.is_empty() || line.starts_with("//") || line.starts_with('*') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        pairs.push((
            key.trim().trim_matches('"').to_string(),
            value
                .trim()
                .trim_end_matches(',')
                .trim()
                .trim_matches('"')
                .to_string(),
        ));
    }
    panic!("`const {decl}` in {LANGUAGE_TS} is never closed by a `}};` line");
}

/// Every tool that reaches an operator resolves to a sentence, not to
/// "Use one of its tools" (issue #701).
///
/// The console's `approvalAction` resolves `EFFECT_LABELS` → `TOOL_LABELS`
/// → a generic fallback, so a gated tool in neither table asks an operator
/// to consent to "use one of its tools" — the #372 defect. It has now
/// recurred three times, and every time the commit that caused it was a
/// Rust one adding a `Reach::Consequence` declaration with no reason to
/// open the frontend at all. So the coupling belongs here, next to
/// [`DECLARED`], where that contributor's `cargo test` reports it.
///
/// Scoped to the whole [`Reach::Consequence`] class rather than to the
/// per-call subset. The grantable ones (`file_write`, `edit`,
/// `apply_patch`, `csv_export`) park exactly the same way, and they are
/// *also* the ones issue #374's Standing-permissions list renders through
/// `toolAction` with no payload block to disambiguate them. A test scoped
/// to `Standing::PerCall` would ship blind to four instances of the class
/// it exists to kill.
#[test]
pub(super) fn every_consequence_tool_has_a_console_label() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../frontend/src/lib/language.ts"
    ));
    // Read as pairs, not keys: the distinctness half below needs the
    // sentences. `label_keys` is the same parse, so the two halves cannot
    // disagree about what the tables hold.
    let effect_labels = label_pairs(source, "EFFECT_LABELS");
    let tool_labels = label_pairs(source, "TOOL_LABELS");
    let effects: Vec<&str> = effect_labels.iter().map(|(k, _)| k.as_str()).collect();
    let tools: Vec<&str> = tool_labels.iter().map(|(k, _)| k.as_str()).collect();

    // Vacuity guards. A parse that quietly returned nothing would report
    // every gated tool as unlabelled — noisy, and therefore self-correcting.
    // A parse that quietly returned *the wrong block* would report none of
    // them, which is the failure that matters: the test would pass forever
    // while the console regressed. These anchors are entries with no reason
    // to move, so a reformat of `language.ts` breaks here loudly instead.
    for anchor in ["payment.send", "workflow.approve"] {
        assert!(
            effects.contains(&anchor),
            "parsed EFFECT_LABELS from {LANGUAGE_TS} without `{anchor}` — \
             the parser is reading the wrong block, not the table shrinking"
        );
    }
    for anchor in ["shell", "workspace_create"] {
        assert!(
            tools.contains(&anchor),
            "parsed TOOL_LABELS from {LANGUAGE_TS} without `{anchor}` — \
             the parser is reading the wrong block, not the table shrinking"
        );
    }
    assert!(
        effects.len() >= 15 && tools.len() >= 10,
        "parsed only {} EFFECT_LABELS and {} TOOL_LABELS keys from \
         {LANGUAGE_TS}; both tables are larger than that, so the parser is \
         stopping early",
        effects.len(),
        tools.len()
    );

    let gated: Vec<&str> = declared_tools()
        .filter(|tool| c(tool).reach.parks_under_supervision())
        .collect();

    // The walk's own vacuity guard (issue #743). A distinctness check over
    // an empty or truncated set passes having asserted nothing, which is
    // precisely the fail-open shape the guards above exist to refuse — and
    // the shape that made #706's reproduction wrong by half.
    //
    // A floor rather than an exact count, matching the `>=` idiom above: the
    // gated set is 25 today and grows whenever a `Reach::Consequence` line
    // is declared, so pinning it exactly would fail every such commit for
    // being correct. What must never happen is the walk *shrinking* toward
    // the four hardcoded names this widened.
    assert!(
        gated.len() >= 20,
        "only {} tools were selected as gated; the declaration table holds \
         far more `Reach::Consequence` entries than that, so the walk is \
         selecting almost nothing and everything below it is vacuous",
        gated.len()
    );

    let mut unlabelled: Vec<&str> = gated
        .iter()
        .copied()
        .filter(|tool| !effects.iter().any(|k| k == tool) && !tools.iter().any(|k| k == tool))
        .collect();
    unlabelled.sort_unstable();
    assert!(
        unlabelled.is_empty(),
        "{unlabelled:?} park for an operator but have no entry in either \
         label map in {LANGUAGE_TS}, so their approval card reads \"Use one \
         of its tools\". Add each to TOOL_LABELS — a gated tool's label \
         only ever appears above the payload block, which is what \
         EFFECT_LABELS entries do not assume and why its \
         EFFECT_DONE_LABELS mirror would demand a past-tense twin these \
         kinds never reach"
    );

    // ...and no two of them read the same sentence (issue #743).
    //
    // Checked after the unlabelled walk on purpose: an unlabelled pair would
    // collide here too, on the fallback, and reporting that as "these two
    // read alike" would name the symptom while the assertion above names the
    // cause. Ordering the two is what keeps one failure message honest.
    //
    // Resolved through the console's own rung order — `EFFECT_LABELS` then
    // `TOOL_LABELS` — because that is what `toolAction` does, and a tool
    // present in both resolves to the effect sentence. Comparing the tables
    // separately would miss exactly the collision that ordering creates.
    let sentence = |tool: &str| -> &str {
        effect_labels
            .iter()
            .chain(tool_labels.iter())
            .find(|(key, _)| key == tool)
            .map(|(_, value)| value.as_str())
            .expect("every gated tool is labelled — asserted directly above")
    };

    let mut by_sentence: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for tool in &gated {
        by_sentence.entry(sentence(tool)).or_default().push(tool);
    }
    let collisions: Vec<String> = by_sentence
        .iter()
        .filter(|(_, sharing)| sharing.len() > 1)
        .map(|(reads, sharing)| format!("{sharing:?} all read {reads:?}"))
        .collect();
    assert!(
        collisions.is_empty(),
        "two gated tools render the same sentence: {}. The Standing \
         permissions list (#374) puts no payload block under a row, so two \
         rows reading alike are two permissions an operator cannot choose \
         between — and on an approval card the payload only disambiguates \
         them if they happen to carry different arguments. Give each its own \
         words in {LANGUAGE_TS}",
        collisions.join("; ")
    );
}
