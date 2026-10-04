//! Tests for the `[group_chat.routing]` block, its resolution, and its wire
//! shapes.

use super::*;
use crate::ports::types::{CompanyId, DeskHiveOverride};
use tinyhivemind_core::embed::{
    CandidateProbability, EvaluationDisposition, RoutingEvaluation, RoutingFallback,
};

fn record(toml_src: &str) -> CompanyRecord {
    let manifest = toml::from_str(toml_src).expect("valid manifest");
    CompanyRecord::from_manifest(CompanyId::new("acme"), manifest)
}

const TWO_DESKS: &str = r#"
[company]
name = "Acme"

[[agent]]
id = "ceo"
role = "Chief Executive"

[[agent]]
id = "engineer"
role = "Engineer"

[[agent]]
id = "writer"
role = "Writer"

[[group_chat]]
id = "engineering"
name = "Engineering desk"
members = ["engineer", "ceo"]

[group_chat.routing]
round_width = 2
max_rounds = 3
turn_timeout_secs = 900

[[group_chat]]
id = "content"
name = "Content desk"
members = ["writer", "ceo"]
"#;

#[test]
fn a_manifest_block_parses_under_routing_and_resolves_its_defaults() {
    let record = record(TWO_DESKS);
    let (config, source) = effective_routing(&record, "engineering");
    assert_eq!(source, RoutingSource::Manifest);
    assert_eq!(config.round_width, Some(2));
    let effective = EffectiveRouting::resolve(&config);
    assert_eq!(effective.round_width, 2);
    assert_eq!(effective.choice_option_limit, DEFAULT_CHOICE_OPTION_LIMIT);
    assert_eq!(effective.max_rounds, 3);
    assert_eq!(effective.turn_timeout_secs, 900);
    let policy = effective.policy();
    assert_eq!(policy.round_width, 2);
    assert_eq!(policy.clarification_threshold, Probability::ONE);
    assert_eq!(policy.minimum_confidence, Probability::ZERO);
}

#[test]
fn an_undeclared_desk_reads_as_default_and_an_overlay_outranks_the_manifest() {
    let mut record = record(TWO_DESKS);
    let (config, source) = effective_routing(&record, "content");
    assert_eq!(source, RoutingSource::Default);
    assert!(config.is_default());
    record.upsert_desk_hive(DeskHiveOverride {
        desk_id: "engineering".into(),
        hive: RoutingConfig {
            round_width: Some(1),
            ..RoutingConfig::default()
        },
    });
    let (config, source) = effective_routing(&record, "engineering");
    assert_eq!(source, RoutingSource::Overlay);
    assert_eq!(config.round_width, Some(1));
    assert!(record.clear_desk_hive("engineering"));
    assert_eq!(
        effective_routing(&record, "engineering").1,
        RoutingSource::Manifest
    );
}

#[test]
fn problems_name_the_zero_and_out_of_range_keys() {
    let config = RoutingConfig {
        round_width: Some(0),
        choice_option_limit: Some(1),
        minimum_confidence: Some(1.5),
        max_rounds: Some(0),
        turn_timeout_secs: Some(0),
        ..RoutingConfig::default()
    };
    let problems = config.problems("desk `x`");
    let joined = problems.join("\n");
    for key in [
        "round_width = 0",
        "choice_option_limit",
        "minimum_confidence",
        "max_rounds = 0",
        "turn_timeout_secs = 0",
    ] {
        assert!(joined.contains(key), "missing `{key}` in:\n{joined}");
    }
    assert_eq!(problems.len(), 5);
    assert!(RoutingConfig::default().problems("desk").is_empty());
}

#[test]
fn the_plan_dto_is_camel_case_and_tagged_by_kind() {
    let evaluation = RoutingEvaluation {
        primary_responder: "engineer".into(),
        primary_probabilities: vec![CandidateProbability {
            candidate_id: "engineer".into(),
            probability: Probability::ONE,
        }],
        confidence: Probability::ONE,
        needs_collaboration: Probability::ZERO,
        needs_clarification: Probability::ZERO,
        contributions: Vec::new(),
        high_impact: Probability::ZERO,
        model_identity: "jev".into(),
        question_schema_version: 1,
        roster_version: 1,
        disposition: EvaluationDisposition::Accepted,
    };
    let hive = RoutingPlan::Hive {
        primary_id: "engineer".into(),
        invited_ids: vec!["ceo".into()],
        evaluation: evaluation.clone(),
    };
    let dto = RoutingPlanDto::from(&hive);
    assert_eq!(
        serde_json::to_value(&dto).unwrap(),
        serde_json::json!({"kind": "hive", "primaryId": "engineer", "invitedIds": ["ceo"]})
    );
    assert_eq!(dto.agent_ids(), vec!["engineer", "ceo"]);
    assert_eq!(dto.router(), Router::Jev);
    assert_eq!(router_of(&hive), Router::Jev);

    let explicit = RoutingPlan::Fallback {
        responder_id: "ceo".into(),
        reason: RoutingFallback::ExplicitMention,
    };
    let dto = RoutingPlanDto::from(&explicit);
    assert_eq!(
        serde_json::to_value(&dto).unwrap(),
        serde_json::json!({"kind": "fallback", "primaryId": "ceo", "reason": "explicit_mention"})
    );
    assert_eq!(dto.router(), Router::Explicit);
    assert_eq!(router_of(&explicit), Router::Explicit);
    let lead = RoutingPlan::Fallback {
        responder_id: "ceo".into(),
        reason: RoutingFallback::ProviderUnavailable,
    };
    assert_eq!(router_of(&lead), Router::Fallback);
    let one = RoutingPlan::One {
        responder_id: "engineer".into(),
        evaluation,
    };
    assert_eq!(
        serde_json::to_value(RoutingPlanDto::from(&one)).unwrap(),
        serde_json::json!({"kind": "one", "primaryId": "engineer"})
    );
    // Round trip: the journal stores the DTO form.
    let back: RoutingPlanDto = serde_json::from_value(serde_json::to_value(&dto).unwrap()).unwrap();
    assert_eq!(back, dto);
}

#[test]
fn the_desk_routing_dto_lists_candidates_with_the_desks_they_share() {
    let record = record(TWO_DESKS);
    let dto = desk_routing_dto(&record, "engineering", Router::Fallback);
    assert_eq!(dto.source, RoutingSource::Manifest);
    assert_eq!(dto.declared.round_width, Some(2));
    assert_eq!(dto.effective.round_width, 2);
    assert_eq!(dto.effective.router, Router::Fallback);
    let ceo = dto
        .candidates
        .iter()
        .find(|candidate| candidate.agent_id == "ceo")
        .expect("ceo is a candidate");
    // `name` is an in-memory carrier the roster build fills, not a manifest
    // key, so a manifest record labels a seat by id.
    assert_eq!(ceo.label, "ceo");
    assert_eq!(ceo.role, "Chief Executive");
    assert_eq!(ceo.shared_with, vec!["content"]);
    let engineer = dto
        .candidates
        .iter()
        .find(|candidate| candidate.agent_id == "engineer")
        .expect("engineer is a candidate");
    assert!(engineer.shared_with.is_empty());
    let value = serde_json::to_value(&dto).unwrap();
    assert_eq!(value["deskId"], "engineering");
    assert_eq!(value["declared"]["round_width"], 2);
    assert_eq!(value["declared"]["max_rounds"], 3);
    assert_eq!(value["effective"]["roundWidth"], 2);
    assert_eq!(value["effective"]["turnTimeoutSecs"], 900);
    assert_eq!(value["candidates"][1]["sharedWith"][0], "content");
    let summary = desk_routing_summary(&record, "content", Router::Jev);
    assert_eq!(summary.source, RoutingSource::Default);
    assert_eq!(summary.round_width, DEFAULT_ROUND_WIDTH);
    assert_eq!(
        serde_json::to_value(&summary).unwrap()["router"],
        serde_json::json!("jev")
    );
}

#[test]
fn probabilities_round_trip_through_the_fixed_point_scale() {
    assert_eq!(probability(0.0), Probability::ZERO);
    assert_eq!(probability(1.0), Probability::ONE);
    assert_eq!(probability(2.0), Probability::ONE);
    assert_eq!(probability(0.5).parts(), 500_000);
}

#[test]
fn the_coordinator_folds_every_desk_to_the_widest_round_and_longest_wall() {
    let record = record(TWO_DESKS);
    let options = coordinator_options(&record);
    // `content` declares nothing, so it runs at the default width of 5 — the
    // widest — while `engineering`'s 3 rounds are fewer than the default 12.
    assert_eq!(options.round_width, DEFAULT_ROUND_WIDTH);
    assert_eq!(
        options.conduct_policy.turn_wall,
        DEFAULT_ROUND_WIDTH as u64 * u64::from(DEFAULT_MAX_ROUNDS)
    );
    assert_eq!(options.retention.settled_episodes, Some(RETAINED_SETTLED_EPISODES));
    assert_eq!(options.retention.delivered, Some(RETAINED_DELIVERIES));
    assert_eq!(turn_timeout(&record), std::time::Duration::from_secs(900));
}

#[test]
fn a_company_with_no_desks_runs_on_the_defaults() {
    let record = record(
        r#"
[company]
name = "Solo"

[[agent]]
id = "a"
role = "A"
"#,
    );
    assert_eq!(coordinator_options(&record).round_width, DEFAULT_ROUND_WIDTH);
    assert_eq!(
        turn_timeout(&record),
        std::time::Duration::from_secs(DEFAULT_TURN_TIMEOUT_SECS)
    );
}
