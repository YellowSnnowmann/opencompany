/// A real empty `finish_reason: length` response crosses the HTTP provider
/// adapter and setup route, so the first-run wizard receives its curated team
/// with the output-budget fallback reason.
#[cfg(feature = "openhuman")]
#[tokio::test]
async fn a_length_stopped_provider_reply_is_reported_to_setup_as_output_budget_exhausted() {
    use super::setup_test_support_1::*;
    use axum::http::StatusCode;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let home = home();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "finish_reason": "length",
                "message": {"role": "assistant", "content": ""}
            }],
            "usage": {"prompt_tokens": 100, "completion_tokens": 4_000, "total_tokens": 4_100}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let (status, body) = post_roster(
        fresh_state(home.path()),
        serde_json::json!({
            "industry": "E-commerce — I sell homeware online",
            "automate": "Meta ads, order dispatch",
            "inferenceProvider": "openai_compatible",
            "inferenceBaseUrl": server.uri(),
            "inferenceModel": "test-model"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["source"], "fallback", "{body}");
    assert_eq!(body["reason"], "output_budget_exhausted", "{body}");
    assert!(
        body["agents"]
            .as_array()
            .is_some_and(|agents| !agents.is_empty()),
        "the setup screen must receive a usable curated team: {body}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
