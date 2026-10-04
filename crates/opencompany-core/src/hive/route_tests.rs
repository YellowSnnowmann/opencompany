//! The starter ladder: mention, then Jev, then the hive's default responder,
//! every starter a member of the hive.

use tinyhivemind_core::embed::{
    CandidateProbability, ContributionProbability, EvaluationDisposition, RoutingEvaluation,
};
use tinyhivemind_core::responder::Probability;

use super::*;
use crate::ports::types::CompanyId;

fn record() -> CompanyRecord {
    let mut record = CompanyRecord::from_manifest(
        CompanyId::new("acme"),
        toml::from_str(
            r#"
[company]
name = "Acme"

[[agent]]
id = "ceo"
role = "Chief Executive"

[[agent]]
id = "writer"
role = "Writer"

[[agent]]
id = "editor"
role = "Editor"

[[group_chat]]
id = "content"
name = "Content"
members = ["writer", "editor"]
"#,
        )
        .expect("valid manifest"),
    );
    record.general_channel.members = vec!["ceo".into(), "writer".into(), "editor".into()];
    record
}

fn mention(id: &str) -> Mention {
    Mention {
        target: MentionTarget::Agent { id: id.to_string() },
        text: format!("@{id}"),
        offset: 0,
        quiet: false,
    }
}

#[tokio::test]
async fn a_mention_starts_exactly_the_named_members_of_the_hive() {
    let record = record();
    let starters = choose(&record, "content", "hi", &[mention("editor")], None, "ceo").await;
    assert_eq!(starters.route, StarterRoute::Mention);
    assert_eq!(starters.members, vec!["editor".to_string()]);
}

#[tokio::test]
async fn a_mention_of_somebody_off_the_hive_falls_back_to_the_default() {
    let record = record();
    let starters = choose(&record, "content", "hi", &[mention("ceo")], None, "ceo").await;
    assert_eq!(starters.route, StarterRoute::Default);
    assert_eq!(starters.members, vec!["writer".to_string()]);
}

#[tokio::test]
async fn without_jev_the_desk_lead_starts() {
    let starters = choose(&record(), "content", "draft a post", &[], None, "ceo").await;
    assert_eq!(starters.route, StarterRoute::Default);
    assert_eq!(starters.members, vec!["writer".to_string()]);
}

#[tokio::test]
async fn general_is_answered_by_the_fallback_when_it_sits_there() {
    let starters = choose(&record(), GENERAL_CHANNEL_ID, "hello", &[], None, "ceo").await;
    assert_eq!(starters.members, vec!["ceo".to_string()]);
}

/// A router that picks one candidate with certainty, or says `none`.
struct Picks(&'static str);

impl Router for Picks {
    fn evaluate<'a>(&'a self, request: &'a RoutingRequest) -> tinyhivemind_core::embed::RouterFuture<'a> {
        let pick = self.0;
        Box::pin(async move {
            let scale = tinyhivemind_core::responder::PROBABILITY_SCALE;
            let probability = |parts: u32| Probability::new(parts).expect("in range");
            let mut primary: Vec<CandidateProbability> = request
                .candidates
                .iter()
                .map(|candidate| CandidateProbability {
                    candidate_id: candidate.id.clone(),
                    probability: probability(if candidate.id == pick { scale } else { 0 }),
                })
                .collect();
            primary.push(CandidateProbability {
                candidate_id: "none".into(),
                probability: probability(if pick == "none" { scale } else { 0 }),
            });
            Ok(RoutingEvaluation {
                primary_responder: pick.to_string(),
                primary_probabilities: primary,
                confidence: probability(scale),
                needs_collaboration: probability(0),
                needs_clarification: probability(0),
                contributions: request
                    .candidates
                    .iter()
                    .map(|candidate| ContributionProbability {
                        candidate_id: candidate.id.clone(),
                        probability: probability(0),
                    })
                    .collect(),
                high_impact: probability(0),
                model_identity: "test".into(),
                question_schema_version: 1,
                roster_version: request.roster_version,
                disposition: EvaluationDisposition::Unchecked,
            })
        })
    }
}

#[tokio::test]
async fn jev_starts_the_member_it_routed_to() {
    let router = Picks("editor");
    let starters = choose(&record(), "content", "copy-edit this", &[], Some(&router), "ceo").await;
    assert_eq!(starters.route, StarterRoute::Jev);
    assert_eq!(starters.members, vec!["editor".to_string()]);
}

#[tokio::test]
async fn an_unclear_jev_plan_falls_back_to_the_lead() {
    let router = Picks("none");
    let starters = choose(&record(), "content", "hmm", &[], Some(&router), "ceo").await;
    assert_eq!(starters.route, StarterRoute::Default);
    assert_eq!(starters.members, vec!["writer".to_string()]);
}
