use super::*;

#[test]
fn a_plain_company_id_is_its_own_root() {
    assert_eq!(memory_root(&CompanyId::new("acme")), "team:acme");
    assert_eq!(
        memory_root(&CompanyId::new("tenant--acme_2")),
        "team:tenant--acme_2"
    );
}

#[test]
fn an_id_outside_the_namespace_charset_is_folded_and_hashed() {
    let root = memory_root(&CompanyId::new("acme corp.io"));
    assert!(root.starts_with("team:acme-corp-io-"), "{root}");
    assert_eq!(root.len(), "team:acme-corp-io-".len() + 8);
    assert_ne!(root, memory_root(&CompanyId::new("acme corp io")));
    assert_eq!(memory_root(&CompanyId::new("")), "team:_");
}

#[test]
fn a_long_id_stays_inside_one_segment() {
    let long = "a".repeat(300);
    let root = memory_root(&CompanyId::new(long));
    assert!(root.len() <= "team:".len() + 128, "{root}");
}

#[test]
fn the_handle_carries_its_company_and_root() {
    let memory = CompanyMemory::new(&CompanyId::new("acme"));
    assert_eq!(memory.company().as_ref(), "acme");
    assert_eq!(memory.root(), "team:acme");
}

#[cfg(not(feature = "openhuman"))]
#[tokio::test]
async fn without_the_runtime_memory_is_off_and_says_why() {
    let memory = CompanyMemory::new(&CompanyId::new("acme"));
    let status = memory.status().await;
    assert!(!status.on);
    assert!(status.reason.unwrap().contains("openhuman"));
    assert!(matches!(
        memory.list(MemoryQuery::default()).await,
        Err(OpenCompanyError::NotInBuild(_))
    ));
}
