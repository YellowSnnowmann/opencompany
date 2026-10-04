//! The starter ladder: mention, then Jev, then the hive's default responder,
//! every starter a member of the hive.

use std::future::Future;
use std::pin::Pin;

use tinyhivemind_core::embed::{EvaluationDisposition, RoutingEvaluation};

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

/// A router that always picks one candidate with certainty.
struct Picks(&'static str);

impl Router for Picks {
    fn evaluate<'a>(
        &'a self,
        request: &'a RoutingRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RoutingEvaluation, tinyhivemind_core::embed::RouterError>> + Send + 'a>>
    {
        let pick = self.0;
        Box::pin(async move {
            let mut evaluation: RoutingEvaluation = serde_json::from_value(serde_json::json!({
                "primary": [],
                "contributions": [],
                "clarification_needed": 0,
                "high_impact": 0,
                "disposition": "accepted"
            }))
            .unwrap_or_else(|_| panic!("evaluation shape for {}", request.message));
            evaluation.disposition = EvaluationDisposition::Accepted;
            let _ = pick;
            Ok(evaluation)
        })
    }
}

#[tokio::test]
async fn an_unusable_jev_plan_falls_back_to_the_lead() {
    let router = Picks("editor");
    let starters = choose(&record(), "content", "draft a post", &[], Some(&router), "ceo").await;
    assert_eq!(starters.members, vec!["writer".to_string()]);
    assert_eq!(starters.route, StarterRoute::Default);
}
