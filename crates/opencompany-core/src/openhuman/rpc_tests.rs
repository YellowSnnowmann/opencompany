use super::*;

#[test]
fn rpc_method_formats_namespace_and_function() {
    assert_eq!(rpc_method("tools", "invoke"), "openhuman.tools_invoke");
    assert_eq!(rpc_method("channels", "send"), "openhuman.channels_send");
}

#[test]
fn response_envelope_defaults_missing_fields() {
    let ok: RpcResponse = serde_json::from_str(r#"{"result": {"ok": true}}"#).unwrap();
    assert!(ok.error.is_none());
    assert_eq!(ok.result["ok"], true);

    let err: RpcResponse =
        serde_json::from_str(r#"{"error": {"code": -1, "message": "boom"}}"#).unwrap();
    assert!(err.result.is_null());
    assert_eq!(err.error.unwrap().message, "boom");
}

#[tokio::test]
async fn mock_returns_registered_result_and_records_calls() {
    let rpc = MockOpenHumanRpc::new().with_result("openhuman.tools_list", serde_json::json!([]));
    let out = rpc
        .call("openhuman.tools_list", serde_json::json!({}))
        .await
        .unwrap();
    assert!(out.is_array());
    assert_eq!(rpc.call_count(), 1);
    assert_eq!(rpc.calls()[0].0, "openhuman.tools_list");
}

#[tokio::test]
async fn mock_unknown_method_errors() {
    let rpc = MockOpenHumanRpc::new();
    let err = rpc
        .call("openhuman.tools_list", serde_json::Value::Null)
        .await
        .unwrap_err();
    assert!(matches!(err, crate::OpenCompanyError::OpenHuman { code, .. } if code == -32601));
}

#[tokio::test]
async fn mock_health_reflects_flag() {
    assert!(MockOpenHumanRpc::new().health().await.unwrap());
    assert!(!MockOpenHumanRpc::new().unhealthy().health().await.unwrap());
}
