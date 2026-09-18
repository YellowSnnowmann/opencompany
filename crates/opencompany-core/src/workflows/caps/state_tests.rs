use super::*;
use crate::store::FsSecretStore;
use serde_json::json;

#[tokio::test]
async fn roundtrips_and_isolates_across_workflows() {
    let dir = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(FsSecretStore::new(dir.path().to_path_buf()));
    let company = CompanyId::new("acme");

    let wf_a = CompanyStateStore::new(secrets.clone(), company.clone(), "a".to_string());
    let wf_b = CompanyStateStore::new(secrets.clone(), company.clone(), "b".to_string());

    wf_a.store("cursor", json!({ "page": 2 })).await.unwrap();
    // Same key, different workflow → independent slot.
    assert_eq!(
        wf_a.load("cursor").await.unwrap(),
        Some(json!({ "page": 2 }))
    );
    assert_eq!(wf_b.load("cursor").await.unwrap(), None);

    wf_b.store("cursor", json!({ "page": 9 })).await.unwrap();
    assert_eq!(
        wf_a.load("cursor").await.unwrap(),
        Some(json!({ "page": 2 }))
    );
    assert_eq!(
        wf_b.load("cursor").await.unwrap(),
        Some(json!({ "page": 9 }))
    );
}

#[test]
fn namespace_is_unambiguous_when_segments_contain_colons() {
    let dir = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(FsSecretStore::new(dir.path().to_path_buf()));
    let company = CompanyId::new("acme");
    let left = CompanyStateStore::new(secrets.clone(), company.clone(), "workflow:a".to_string());
    let right = CompanyStateStore::new(secrets, company, "workflow".to_string());

    assert_ne!(left.namespaced("cursor"), right.namespaced("a:cursor"));
}

#[tokio::test]
async fn noop_state_reads_none_and_drops_writes() {
    NoopState.store("k", json!(1)).await.unwrap();
    assert_eq!(NoopState.load("k").await.unwrap(), None);
}
