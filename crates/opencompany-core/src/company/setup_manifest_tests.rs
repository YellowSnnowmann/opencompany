//! Setup manifest synthesis: validation, admin invite, policy tier, ids,
//! naming, and serde round-tripping of the setup answers.

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

/// The whole point of the synthesis: what comes out must be a company the
/// runtime will accept. `validate` is what `opencompany check` runs, so an
/// empty problem list is the same bar a hand-written manifest clears.
#[test]
fn a_synthesised_company_passes_validation() {
    let answers = answers("E-commerce — I sell homeware online", "Meta ads, dispatch");
    let roster = vec![
        proposed("Meta Ads Specialist"),
        proposed("Order Dispatch Coordinator"),
        proposed("Accountant"),
        proposed("Operations Lead"),
    ];
    let manifest = manifest_from_setup(&answers, &roster, Some("ada@example.com"));
    assert_eq!(manifest.validate(), Vec::<String>::new());
    assert_eq!(manifest.agents.len(), 4);
}

/// The dead end this flow exists to close: no shipped template invites
/// anybody, so an operator who picks email sign-in and is not written into
/// `[users].admins` completes setup and can then sign in as nobody.
#[test]
fn the_operator_is_invited_as_an_admin() {
    let manifest = manifest_from_setup(
        &answers("a shop", ""),
        &[proposed("Accountant")],
        Some("  ada@example.com  "),
    );
    assert_eq!(manifest.users.admins, vec!["ada@example.com".to_string()]);
}

/// A host that needs no sign-in supplies no address, and inviting `""`
/// would put an unusable row in the admin list.
#[test]
fn no_address_invites_nobody() {
    for email in [None, Some(""), Some("   ")] {
        let manifest = manifest_from_setup(&answers("a shop", ""), &[proposed("Ops")], email);
        assert!(manifest.users.admins.is_empty(), "{email:?}");
    }
}

/// Setup-created and provision-created companies must be indistinguishable.
/// Reading the constant rather than a literal is what keeps them that way
/// when the product next moves the default (#605).
#[test]
fn the_policy_tier_is_the_provisioned_default_not_a_literal() {
    let manifest = manifest_from_setup(&answers("a shop", ""), &[proposed("Ops")], None);
    assert_eq!(
        manifest.policy.mode,
        crate::company::PROVISIONED_POLICY_MODE
    );
}

/// `validate` rejects duplicate ids, and two roles can slug alike — so the
/// de-duplication has to happen here rather than surface to an operator who
/// typed nothing wrong.
#[test]
fn roles_that_slug_alike_still_get_distinct_ids() {
    let manifest = manifest_from_setup(
        &answers("a shop", ""),
        &[
            proposed("Ops Lead"),
            proposed("ops  lead"),
            proposed("OPS-LEAD"),
        ],
        None,
    );
    let ids: Vec<&str> = manifest.agents.iter().map(|a| a.id.as_str()).collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
    assert_eq!(manifest.validate(), Vec::<String>::new());
}

/// A role that starts with a digit slugs to something `is_snake_case`
/// refuses, and the operator never sees why. Handled here instead.
#[test]
fn a_role_starting_with_a_digit_still_yields_a_valid_id() {
    let manifest = manifest_from_setup(&answers("a studio", ""), &[proposed("3D Artist")], None);
    assert_eq!(manifest.validate(), Vec::<String>::new());
    assert!(
        manifest.agents[0]
            .id
            .starts_with(|c: char| c.is_ascii_lowercase()),
        "{}",
        manifest.agents[0].id
    );
}

/// The name is taken from the first clause of their own sentence rather
/// than asked for — a name is trivial to change later and tedious to be
/// asked for before you have seen anything.
#[test]
fn the_company_is_named_from_the_first_clause() {
    for (typed, expected) in [
        ("E-commerce — I sell homeware online", "E-commerce"),
        // A spaced hyphen is the same clause break, typed by someone whose
        // keyboard has no em dash.
        ("E-commerce - I sell homeware online", "E-commerce"),
        (
            "A yoga studio in Pune, drop-in classes",
            "A yoga studio in Pune",
        ),
        // No separator at all: the whole sentence is the name.
        ("Homeware shop", "Homeware shop"),
    ] {
        let manifest = manifest_from_setup(&answers(typed, ""), &[proposed("Ops")], None);
        assert_eq!(manifest.company.name, expected, "typed: {typed}");
    }
}

/// The hyphen regression, kept as its own case because it is the one a
/// reader would not predict: "E-commerce" must never become "E".
#[test]
fn a_hyphen_inside_a_word_does_not_split_the_name() {
    let manifest = manifest_from_setup(
        &answers("e-commerce and drop-shipping", ""),
        &[proposed("Ops")],
        None,
    );
    assert_eq!(manifest.company.name, "e-commerce and drop-shipping");
}

/// Someone who typed nothing still gets a valid, named company.
#[test]
fn an_unnamed_business_still_yields_a_valid_company() {
    let manifest = manifest_from_setup(&SetupAnswers::default(), &[proposed("Ops")], None);
    assert!(!manifest.company.name.trim().is_empty());
    assert_eq!(manifest.validate(), Vec::<String>::new());
}

/// Setup builds a roster and nothing else. Desks, workflows, schedules and
/// budgets stay at their defaults, so a later edit is an ordinary change
/// rather than an unpicking of something setup assumed.
#[test]
fn synthesis_invents_nothing_beyond_the_roster() {
    let manifest = manifest_from_setup(
        &answers("a shop", "everything"),
        &[proposed("Ops"), proposed("Accountant")],
        None,
    );
    assert!(manifest.group_chats.is_empty(), "no desks were asked for");
    assert!(manifest.schedules.is_empty(), "no schedule was asked for");
}

/// The answers ride on the company record, so they must survive the round
/// trip the record makes through its store.
#[test]
fn answers_round_trip_through_serde() {
    let answers = SetupAnswers {
        industry: "E-commerce".into(),
        team_hint: "plus customer support".into(),
        automate: "Meta ads, order dispatch".into(),
    };
    let json = serde_json::to_string(&answers).expect("serialize");
    assert_eq!(
        serde_json::from_str::<SetupAnswers>(&json).expect("deserialize"),
        answers
    );
    // And a record written before setup existed still loads.
    assert_eq!(
        serde_json::from_str::<SetupAnswers>("{}").expect("empty"),
        SetupAnswers::default()
    );
}
