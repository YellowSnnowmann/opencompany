//! Content-validation tests: skill-registry parsing, bundled-workflow
//! runnability, and per-agent output-destination resolution (split out
//! of `content_tests.rs`).

use super::content_tests_support::*;
use super::workflow_file::WorkflowNodeKind;
use super::{
    CompanyManifest, grants_chargebee_explicit, grants_composio_explicit, grants_media_explicit,
    grants_search_explicit, grants_workspace_write_explicit, load_catalog_skills, parse_workflow,
};
use crate::runtime::builder::agent_scoped_grants;

#[test]
fn the_skill_registry_parses() {
    let skills = load_catalog_skills(&repo_root().join("companies"))
        .unwrap_or_else(|err| panic!("bundle skills: {err}"));
    assert!(
        skills.iter().any(|skill| skill.slug == "web-research"),
        "expected the baseline's web-research skill in the registry"
    );
    for skill in &skills {
        assert!(!skill.name.is_empty(), "skill `{}` has no name", skill.slug);
        assert!(
            !skill.description.is_empty(),
            "skill `{}` has no description",
            skill.slug
        );
    }
}

/// Every bundled workflow must be *runnable*, not merely parseable (issue #530).
/// `every_workflow_graph_parses` only proves the TOML deserializes; it never
/// checks that a `tool_call` names a wired tool or that an `agent` names a real
/// teammate — which is exactly how the marketing agency preset shipped pointing
/// `research` at an unwired slug (halt) and `publish` at a nonexistent HTTP node.
///
/// This translates each graph the way the engine will, **compiles** it onto the
/// tinyflows engine (so a graph that parses but can't compile — an unbounded
/// guarded cycle, say — is caught at load, not first run; issue #661), and
/// asserts the two facts that decide whether a run halts: every `tool_call` slug
/// resolves to a real toolbelt namespace ([`namespace_of`]), and every `agent`
/// ref is on that company's roster.
///
/// Gated on `openhuman` because `translate` and `namespace_of` live behind that
/// feature; the `Rust (openhuman, tinymemory)` CI lane runs it.
#[cfg(feature = "openhuman")]
#[test]
fn every_bundled_workflow_is_runnable_against_its_roster() {
    use std::collections::BTreeSet;

    use tinyflows::model::NodeKind;

    use crate::harness::toolbelt::namespace_of;
    use crate::workflows::translate;

    for company in subdirs(&repo_root().join("companies")) {
        let manifest = CompanyManifest::from_path(&company)
            .unwrap_or_else(|err| panic!("{}: {err}", company.display()));
        let roster: BTreeSet<&str> = manifest.agents.iter().map(|a| a.id.as_str()).collect();

        for file in toml_files(&company.join("workflows")) {
            let text = std::fs::read_to_string(&file)
                .unwrap_or_else(|err| panic!("read {}: {err}", file.display()));
            let workflow =
                parse_workflow(&text).unwrap_or_else(|err| panic!("{}: {err}", file.display()));

            // `parse_workflow` is lenient on the #661 author-time rules (issue
            // #682) so pre-#661 tenant graphs still load. That leniency must NOT
            // apply to what WE ship: a seed a human could no longer author from
            // the console (a field-less condition, a branch not labeled yes/no, a
            // slug-less tool_call, an http_request missing method/url) has to fail
            // CI here. Run the STRICT pass over every shipped seed.
            let raw = super::raw_workflow_from_toml(&text)
                .unwrap_or_else(|err| panic!("{}: {err}", file.display()));
            let strict_problems = super::workflow_file::validate(&raw, true);
            assert!(
                strict_problems.is_empty(),
                "{}: shipped seed fails strict author-time validation (issue #661/#682): {strict_problems:?}",
                file.display()
            );

            let graph = translate(&workflow);

            // Beyond parse+translate, every seed must COMPILE onto the tinyflows
            // engine (issue #661). Compile is the pass that rejects an unbounded
            // guarded cycle (`IllegalCycle`) and other structural faults a bare
            // parse misses — a seed that parses but cannot compile would fail at
            // first run, not at load, so the whole company's workflows break.
            tinyflows::compiler::compile(&graph).unwrap_or_else(|err| {
                panic!(
                    "{}: translated graph does not compile: {err}",
                    file.display()
                )
            });

            for node in &graph.nodes {
                match node.kind {
                    NodeKind::ToolCall => {
                        let slug = node
                            .config
                            .get("slug")
                            .and_then(|v| v.as_str())
                            .unwrap_or_else(|| {
                                panic!(
                                    "{} node `{}`: a tool_call with no slug",
                                    file.display(),
                                    node.id
                                )
                            });
                        assert!(
                            namespace_of(slug).is_some(),
                            "{} node `{}`: tool_call slug `{slug}` maps to no toolbelt namespace, so \
                             the run halts on it — every tool_call must name a wired tool (shell / \
                             code / web / search / …).",
                            file.display(),
                            node.id
                        );
                        // Beyond "is it wired", the company must GRANT the slug's
                        // namespace or the run is denied at the invoke gate. Use
                        // the same search-explicit / grants_cover split the
                        // run-time invoker and the author-time create path use.
                        let namespace = namespace_of(slug).expect("asserted present just above");
                        let granted = if namespace == "search" {
                            grants_search_explicit(&manifest.tools.allow)
                        } else {
                            crate::harness::build::grants_cover(&manifest.tools.allow, namespace)
                        };
                        assert!(
                            granted,
                            "{} node `{}`: tool_call slug `{slug}` (namespace `{namespace}`) is not \
                             granted by this company's [tools].allow ({:?}) — the run is denied at \
                             the invoke gate. Grant it in `[tools].allow`.",
                            file.display(),
                            node.id,
                            manifest.tools.allow
                        );
                    }
                    NodeKind::Agent => {
                        let agent_ref = node
                            .config
                            .get("agent_ref")
                            .and_then(|v| v.as_str())
                            .unwrap_or_else(|| {
                                panic!(
                                    "{} node `{}`: an agent node with no agent_ref",
                                    file.display(),
                                    node.id
                                )
                            });
                        assert!(
                            roster.contains(agent_ref),
                            "{} node `{}`: agent_ref `{agent_ref}` is not on the roster ({roster:?}) \
                             — the step would route to a teammate that does not exist.",
                            file.display(),
                            node.id
                        );
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The marketing agency's default desktop preset specifically — the three
/// defects issue #530 fixed, pinned so a future edit cannot silently reintroduce
/// them: `research` calls the metered `web_search` and continues past a search
/// failure rather than halting, and `publish` is the copywriter assembly step
/// (there is no CMS to POST to).
#[cfg(feature = "openhuman")]
#[test]
fn the_marketing_campaign_preset_is_runnable() {
    use crate::workflows::translate;

    let path = repo_root().join("companies/marketing_agency/workflows/campaign_pipeline.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let graph = translate(&parse_workflow(&text).expect("campaign parses"));
    let node = |id: &str| {
        graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap_or_else(|| panic!("no node `{id}`"))
    };

    let research = node("research");
    assert_eq!(research.config["slug"], "web_search");
    assert_eq!(research.config["on_error"], "continue");

    assert_eq!(node("publish").config["agent_ref"], "copywriter");
}

/// The marketing agency's creative desk ceiling must not strip the company's
/// workspace-write grant.
///
/// The desk states `["*", "workspace.write"]`, and the `*` half deliberately
/// confers no workspace writes — [`grants_workspace_write_explicit`] matches
/// only the bare `workspace` or the exact `workspace.write` token — so the
/// write token has to be restated in the ceiling for the desk's agents to hold
/// it. Their own AGENTS.md promises the `agents/<id>/` folder is always
/// writable, and `agent_scoped_grants` would silently strip that promise with
/// a `["*"]`-only ceiling. Pinned through the *three-level* narrowing (the
/// `effective_grants` the search-posture tests use ignores desks, which is
/// precisely how this gap shipped) so a future edit cannot quietly reintroduce
/// the stripping.
#[test]
fn a_restricting_desk_does_not_strip_the_workspace_write_token() {
    let manifest = load_company("marketing_agency");
    let creative = manifest
        .group_chats
        .iter()
        .find(|chat| chat.id == "creative")
        .expect("the marketing agency declares the creative desk");
    assert!(
        !creative.tools.is_empty(),
        "the creative desk must state a ceiling or this test asserts nothing"
    );

    for id in ["creative_director", "copywriter", "landing_page_builder"] {
        let agent = manifest
            .agents
            .iter()
            .find(|agent| agent.id == id)
            .unwrap_or_else(|| panic!("{id} is a member of the creative desk"));
        let desk_refs: Vec<&[String]> = manifest
            .group_chats
            .iter()
            .filter(|chat| chat.members.iter().any(|member| member == id))
            .map(|chat| chat.tools.as_slice())
            .collect();
        let grants = agent_scoped_grants(&manifest.tools.allow, &desk_refs, agent.tools.as_deref());

        assert!(
            grants_workspace_write_explicit(&grants),
            "{id}: the creative desk ceiling ({:?}) must keep the company's \
             workspace write grant; effective grants: {grants:?}",
            creative.tools
        );
        // The desk still deliberately withholds the billed / third-party
        // opt-ins the company grants at the top level.
        assert!(
            !grants_search_explicit(&grants),
            "{id}: the creative desk must stay searchless; effective grants: {grants:?}"
        );
        assert!(
            !grants_media_explicit(&grants),
            "{id}: the creative desk must stay media-less; effective grants: {grants:?}"
        );
        assert!(
            !grants_composio_explicit(&grants),
            "{id}: the creative desk must stay composio-less; effective grants: {grants:?}"
        );
    }
}

/// The marketing agency's `chargebee` exclusion lives at the per-agent layer,
/// not on the strategy/growth desk ceilings, so the console flow the manifest
/// documents — naming a biller from the console — actually works.
///
/// A desk ceiling is manifest-only and cannot be widened from the console, so
/// an exclusion stated there would make the billing grant unreachable by any
/// shipped teammate. Pinned through the real three-level narrowing
/// (`agent_scoped_grants`): a shipped member's own `tools` line excludes
/// `chargebee`, while the desk level (no ceiling) admits an operator override
/// that names it.
#[test]
fn a_marketing_biller_can_be_named_from_the_console() {
    let manifest = load_company("marketing_agency");

    // The desks this PR touched state no ceiling — the exclusion must not live
    // on an unwidenable layer.
    for id in ["strategy", "growth"] {
        let desk = manifest
            .group_chats
            .iter()
            .find(|chat| chat.id == id)
            .unwrap_or_else(|| panic!("{id} desk"));
        assert!(
            desk.tools.is_empty(),
            "{id}: the `chargebee` exclusion must not live on the desk ceiling \
             (an unwidenable layer); found {:?}",
            desk.tools
        );
    }

    // Every shipped strategy/growth member holds the belt minus `chargebee`.
    for id in [
        "brand_strategist",
        "seo_specialist",
        "analytics_analyst",
        "paid_ads_manager",
        "email_marketer",
    ] {
        let agent = manifest
            .agents
            .iter()
            .find(|agent| agent.id == id)
            .unwrap_or_else(|| panic!("{id} is a marketing teammate"));
        let desk_refs: Vec<&[String]> = manifest
            .group_chats
            .iter()
            .filter(|chat| chat.members.iter().any(|member| member == id))
            .map(|chat| chat.tools.as_slice())
            .collect();
        let grants = agent_scoped_grants(&manifest.tools.allow, &desk_refs, agent.tools.as_deref());
        assert!(
            !grants_chargebee_explicit(&grants),
            "{id}: a shipped marketing teammate must not hold billing tools; \
             effective grants: {grants:?}"
        );
    }

    // An operator naming the biller from the console — adding `chargebee` to a
    // strategy member's override — now survives the desk level.
    let brand = manifest
        .agents
        .iter()
        .find(|agent| agent.id == "brand_strategist")
        .unwrap();
    let mut override_tools = brand.tools.clone().unwrap_or_default();
    override_tools.push("chargebee".to_string());
    let strategy = manifest
        .group_chats
        .iter()
        .find(|chat| chat.id == "strategy")
        .unwrap();
    let desk_refs: Vec<&[String]> = vec![strategy.tools.as_slice()];
    let grants = agent_scoped_grants(&manifest.tools.allow, &desk_refs, Some(&override_tools));
    assert!(
        grants_chargebee_explicit(&grants),
        "the console override naming the biller must survive the desk layer; \
         effective grants: {grants:?}"
    );
}

/// The software company ships the billing ceiling and nobody holding it.
///
/// The template had no `chargebee` at all until #1854, which made the
/// capability unreachable rather than withheld: `[tools].allow` has no runtime
/// write path, so a company booted from this bundle could store a Chargebee key
/// and have it reach no teammate, with nothing in the console able to fix it.
///
/// Granting it needed every agent to state a belt first. All nine omitted their
/// `tools` line, and an omitted line inherits the WHOLE company grant — so the
/// one-line version of this change would have handed billing to the QA engineer
/// and the docs writer rather than to nobody. This pins both halves: the
/// ceiling exists, and no shipped teammate resolves to holding it.
#[test]
fn the_software_company_ships_billing_that_reaches_nobody_yet() {
    let manifest = load_company("software_company");

    assert!(
        grants_chargebee_explicit(&manifest.tools.allow),
        "the ceiling must exist, or an operator has no way to name a biller: {:?}",
        manifest.tools.allow
    );

    // The exclusion must not live on a desk: desk ceilings are manifest-only,
    // so an exclusion there could never be widened from the console — which
    // would make the ceiling above decorative.
    for chat in &manifest.group_chats {
        assert!(
            chat.tools.is_empty(),
            "{}: the `chargebee` exclusion must not live on the desk ceiling              (an unwidenable layer); found {:?}",
            chat.id,
            chat.tools
        );
    }

    for agent in &manifest.agents {
        let desk_refs: Vec<&[String]> = manifest
            .group_chats
            .iter()
            .filter(|chat| chat.members.contains(&agent.id))
            .map(|chat| chat.tools.as_slice())
            .collect();
        let grants = agent_scoped_grants(&manifest.tools.allow, &desk_refs, agent.tools.as_deref());
        assert!(
            !grants_chargebee_explicit(&grants),
            "{}: a shipped teammate must not hold billing tools; effective              grants: {grants:?}",
            agent.id
        );
    }

    // …and naming one from the console reaches the tools, which is the whole
    // point of the ceiling being there.
    let support = manifest
        .agents
        .iter()
        .find(|agent| agent.id == "customer_support")
        .expect("customer_support is on this roster");
    let mut named = support.tools.clone().unwrap_or_default();
    named.push("chargebee".to_string());
    let desk_refs: Vec<&[String]> = manifest
        .group_chats
        .iter()
        .filter(|chat| {
            chat.members
                .iter()
                .any(|member| member == "customer_support")
        })
        .map(|chat| chat.tools.as_slice())
        .collect();
    let grants = agent_scoped_grants(&manifest.tools.allow, &desk_refs, Some(&named));
    assert!(
        grants_chargebee_explicit(&grants),
        "an operator naming the biller from the console must reach billing; \
         effective grants: {grants:?}"
    );
}

/// A creative member cross-seated onto an unrestricted desk must not widen to
/// the company grant.
///
/// Desks combine by **union**, and a member scoped only by the creative desk
/// ceiling would resolve to the full company grant — billing included — the
/// moment an operator seats them on the strategy or growth desk, which state
/// no ceiling. The `chargebee` exclusion must therefore ride on the member's
/// own `tools` line (the company belt minus `chargebee`), not on the desk
/// alone. Pinned through the same three-level narrowing the roster build uses.
#[test]
fn a_creative_member_cross_seated_on_an_unrestricted_desk_stays_billing_less() {
    let manifest = load_company("marketing_agency");
    let strategy = manifest
        .group_chats
        .iter()
        .find(|chat| chat.id == "strategy")
        .unwrap();
    assert!(
        strategy.tools.is_empty(),
        "precondition: the strategy desk must be unrestricted or this test \
         proves nothing"
    );
    let creative = manifest
        .group_chats
        .iter()
        .find(|chat| chat.id == "creative")
        .unwrap();

    for id in ["creative_director", "copywriter", "landing_page_builder"] {
        let agent = manifest
            .agents
            .iter()
            .find(|agent| agent.id == id)
            .unwrap_or_else(|| panic!("{id} is a member of the creative desk"));
        assert!(
            agent.tools.as_deref().is_some_and(|t| !t.is_empty()),
            "{id}: the `chargebee` exclusion must ride on the member's own \
             `tools` line, not only on the creative desk ceiling"
        );

        // Seated on the creative desk AND the unrestricted strategy desk: the
        // union would otherwise be the company grant.
        let desk_refs: Vec<&[String]> = vec![creative.tools.as_slice(), strategy.tools.as_slice()];
        let grants = agent_scoped_grants(&manifest.tools.allow, &desk_refs, agent.tools.as_deref());
        assert!(
            !grants_chargebee_explicit(&grants),
            "{id}: cross-seating a creative member onto an unrestricted desk \
             must not hand back billing; effective grants: {grants:?}"
        );
    }
}

/// Every seeded `output` node names a destination its own manifest can resolve,
/// except the research lab, which deliberately proves that workflows can
/// coordinate without desks (issue #963).
///
/// Two failures this catches, and they are opposite:
///
///  1. **A destination that cannot resolve.** A `channel` target is a
///     [`ChannelAdapter`] id, and the adapters a company gets are one per desk in
///     its own manifest (`runtime::builder`, issue #837). A target naming a desk
///     that manifest does not declare parses fine, ships, and fails at run time
///     on a freshly provisioned tenant — which is the class of bug #947 is about,
///     one step further along.
///  2. **A template that quietly loses its destination.** All shipped
///     templates except the research lab now declare a desk for their terminal
///     output. The single exception is named below so removing any other
///     destination fails rather than passing as "well, some have none".
///
/// `research_lab` explains in its own manifest why it has no desk: its
/// workflow is the proving ground for collapsing desk coordination into the
/// graph itself.
#[test]
fn every_seeded_output_destination_resolves_against_its_own_manifest() {
    const DESKLESS_WORKFLOW_TEMPLATE: &str = "research_lab";

    let mut checked = 0;
    let mut with_destination = 0;
    for dir in subdirs(&repo_root().join("companies")) {
        let company = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .to_string();
        let manifest_path = dir.join("company.toml");
        if !manifest_path.exists() {
            continue;
        }
        let manifest: CompanyManifest =
            toml::from_str(&std::fs::read_to_string(&manifest_path).unwrap())
                .unwrap_or_else(|err| panic!("{company}/company.toml: {err}"));
        let desks: Vec<&str> = manifest
            .group_chats
            .iter()
            .map(|chat| chat.id.as_str())
            .collect();

        for path in toml_files(&dir.join("workflows")) {
            let text = std::fs::read_to_string(&path).unwrap();
            let file =
                parse_workflow(&text).unwrap_or_else(|err| panic!("{}: {err:?}", path.display()));
            let label = format!("{company}/{}", path.file_name().unwrap().to_string_lossy());

            for node in file
                .nodes
                .iter()
                .filter(|n| n.kind == WorkflowNodeKind::Output)
            {
                checked += 1;
                let Some(destination) = node.destination.as_ref() else {
                    assert!(
                        company == DESKLESS_WORKFLOW_TEMPLATE,
                        "{label} has an output node with no destination, so a run of it \
                         delivers nothing. Give it a channel destination backed by a \
                         manifest desk. The research lab is the only intentional exception."
                    );
                    assert!(
                        desks.is_empty(),
                        "{label} is the deskless-workflow exception but its manifest \
                         declares desks {desks:?}. Give its output node a destination."
                    );
                    continue;
                };
                with_destination += 1;
                assert_eq!(
                    destination.kind, "channel",
                    "{label} uses destination kind `{}`. A seeded template routes to a real \
                     desk channel: `owner` on a no-mailbox tenant now lands on the operator \
                     channel (issue #1757) rather than the desk a template means to post in, \
                     and `email` would hardcode a recipient into a shipped template.",
                    destination.kind
                );
                let target = destination.target.as_deref().unwrap_or("");
                assert!(
                    desks.contains(&target),
                    "{label} delivers to channel `{target}`, which is not a desk \
                     {company}'s manifest declares. A company's channel adapters are one \
                     per desk, so this resolves nowhere at run time. Declared: {desks:?}"
                );
            }
        }
    }

    // The relation, not a hand-maintained total. #963's count was a literal (22
    // by the time `e2e_harness/long_pipeline.toml` landed), which meant every
    // added workflow failed this test on arithmetic rather than on anything
    // about destinations — and the fix was always to bump the number, which is
    // a guard nobody reads. What the count was actually protecting is stated
    // directly instead: **exactly one** seeded output node in the whole
    // repository has no destination, and it is the research lab's.
    assert!(
        checked > 0,
        "no seeded output nodes were checked at all — the walk found nothing"
    );
    assert_eq!(
        with_destination,
        checked - 1,
        "every seeded output except the research lab's deliberate deskless workflow \
         carries a destination"
    );
}
