//! Setup roster synthesis: template selection, bounds, truncation, and
//! duplicate/short-roster handling.

use super::*;

fn answers(industry: &str, automate: &str) -> SetupAnswers {
    SetupAnswers {
        industry: industry.to_string(),
        team_hint: String::new(),
        automate: automate.to_string(),
    }
}

fn agent(role: &str) -> ProposedAgent {
    ProposedAgent {
        name: role.to_string(),
        role: role.to_string(),
        description: "does the thing".to_string(),
        focus: None,
    }
}

/// The spec's worked example: "I sell homeware online" must staff the
/// e-commerce team, mandate-for-mandate.
#[test]
fn the_worked_example_lands_the_ecommerce_roster() {
    let picked = match_template(&answers(
        "E-commerce — I sell homeware online",
        "Social media posts, Meta ads, generating my reports, order dispatch",
    ));
    assert_eq!(picked.key, "ecommerce");
    let roles: Vec<&str> = picked.agents.iter().map(|a| a.role).collect();
    assert!(roles.contains(&"Logistics Coordinator"), "{roles:?}");
    assert!(roles.contains(&"Meta Ads Specialist"), "{roles:?}");
}

/// The weighting that keeps the automation list from overruling the
/// business. An e-commerce operator naming social posts is still running a
/// shop, and staffing them as a content studio would leave nobody on
/// dispatch.
#[test]
fn the_industry_answer_outweighs_the_automation_list() {
    let picked = match_template(&answers(
        "online store selling homeware",
        "instagram, tiktok, youtube, podcast, newsletter, blog",
    ));
    assert_eq!(picked.key, "ecommerce");
}

/// The automation answer still decides when the industry says nothing
/// recognisable — it is the tiebreak, not dead weight.
#[test]
fn the_automation_answer_breaks_a_tie() {
    let picked = match_template(&answers("just me", "scheduling my youtube uploads"));
    assert_eq!(picked.key, "content");
}

/// A miss must land a real team, not nothing. This is decision D3's cheap
/// half: the never-strand fallback is a curated roster.
#[test]
fn an_unrecognised_business_still_gets_a_real_team() {
    let picked = match_template(&answers("zzzz qqqq", ""));
    assert_eq!(picked.key, "generic");
    assert!(picked.agents.len() >= MIN_AGENTS);
}

/// Every curated roster must itself satisfy the rules it is the fallback
/// for. A template that could not pass validation would be a floor that
/// does not hold.
#[test]
fn every_template_is_within_its_own_bounds() {
    for template in TEMPLATES {
        let count = template.agents.len();
        assert!(
            (MIN_AGENTS..=MAX_AGENTS).contains(&count),
            "{} has {count} agents",
            template.key
        );
        let validated = validate_roster(template.proposed());
        assert_eq!(
            validated.len(),
            count,
            "{} lost agents to validation",
            template.key
        );
        for a in template.agents {
            assert!(
                !a.role.trim().is_empty(),
                "{} has a blank role",
                template.key
            );
            assert!(
                a.description.chars().count() <= MAX_DESCRIPTION,
                "{} has an over-long mandate",
                template.key
            );
        }
    }
}

/// Template keys are how a proposal reports which roster it came from, so
/// two templates sharing one would make that report ambiguous.
#[test]
fn template_keys_are_unique() {
    let mut keys: Vec<&str> = TEMPLATES.iter().map(|t| t.key).collect();
    keys.sort_unstable();
    let before = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), before, "duplicate template key");
}

#[test]
fn an_over_long_roster_is_truncated() {
    let long: Vec<ProposedAgent> = (0..12).map(|i| agent(&format!("Role {i}"))).collect();
    assert_eq!(validate_roster(long).len(), MAX_AGENTS);
}

/// **No padding.** A short roster comes back short, so nothing an operator is
/// shown was quietly borrowed from a template they never saw.
///
/// The regression this guards is concrete: a yoga studio's pass returned
/// three agents, validation padded it to four from the `content` template,
/// and the fourth teammate on screen was a Content Strategist — rendered
/// identically to the three the operator had actually asked for. Deciding
/// what to do about a thin roster belongs to the caller, which falls back to
/// the curated team **whole**.
#[test]
fn a_short_roster_is_left_short_rather_than_padded() {
    let roster = validate_roster(vec![agent("Meta Ads Specialist")]);
    assert_eq!(roster.len(), 1, "validation must not invent teammates");
    assert_eq!(roster[0].role, "Meta Ads Specialist");
}

/// Two teammates sharing one job is the failure the operator would have to
/// clean up by hand, so near-miss spellings collapse too.
#[test]
fn duplicate_roles_collapse_however_they_are_spelled() {
    let roster = validate_roster(vec![
        agent("SEO Specialist"),
        agent("seo  specialist"),
        agent("SEO-Specialist"),
    ]);
    let seo = roster
        .iter()
        .filter(|a| role_slug(&a.role) == "seo-specialist")
        .count();
    assert_eq!(seo, 1, "{roster:?}");
}

#[test]
fn a_roleless_entry_is_dropped_and_a_blank_name_falls_back_to_the_role() {
    let roster = validate_roster(vec![
        ProposedAgent {
            name: "Ghost".into(),
            role: "   ".into(),
            description: String::new(),
            focus: None,
        },
        ProposedAgent {
            name: "  ".into(),
            role: "Data Analyst".into(),
            description: String::new(),
            focus: None,
        },
    ]);
    assert!(roster.iter().all(|a| !a.role.trim().is_empty()));
    let analyst = roster.iter().find(|a| a.role == "Data Analyst").unwrap();
    assert_eq!(analyst.name, "Data Analyst");
}

/// A model asked for one line occasionally writes a paragraph. The card has
/// one line for it, so the cap is on the data.
#[test]
fn an_over_long_mandate_is_clamped() {
    let essay = "word ".repeat(200);
    let roster = validate_roster(vec![ProposedAgent {
        name: "A".into(),
        role: "Analyst".into(),
        description: essay,
        focus: None,
    }]);
    let clamped = &roster[0].description;
    assert!(clamped.chars().count() <= MAX_DESCRIPTION + 1, "{clamped}");
    assert!(clamped.ends_with('…'), "{clamped}");
}

/// Validation of nothing is nothing. The floor is the caller's business now,
/// and `template_proposal` is where an operator with no usable model still
/// gets a real team.
#[test]
fn validation_of_an_empty_roster_stays_empty() {
    assert!(validate_roster(Vec::new()).is_empty());
}

/// The honest fallback: a full curated team, labelled as such, for the
/// offline path and every failure path.
#[test]
fn the_fallback_is_a_whole_curated_team_and_says_so() {
    let proposal = template_proposal(
        &answers("I sell homeware online", ""),
        FallbackReason::NoModel,
    );
    assert_eq!(proposal.template_key, "ecommerce");
    assert_eq!(proposal.source, RosterSource::Fallback);
    assert_eq!(proposal.source.as_str(), "fallback");
    assert!(
        proposal.agents.len() >= MIN_AGENTS,
        "a fallback must be a workable team, got {}",
        proposal.agents.len()
    );
    // Whole, not blended: every row is the template's own.
    let curated: Vec<&str> = ECOMMERCE.agents.iter().map(|a| a.role).collect();
    for a in &proposal.agents {
        assert!(
            curated.contains(&a.role.as_str()),
            "{} is not curated",
            a.role
        );
    }
}

// ---------------------------------------------------------------------
// Synthesising a company from the answers
// ---------------------------------------------------------------------
