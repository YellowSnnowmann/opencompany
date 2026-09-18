use super::*;

/// A run admitted before the switch was pulled must not keep making
/// requests. Admission is taken once for the whole run, so the request
/// itself has to ask.
///
/// Refused before the URL is even resolved: the point is that no packet
/// leaves, not that a bad one is rejected.
#[tokio::test]
async fn a_stopped_company_refuses_a_node_http_request() {
    use tinyflows::caps::HttpClient;
    let gate = Arc::new(crate::policy::gate::ManifestApprovalGate::new(
        crate::company::Policy {
            mode: "full".to_string(),
            always_approve: Vec::new(),
            auto_approve_under_usd: None,
            approval_ttl_hours: None,
        },
    ));
    let client = GuardedHttpClient::new(Arc::new(SecurityPolicy::default()), Vec::new())
        .with_emergency_gate(Some(gate.clone()));

    gate.set_emergency(true);
    let refused = client
        .request(
            json!({ "method": "GET", "url": "https://api.test/x" }),
            None,
        )
        .await
        .expect_err("a stopped company must make no request");
    assert!(
        matches!(refused, EngineError::Capability(ref m) if m.contains("is stopped")),
        "{refused:?}"
    );

    gate.set_emergency(false);
    let allowed = client
        .request(
            json!({ "method": "GET", "url": "https://api.test/x" }),
            None,
        )
        .await;
    assert!(
        !matches!(allowed, Err(EngineError::Capability(ref m)) if m.contains("is stopped")),
        "released, so the stop no longer refuses it: {allowed:?}"
    );
}

#[test]
fn to_tool_args_maps_method_url_headers_and_stringifies_body() {
    let descriptor = json!({
        "method": "POST",
        "url": "https://api.test/x",
        "headers": { "Content-Type": "application/json" },
        "body": { "q": "hi" }
    });
    let args = to_tool_args(&descriptor);
    assert_eq!(args["method"], "POST");
    assert_eq!(args["url"], "https://api.test/x");
    assert_eq!(args["headers"]["Content-Type"], "application/json");
    // A structured body is carried as its JSON string encoding.
    assert_eq!(args["body"], json!("{\"q\":\"hi\"}"));

    // A string body passes through unchanged; a missing body is omitted.
    let str_body = to_tool_args(&json!({ "url": "u", "body": "raw" }));
    assert_eq!(str_body["body"], "raw");
    let no_body = to_tool_args(&json!({ "url": "u" }));
    assert!(no_body.get("body").is_none());
}

#[test]
fn parse_http_output_extracts_status_and_body() {
    let output =
        "Status: 200 OK\nResponse Headers: content-type: json\n\nResponse Body:\n{\"ok\":true}";
    let (status, body) = parse_http_output(output);
    assert_eq!(status, json!(200));
    assert_eq!(body, "{\"ok\":true}");
}

#[test]
fn from_tool_result_maps_success_and_error() {
    let ok = from_tool_result(ToolResult::success(
        "Status: 201 Created\n\nResponse Body:\nhi",
    ))
    .unwrap();
    assert_eq!(ok["status"], 201);
    assert_eq!(ok["body"], "hi");

    let err = from_tool_result(ToolResult::error("URL is not allowed: 127.0.0.1")).unwrap_err();
    assert!(
        matches!(err, EngineError::Capability(ref m) if m.contains("127.0.0.1")),
        "{err:?}"
    );
}
