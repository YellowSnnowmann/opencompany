use super::*;

fn request(category: FeedbackCategory) -> IngestRequest {
    IngestRequest {
        category,
        title: "[bug] it broke".to_string(),
        body: "**Category:** bug\n\nit broke\n\n— filed by @acme".to_string(),
        origin: "acme".to_string(),
        external_ref: "item-1".to_string(),
    }
}

#[test]
fn maps_categories_onto_the_hub_type_pair() {
    // Something the product did wrong.
    for category in [FeedbackCategory::Bug, FeedbackCategory::WrongOutput] {
        assert_eq!(request(category).wire_type(), "bug", "{category:?}");
    }
    // Something the product does not do yet.
    for category in [
        FeedbackCategory::MissingCapability,
        FeedbackCategory::TemplateGap,
        FeedbackCategory::ApprovalFriction,
        FeedbackCategory::Docs,
    ] {
        assert_eq!(request(category).wire_type(), "feature", "{category:?}");
    }
}

#[test]
fn product_is_always_opencompany() {
    assert_eq!(PRODUCT, "opencompany");
}

#[tokio::test]
async fn mock_records_the_forwarded_request() {
    let client = MockTinyHumansClient::new();
    let outcome = client
        .ingest(&request(FeedbackCategory::Bug))
        .await
        .unwrap();
    assert_eq!(
        outcome,
        IngestOutcome::Accepted {
            remote_id: Some("hub-1".to_string())
        }
    );
    let forwarded = client.forwarded();
    assert_eq!(forwarded.len(), 1);
    assert_eq!(forwarded[0].external_ref, "item-1");
    assert_eq!(forwarded[0].origin, "acme");
}

#[tokio::test]
async fn mock_can_reject_and_fail() {
    let rejected = MockTinyHumansClient::new().with_outcome(IngestOutcome::Rejected {
        reason: "spam".to_string(),
    });
    assert_eq!(
        rejected
            .ingest(&request(FeedbackCategory::Bug))
            .await
            .unwrap(),
        IngestOutcome::Rejected {
            reason: "spam".to_string()
        }
    );

    let failing = MockTinyHumansClient::new().with_failure("connection refused");
    assert!(
        failing
            .ingest(&request(FeedbackCategory::Bug))
            .await
            .is_err()
    );
    // The attempt is still recorded, so a test can assert what we tried to send.
    assert_eq!(failing.forwarded().len(), 1);
}
