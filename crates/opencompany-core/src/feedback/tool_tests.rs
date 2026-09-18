use super::*;
use crate::feedback::types::ConsentMode;
use crate::runtime::tools::StubToolProvider;
use crate::store::FsEventLog;
use crate::store::paths::Bundle;

/// The caller must hold the returned handle: it owns the bundle root and
/// removes it on drop.
fn wiring() -> (BuiltinToolProvider, Arc<FeedbackStore>, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("oc-btool-")
        .tempdir()
        .expect("tempdir");
    let root = dir.path().to_path_buf();
    let bundle = Bundle::new(root.clone(), &CompanyId::new("acme"));
    let feedback = Arc::new(FeedbackStore::new(&bundle));
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(root.clone()));
    let inner: Arc<dyn ToolProvider> = Arc::new(StubToolProvider::new(vec!["email.*".into()]));
    let provider = BuiltinToolProvider::new(inner, feedback.clone(), events, ConsentMode::Manual);
    (provider, feedback, dir)
}

#[tokio::test]
async fn catalog_includes_feedback_tool() {
    let (provider, _fb, _root) = wiring();
    let catalog = provider.catalog(&CompanyId::new("acme")).await.unwrap();
    assert!(catalog.iter().any(|t| t.name == FEEDBACK_TOOL));
}

#[tokio::test]
async fn catalog_advertises_send_email() {
    let (provider, _fb, _root) = wiring();
    let specs = provider.catalog(&CompanyId::new("acme")).await.unwrap();
    let spec = specs
        .iter()
        .find(|s| s.name == "send_email")
        .expect("send_email advertised");
    let req = &spec.input_schema["required"];
    assert!(req.as_array().unwrap().iter().any(|v| v == "to"));
}

#[tokio::test]
async fn feedback_tool_captures_without_grant() {
    let (provider, feedback, _root) = wiring();
    let result = provider
        .invoke(
            &CompanyId::new("acme"),
            ToolCall {
                tool: FEEDBACK_TOOL.into(),
                args: serde_json::json!({ "category": "bug", "note": "route broke" }),
            },
        )
        .await
        .unwrap();
    assert!(result.ok);
    // The item was persisted even though `feedback` is not in the grant.
    let items = feedback.list().await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].operator_words, "route broke");
    assert_eq!(items[0].category, FeedbackCategory::Bug);
}

#[tokio::test]
async fn non_feedback_tool_delegates_and_enforces_grants() {
    let (provider, _fb, _root) = wiring();
    // Ungranted tool is still rejected by the inner provider.
    let err = provider
        .invoke(
            &CompanyId::new("acme"),
            ToolCall {
                tool: "payment.send".into(),
                args: serde_json::Value::Null,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, crate::OpenCompanyError::ToolNotGranted(t) if t == "payment.send"));
}
