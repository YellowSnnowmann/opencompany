//! Setup focus fixtures and grant narrowing: job-item coverage, recognising
//! a reference-team roster, and that an unrecognised or tampered focus can
//! never grant more than a recognised one.

use super::*;

fn answers(industry: &str, automate: &str) -> SetupAnswers {
    SetupAnswers {
        industry: industry.to_string(),
        team_hint: String::new(),
        automate: automate.to_string(),
    }
}
fn proposed(role: &str) -> ProposedAgent {
    ProposedAgent {
        name: role.split_whitespace().next().unwrap_or(role).to_string(),
        role: role.to_string(),
        description: format!("Owns {}.", role.to_lowercase()),
        focus: None,
    }
}

// ---------------------------------------------------------------------
// The job checklist coverage is judged against
// ---------------------------------------------------------------------

/// The splitting rule, from the fixture the console's test reads too.
///
/// The fixture is the whole mitigation for having two implementations of one
/// rule: the console echoes the items live while someone types, and the host
/// numbers them for the prompt. The first version of this feature shipped a
/// hand-copied keyword list in the browser and it drifted within a week.
#[test]
fn job_items_matches_the_shared_fixture() {
    #[derive(serde::Deserialize)]
    struct Case {
        why: String,
        input: String,
        items: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct Fixture {
        #[serde(rename = "maxJobs")]
        max_jobs: usize,
        cases: Vec<Case>,
    }

    let raw = include_str!("../../tests/fixtures/setup-jobs.json");
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert_eq!(
        fixture.max_jobs, MAX_JOBS,
        "the fixture and the host disagree about the cap"
    );
    assert!(
        !fixture.cases.is_empty(),
        "an empty fixture asserts nothing"
    );
    for case in fixture.cases {
        assert_eq!(job_items(&case.input), case.items, "{}", case.why);
    }
}

/// Coverage is set maths over the host's list, not a sentence from the
/// model. An index that names nothing covers nothing.
#[test]
fn an_out_of_range_claim_covers_nothing() {
    let jobs = job_items("ads, dispatch, invoices");
    assert_eq!(
        uncovered_jobs(&jobs, &[0, 99]),
        vec!["dispatch", "invoices"]
    );
    assert!(uncovered_jobs(&jobs, &[0, 1, 2]).is_empty());
    assert_eq!(uncovered_jobs(&jobs, &[]), jobs);
}

/// A curated team was chosen by keyword and never read the list, so it
/// reports its provenance rather than a coverage claim it cannot make.
#[test]
fn the_fallback_echoes_the_jobs_but_claims_no_coverage() {
    let proposal = template_proposal(
        &answers("I sell homeware online", "Meta ads, order dispatch"),
        FallbackReason::NoModel,
    );
    assert_eq!(proposal.jobs, vec!["Meta ads", "order dispatch"]);
    assert!(
        proposal.uncovered.is_empty(),
        "a fallback must not claim a gap it never looked for"
    );
}

// ---------------------------------------------------------------------
// Refusing to call a copy an original
// ---------------------------------------------------------------------

/// The degenerate answer the reference team invites: hand the whole thing
/// back. Nothing about its *shape* is wrong, so validation admits it — and
/// the operator would then be told "built from what you told us" about a
/// roster nobody designed.
#[test]
fn a_roster_that_is_only_the_reference_team_is_recognised() {
    assert!(is_entirely_reference_team(
        &ECOMMERCE.proposed(),
        &ECOMMERCE
    ));
    // Re-spacing and re-casing are not authorship.
    let restyled: Vec<ProposedAgent> = ECOMMERCE
        .agents
        .iter()
        .map(|a| ProposedAgent {
            name: a.name.to_string(),
            role: a.role.to_uppercase().replace(' ', "  "),
            description: a.description.to_string(),
            focus: Some(a.focus),
        })
        .collect();
    assert!(is_entirely_reference_team(&restyled, &ECOMMERCE));
}

/// It must not fire on a designed line-up. One added role is a decision the
/// model made, and this guard exists to protect the provenance claim — not
/// to police how much of the reference wording survived.
#[test]
fn one_role_of_its_own_is_enough_to_be_a_designed_team() {
    let mut roster = ECOMMERCE.proposed();
    roster.push(proposed("Cold Email Specialist"));
    assert!(!is_entirely_reference_team(&roster, &ECOMMERCE));

    // The real case this was checked against: three template roles and
    // three of the model's own is a designed team.
    let mixed = vec![
        proposed("SEO Specialist"),
        proposed("Logistics Coordinator"),
        proposed("Accountant"),
        proposed("Cold Email Specialist"),
        proposed("Product Researcher"),
        proposed("Social Media Manager"),
    ];
    assert!(!is_entirely_reference_team(&mixed, &ECOMMERCE));
}

/// An empty roster is not a copy of anything. Reported as false so the
/// caller's own too-thin check stays the thing that handles it — two rules
/// competing over one case is how the padding bug happened.
#[test]
fn an_empty_roster_is_not_a_copy() {
    assert!(!is_entirely_reference_team(&[], &ECOMMERCE));
}

/// The hole a prompt-injection test found: an **invalid** focus used to
/// produce a wider agent than any valid one, because an empty `tools` list is
/// read as "inherit the company belt".
///
/// Still the invariant after the belts were widened, and still the reason
/// the fallback is a real focus rather than an empty list. What the unknown
/// case may now hold is the base belt plus workspace writes — what it may
/// never hold is the catch-all, or any namespace no recognised shape asks
/// for. `media`, `composio` and `shell` are the ones worth naming: each is
/// reachable from exactly one shape, and a tampered focus must not be a
/// route to any of them.
#[test]
fn an_unrecognised_focus_can_never_out_grant_a_recognised_one() {
    const FORBIDDEN: [&str; 4] = ["media", "composio", "repo", "shell"];
    let unknown = tools_for_focus(AgentFocus::from_wire("media"));
    assert!(!unknown.is_empty());
    for grant in &unknown {
        let namespace = grant.split(['.', '_', ':']).next().unwrap_or(grant);
        assert!(
            !FORBIDDEN.contains(&namespace),
            "unknown focus grants {grant}"
        );
        assert_ne!(grant, "*");
    }
    // And the belt it lands on is one a real focus already has, not a
    // bespoke list that could drift away from the vocabulary.
    assert!(
        AgentFocus::ALL.iter().any(|f| f.tools() == unknown),
        "the fallback belt must be one of the real ones: {unknown:?}"
    );
}

/// The whole point, end to end: a roster whose focus values were tampered
/// with still yields agents that ask for a belt rather than inheriting one.
#[test]
fn a_tampered_focus_still_narrows_the_agent() {
    let wire = r#"[
        {"name":"A","role":"Ops","description":"d","focus":"media"},
        {"name":"B","role":"Money","description":"d","focus":"composio"},
        {"name":"C","role":"Writer","description":"d"}
    ]"#;
    let roster: Vec<ProposedAgent> = serde_json::from_str(wire).expect("parses");
    let manifest = manifest_from_setup(&answers("a shop", ""), &roster, None);
    for agent in &manifest.agents {
        assert!(
            agent.tools.as_deref().is_some_and(|t| !t.is_empty()),
            "{} inherits the lot",
            agent.id
        );
        assert!(
            !agent
                .tools
                .iter()
                .flatten()
                .any(|t| t == "media" || t == "composio" || t == "*"),
            "{} holds {:?}",
            agent.id,
            agent.tools
        );
    }
    assert_eq!(manifest.validate(), Vec::<String>::new());
}

// ---------------------------------------------------------------------
// The admin address, and the console that must agree about it
// ---------------------------------------------------------------------

/// The rule the console re-implements, pinned to a shared fixture.
///
/// A wizard that let `as` through produced a company whose manifest failed
/// validation on the *last* screen, after the roster had been designed and
/// the apply attempted — the operator was told "that didn't apply" about a
/// mistake they made four steps earlier.
///
/// The console cannot call this validator, so it re-implements the rule, and
/// this fixture is what stops the two drifting. Deliberately loose on the
/// host side: `normalize_email` is trim + lowercase and the only structural
/// demand is an `@`, because the rule exists to stop an entry normalizing
/// into something `LoginIdentity::parse` would misread — not to police what
/// a mail server accepts. A console applying a stricter regex would reject
/// addresses the host takes happily.
#[test]
fn the_admin_address_rule_matches_the_shared_fixture() {
    #[derive(serde::Deserialize)]
    struct Case {
        why: String,
        input: String,
        usable: bool,
    }
    #[derive(serde::Deserialize)]
    struct Fixture {
        cases: Vec<Case>,
    }

    let raw = include_str!("../../tests/fixtures/setup-admin-email.json");
    let fixture: Fixture = serde_json::from_str(raw).expect("fixture parses");
    assert!(
        !fixture.cases.is_empty(),
        "an empty fixture asserts nothing"
    );

    for case in &fixture.cases {
        assert_eq!(
            crate::ports::users::is_usable_admin_email(&case.input),
            case.usable,
            "{} — input {:?}",
            case.why,
            case.input
        );
    }

    // And the manifest validator applies the same rule, not a second one:
    // every address the predicate rejects must be refused when written.
    for case in fixture
        .cases
        .iter()
        .filter(|c| !c.usable && !c.input.trim().is_empty())
    {
        let manifest = manifest_from_setup(
            &answers("a shop", ""),
            &[proposed("Ops")],
            Some(&case.input),
        );
        assert!(
            manifest
                .validate()
                .iter()
                .any(|p| p.contains("[users].admins")),
            "{} — {:?} reached a valid manifest",
            case.why,
            case.input
        );
    }
}
