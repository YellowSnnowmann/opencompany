use super::*;
use crate::acp::confine::Confinement;

fn auto_approving(root: &std::path::Path) -> AutoApprovingFiles<ConfinedFiles> {
    AutoApprovingFiles::new(ConfinedFiles::new(Confinement::new(root).unwrap(), None))
}

#[tokio::test]
async fn falls_back_to_reject_once_when_the_agent_offers_no_allow_option() {
    let dir = tempfile::tempdir().unwrap();
    let handler = auto_approving(dir.path());
    let options = json!([
        { "optionId": "n1", "name": "Reject", "kind": "reject_once" },
        { "optionId": "n2", "name": "Reject always", "kind": "reject_always" },
    ]);

    assert_eq!(
        handler.request_permission(&Value::Null, &options).await,
        "n1"
    );
}

#[tokio::test]
async fn falls_back_to_the_literal_reject_when_the_agent_offers_nothing_to_pick() {
    let dir = tempfile::tempdir().unwrap();
    let handler = auto_approving(dir.path());

    assert_eq!(
        handler.request_permission(&Value::Null, &json!([])).await,
        "reject"
    );
}
