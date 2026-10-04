use super::*;
use crate::ports::CompanyId;

/// The process-wide runtime, which a test build boots with TinyMemory's
/// in-memory reference engine installed (`openhuman_runtime::build`).
async fn memory(company: &str) -> CompanyMemory {
    openhuman_runtime::global(RuntimeBoot::ephemeral())
        .await
        .expect("runtime");
    CompanyMemory::new(&CompanyId::new(format!(
        "{company}-{}",
        uuid::Uuid::new_v4().simple()
    )))
}

#[test]
fn the_root_matches_tinymemorys_own_sanitizing() {
    for raw in ["acme", "acme corp.io", "", "tenant--acme"] {
        let ours = super::super::memory_root(&CompanyId::new(raw));
        let theirs = format!("team:{}", tm::Segment::sanitized(SegmentKind::Team, raw).id());
        assert_eq!(ours, theirs, "{raw:?}");
    }
}

#[tokio::test]
async fn a_learning_lands_at_the_company_root_and_is_listed() {
    let memory = memory("learn").await;
    assert!(memory.status().await.on);
    let row = memory
        .learn("Invoices go out on the 1st", LearningKind::Procedure, vec!["operator".into()])
        .await
        .expect("learn");
    assert_eq!(row.kind, MemoryItemKind::Learning);
    assert_eq!(row.namespace, memory.root());
    assert_eq!(row.title, "Invoices go out on the 1st");
    assert!(row.tags.contains(&"operator".to_string()));

    let page = memory
        .list(MemoryQuery {
            kind: Some(MemoryItemKind::Learning),
            ..MemoryQuery::default()
        })
        .await
        .expect("list");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, row.id);
    let tagged = memory
        .list(MemoryQuery {
            tags_any: vec!["operator".into()],
            ..MemoryQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(tagged.items.len(), 1);
}

#[tokio::test]
async fn one_company_never_reaches_anothers_memory() {
    let acme = memory("acme").await;
    let globex = memory("globex").await;
    let row = acme
        .learn("Acme's secret sauce", LearningKind::Fact, Vec::new())
        .await
        .unwrap();
    assert!(globex.get(vec![row.id.clone()]).await.unwrap().is_empty());
    assert!(globex.list(MemoryQuery::default()).await.unwrap().items.is_empty());
    assert_eq!(globex.forget(vec![row.id.clone()]).await.unwrap(), 0);
    assert_eq!(acme.get(vec![row.id.clone()]).await.unwrap().len(), 1);
    assert_eq!(acme.forget(vec![row.id.clone()]).await.unwrap(), 1);
    assert!(acme.get(vec![row.id]).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_filed_document_is_a_brain_source() {
    let memory = memory("brain").await;
    let filed = memory
        .brain_file("Handbook", "markdown", "# Handbook\nBe kind.")
        .await
        .expect("file");
    assert_eq!(filed.source, "markdown");
    let sources = memory.brain_sources().await.expect("sources");
    assert_eq!(sources.root, memory.root());
    assert_eq!(sources.sources.len(), 1);
    assert_eq!(sources.sources[0].source, "markdown");
    let docs = memory
        .list(MemoryQuery {
            kind: Some(MemoryItemKind::Document),
            ..MemoryQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(docs.items[0].source.as_deref(), Some("markdown"));
    assert_eq!(memory.brain_forget("markdown").await.unwrap(), 1);
}

#[test]
fn hits_map_their_agent_and_source_from_the_namespace() {
    let meta = tm::MemoryMeta {
        namespace: "team:acme/agent:ceo".parse().unwrap(),
        ..tm::MemoryMeta::default()
    };
    let row = item(tm::Hit {
        id: tm::ItemId("1".into()),
        kind: ItemKind::Conversation,
        text: "\n  user: hi\nassistant: hello".into(),
        meta,
        score: 0.0,
        confidence: None,
    });
    assert_eq!(row.agent_id.as_deref(), Some("ceo"));
    assert_eq!(row.source, None);
    assert_eq!(row.title, "user: hi");
    assert_eq!(row.updated_at, 0);
}

#[test]
fn an_off_engine_is_a_configuration_not_a_failure() {
    assert!(matches!(
        error(MemoryError::Off("signed out".into())),
        OpenCompanyError::NotConfigured(_)
    ));
    assert!(matches!(
        error(MemoryError::InvalidRequest("bad".into())),
        OpenCompanyError::InvalidRequest(_)
    ));
    assert!(matches!(
        error(MemoryError::Engine("down".into())),
        OpenCompanyError::Store(_)
    ));
}
