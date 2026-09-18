use super::*;
use crate::feedback::github::MockGitHubClient;
use crate::feedback::types::{FeedbackCategory, FeedbackInput};

fn item(category: FeedbackCategory, template: Option<&str>) -> FeedbackItem {
    FeedbackItem::capture(
        FeedbackInput {
            category,
            note: "n".into(),
            work_ref: None,
            template_name: template.map(str::to_string),
            template_version: None,
        },
        "0.1.0",
        ConsentMode::Auto,
    )
}

#[test]
fn classify_covers_all_four_axes() {
    let labels = classify_labels(
        &item(FeedbackCategory::Bug, None),
        Severity::Blocked,
        FeedbackSource::Operator,
    );
    assert!(labels.contains(&"feedback".to_string()));
    assert!(labels.contains(&"type/bug".to_string()));
    assert!(labels.contains(&"area/runtime".to_string()));
    assert!(labels.contains(&"sev/blocked".to_string()));
    assert!(labels.contains(&"source/operator".to_string()));
    // Exactly one label per axis plus the base label.
    assert_eq!(labels.len(), 5);
}

#[test]
fn classify_names_template_area_and_money_lost() {
    let labels = classify_labels(
        &item(FeedbackCategory::TemplateGap, Some("marketing_agency")),
        Severity::MoneyLost,
        FeedbackSource::AgentFiled,
    );
    assert!(labels.contains(&"area/template:marketing_agency".to_string()));
    assert!(labels.contains(&"sev/money-lost".to_string()));
    assert!(labels.contains(&"source/agent-filed".to_string()));
    assert!(labels.contains(&"type/template-gap".to_string()));
}

#[test]
fn severity_and_source_wire_tokens_are_exact() {
    assert_eq!(Severity::Annoyance.as_str(), "annoyance");
    assert_eq!(Severity::Blocked.as_str(), "blocked");
    assert_eq!(Severity::MoneyLost.as_str(), "money-lost");
    assert_eq!(FeedbackSource::Operator.as_str(), "operator");
    assert_eq!(FeedbackSource::AgentFiled.as_str(), "agent-filed");
    assert_eq!(FeedbackSource::Platform.as_str(), "platform");
}

#[tokio::test]
async fn dedupe_comments_on_canonical_and_closes_duplicate() {
    let client = Arc::new(
        MockGitHubClient::new()
            .with_existing(42, "https://gh/issues/42", "email drafts too formal")
            .with_existing(57, "https://gh/issues/57", "email drafts too formal"),
    );
    let agent = TriageAgent::new(client.clone(), "acme/repo");
    let candidate = ExistingIssue {
        number: 57,
        url: "https://gh/issues/57".into(),
        title: "email drafts too formal".into(),
    };

    let plan = agent.dedupe(&candidate).await.unwrap();
    match plan {
        DedupePlan::Merged { canonical, closed } => {
            assert_eq!(canonical.number, 42);
            assert_eq!(closed, 57);
        }
        other => panic!("expected Merged, got {other:?}"),
    }
    // Commented on the canonical, closed the duplicate, created nothing.
    assert_eq!(client.comments().len(), 1);
    assert_eq!(client.comments()[0].0, 42);
    assert_eq!(client.closed(), vec![57]);
    assert!(client.created().is_empty());
}

#[tokio::test]
async fn dedupe_leaves_a_distinct_issue_untouched() {
    let client =
        Arc::new(MockGitHubClient::new().with_existing(9, "https://gh/issues/9", "unique problem"));
    let agent = TriageAgent::new(client.clone(), "acme/repo");
    let candidate = ExistingIssue {
        number: 9,
        url: "https://gh/issues/9".into(),
        title: "unique problem".into(),
    };
    assert_eq!(
        agent.dedupe(&candidate).await.unwrap(),
        DedupePlan::Distinct
    );
    assert!(client.comments().is_empty());
    assert!(client.closed().is_empty());
}

#[test]
fn cluster_plans_group_similar_titles_and_score() {
    let issues = vec![
        ExistingIssue {
            number: 3,
            url: "u3".into(),
            title: "[wrong-output] email drafts too formal".into(),
        },
        ExistingIssue {
            number: 1,
            url: "u1".into(),
            title: "Email drafts too formal".into(),
        },
        ExistingIssue {
            number: 2,
            url: "u2".into(),
            title: "email  drafts   too formal".into(),
        },
        ExistingIssue {
            number: 8,
            url: "u8".into(),
            title: "a lone report".into(),
        },
    ];
    let plans = cluster_plans(&issues);
    assert_eq!(plans.len(), 1, "only the 3-member group clusters");
    let plan = &plans[0];
    assert_eq!(plan.count, 3);
    assert_eq!(plan.members, vec![1, 2, 3]);
    assert!(plan.title.starts_with("3 reports:"));
    // count × severity.
    assert_eq!(plan.score(Severity::Annoyance), 3);
    assert_eq!(plan.score(Severity::MoneyLost), 30);
    assert!(!plan.promote(Severity::Annoyance, 10));
    assert!(plan.promote(Severity::MoneyLost, 10));
}

#[tokio::test]
async fn maintain_cluster_creates_issue_and_closes_members() {
    let client = Arc::new(MockGitHubClient::new());
    let agent = TriageAgent::new(client.clone(), "acme/repo");
    let plan = ClusterPlan {
        key: "email drafts too formal".into(),
        summary: "email drafts too formal".into(),
        members: vec![1, 2, 3],
        count: 3,
        title: "3 reports: email drafts too formal".into(),
    };
    let url = agent
        .maintain_cluster(&plan, "template:marketing_agency")
        .await
        .unwrap();
    assert!(url.starts_with("https://github.com/mock/issues/"));
    let created = client.created();
    assert_eq!(created.len(), 1);
    assert!(created[0].title.contains("template:marketing_agency"));
    assert!(
        created[0]
            .labels
            .contains(&"area/template:marketing_agency".to_string())
    );
    // Every member was commented on and closed.
    assert_eq!(client.closed(), vec![1, 2, 3]);
    assert_eq!(client.comments().len(), 3);
}

#[test]
fn throttle_downgrades_auto_after_low_quality_filings() {
    let ledger = QualityLedger::new(0.5, 2);
    // Below the sample floor: no downgrade yet.
    ledger.record_filed("noisy");
    ledger.record_low_quality("noisy");
    assert_eq!(
        ledger.effective_consent("noisy", ConsentMode::Auto),
        ConsentMode::Auto
    );
    // Second low-quality filing crosses the floor and the ratio.
    ledger.record_filed("noisy");
    ledger.record_low_quality("noisy");
    assert_eq!(
        ledger.effective_consent("noisy", ConsentMode::Auto),
        ConsentMode::Assisted
    );
    // A clean handle keeps Auto; non-Auto modes always pass through.
    ledger.record_filed("clean");
    ledger.record_filed("clean");
    assert_eq!(
        ledger.effective_consent("clean", ConsentMode::Auto),
        ConsentMode::Auto
    );
    assert_eq!(
        ledger.effective_consent("noisy", ConsentMode::Manual),
        ConsentMode::Manual
    );
}

#[test]
fn escalation_flags_money_lost_and_brain() {
    let money = escalation_for(&["sev/money-lost".to_string(), "area/product".to_string()]);
    assert!(money.pages_maintainers);
    assert!(money.mirror_to.is_none());

    let brain = escalation_for(&["area/brain".to_string(), "sev/annoyance".to_string()]);
    assert!(!brain.pages_maintainers);
    assert_eq!(brain.mirror_to.as_deref(), Some("tinyhumansai/medulla"));
}

#[test]
fn release_notes_map_and_process_bug_check() {
    let mut filed = item(FeedbackCategory::Bug, None);
    filed.filed_issue_url = Some("https://gh/issues/7".into());
    let other = item(FeedbackCategory::Docs, None);
    let items = vec![filed, other];

    let mapped = map_fixed_issues(&["https://gh/issues/7".to_string()], &items);
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].0, "https://gh/issues/7");
    assert_eq!(mapped[0].1.category, FeedbackCategory::Bug);

    // A closed feedback issue missing from the notes is a process bug.
    let bugs = process_bug_check(
        &[
            "https://gh/issues/7".to_string(),
            "https://gh/issues/9".to_string(),
        ],
        &["https://gh/issues/7".to_string()],
    );
    assert_eq!(bugs, vec!["https://gh/issues/9".to_string()]);
}
