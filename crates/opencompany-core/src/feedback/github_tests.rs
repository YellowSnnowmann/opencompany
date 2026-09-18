use super::*;

#[tokio::test]
async fn no_client_degrades_to_manual_link() {
    let limiter = RateLimiter::default();
    let out = file_feedback(
        None,
        "tinyhumansai/opencompany",
        "acme",
        "[bug] broken route",
        "body — filed by @acme",
        &["feedback".into(), "source/agent-filed".into()],
        ConsentMode::Auto,
        &limiter,
    )
    .await
    .unwrap();
    match out {
        FilingOutcome::ManualLink { url } => {
            assert!(url.starts_with("https://github.com/tinyhumansai/opencompany/issues/new"));
            assert!(url.contains("title="));
            assert!(url.contains("body="));
            assert!(url.contains("labels="));
        }
        other => panic!("expected ManualLink, got {other:?}"),
    }
}

#[tokio::test]
async fn manual_consent_never_calls_client() {
    let client = MockGitHubClient::new();
    let limiter = RateLimiter::default();
    let out = file_feedback(
        Some(&client),
        "r/r",
        "acme",
        "t",
        "b",
        &[],
        ConsentMode::Manual,
        &limiter,
    )
    .await
    .unwrap();
    assert!(matches!(out, FilingOutcome::ManualLink { .. }));
    assert!(client.created().is_empty());
}

#[tokio::test]
async fn assisted_consent_never_auto_creates() {
    // A throttled low-quality filer is downgraded Auto -> Assisted, which
    // must go to the operator for approval, never silently auto-file.
    let client = MockGitHubClient::new();
    let limiter = RateLimiter::default();
    let out = file_feedback(
        Some(&client),
        "r/r",
        "acme",
        "t",
        "b",
        &[],
        ConsentMode::Assisted,
        &limiter,
    )
    .await
    .unwrap();
    assert!(matches!(out, FilingOutcome::ManualLink { .. }));
    assert!(client.created().is_empty());
}

#[tokio::test]
async fn auto_consent_creates_issue_with_labels_and_signature() {
    let client = MockGitHubClient::new();
    let limiter = RateLimiter::default();
    let body = sign_body("the route is broken", "acme");
    let labels = vec![
        "feedback".to_string(),
        "type/bug".to_string(),
        "area/runtime".to_string(),
        "sev/annoyance".to_string(),
        "source/agent-filed".to_string(),
    ];
    let out = file_feedback(
        Some(&client),
        "r/r",
        "acme",
        "[bug] route",
        &body,
        &labels,
        ConsentMode::Auto,
        &limiter,
    )
    .await
    .unwrap();
    assert!(matches!(out, FilingOutcome::Filed { .. }));

    let created = client.created();
    assert_eq!(created.len(), 1);
    assert!(created[0].body.contains("— filed by @acme"));
    assert!(
        created[0]
            .labels
            .contains(&"source/agent-filed".to_string())
    );
    assert!(created[0].labels.contains(&"type/bug".to_string()));
}

#[tokio::test]
async fn dedupe_comments_instead_of_creating() {
    let client = MockGitHubClient::new().with_existing(
        42,
        "https://github.com/mock/issues/42",
        "[bug] route",
    );
    let limiter = RateLimiter::default();
    let out = file_feedback(
        Some(&client),
        "r/r",
        "acme",
        "[bug] route",
        "body — filed by @acme",
        &[],
        ConsentMode::Auto,
        &limiter,
    )
    .await
    .unwrap();
    match out {
        FilingOutcome::Deduped { url } => assert_eq!(url, "https://github.com/mock/issues/42"),
        other => panic!("expected Deduped, got {other:?}"),
    }
    assert!(client.created().is_empty());
    assert_eq!(client.comments().len(), 1);
    assert_eq!(client.comments()[0].0, 42);
}

#[tokio::test]
async fn rate_limit_trips_after_budget() {
    let client = MockGitHubClient::new();
    let limiter = RateLimiter::new(1);
    let file = |title: &'static str| {
        file_feedback(
            Some(&client),
            "r/r",
            "acme",
            title,
            "b — filed by @acme",
            &[],
            ConsentMode::Auto,
            &limiter,
        )
    };
    assert!(matches!(
        file("one").await.unwrap(),
        FilingOutcome::Filed { .. }
    ));
    assert!(matches!(
        file("two").await.unwrap(),
        FilingOutcome::RateLimited
    ));
}

#[test]
fn signature_carries_handle() {
    assert_eq!(sign_body("x", "acme"), "x\n\n— filed by @acme");
}
