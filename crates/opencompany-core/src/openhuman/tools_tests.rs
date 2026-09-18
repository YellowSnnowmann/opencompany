use super::*;
use crate::openhuman::rpc::MockOpenHumanRpc;
use crate::runtime::tools::StubToolProvider;

fn company() -> CompanyId {
    CompanyId::new("acme")
}

fn spec(name: &str) -> serde_json::Value {
    serde_json::json!({ "name": name, "description": "", "input_schema": {} })
}

#[tokio::test]
async fn catalog_filters_by_grants() {
    let rpc = Arc::new(MockOpenHumanRpc::new().with_result(
        "openhuman.tools_list",
        serde_json::json!([spec("email.send"), spec("payment.send")]),
    ));
    let provider = OpenHumanToolProvider::new(
        rpc,
        vec!["email.*".into()],
        Arc::new(StubToolProvider::new(vec!["email.*".into()])),
    );
    let catalog = provider.catalog(&company()).await.unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].name, "email.send");
}

#[tokio::test]
async fn ungranted_invoke_rejected_before_rpc() {
    let rpc = Arc::new(MockOpenHumanRpc::new());
    let provider = OpenHumanToolProvider::new(
        rpc.clone(),
        vec!["email.*".into()],
        Arc::new(StubToolProvider::new(vec!["email.*".into()])),
    );
    let err = provider
        .invoke(
            &company(),
            ToolCall {
                tool: "payment.send".into(),
                args: serde_json::Value::Null,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, OpenCompanyError::ToolNotGranted(t) if t == "payment.send"));
    // Rejection must be side-effect-free: no RPC issued.
    assert_eq!(rpc.call_count(), 0);
}

#[tokio::test]
async fn granted_invoke_decodes_tool_result() {
    let rpc = Arc::new(MockOpenHumanRpc::new().with_result(
        "openhuman.tools_invoke",
        serde_json::json!({ "ok": true, "output": { "id": "msg_1" } }),
    ));
    let provider = OpenHumanToolProvider::new(
        rpc.clone(),
        vec!["email.*".into()],
        Arc::new(StubToolProvider::new(vec!["email.*".into()])),
    );
    let result = provider
        .invoke(
            &company(),
            ToolCall {
                tool: "email.send".into(),
                args: serde_json::json!({ "to": "a@b.c" }),
            },
        )
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(result.output["id"], "msg_1");
    assert_eq!(rpc.call_count(), 1);
}

#[tokio::test]
async fn granted_invoke_rpc_failure_is_well_formed() {
    // No handler registered → the mock errors, standing in for an RPC failure.
    let rpc = Arc::new(MockOpenHumanRpc::new());
    let provider = OpenHumanToolProvider::new(
        rpc,
        vec!["email.*".into()],
        Arc::new(StubToolProvider::new(vec!["email.*".into()])),
    );
    let result = provider
        .invoke(
            &company(),
            ToolCall {
                tool: "email.send".into(),
                args: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();
    assert!(!result.ok);
    assert_eq!(result.output["error"], "openhuman rpc failed");
}

#[tokio::test]
async fn rpc_failure_falls_back_to_builtin_catalog() {
    // Unhealthy/erroring mock (no tools_list handler) → the stub's catalog.
    let rpc = Arc::new(MockOpenHumanRpc::new().unhealthy());
    let provider = OpenHumanToolProvider::new(
        rpc,
        vec!["email.*".into()],
        Arc::new(StubToolProvider::new(vec!["email.*".into()])),
    );
    let catalog = provider.catalog(&company()).await.unwrap();
    assert!(catalog.is_empty());
}
