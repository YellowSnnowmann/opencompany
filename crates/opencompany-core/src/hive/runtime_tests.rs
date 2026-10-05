//! The company hive without live agents: one hive per company and store,
//! and the desks synced as hives.

use std::sync::Arc;

use super::*;
use crate::store::{FsEventLog, MemoryHiveStore};

fn record() -> CompanyRecord {
    let mut record = CompanyRecord::from_manifest(
        CompanyId::new(format!("acme-{}", uuid::Uuid::new_v4().simple())),
        toml::from_str(
            r#"
[company]
name = "Acme"

[[agent]]
id = "writer"
role = "Writer"

[[group_chat]]
id = "content"
name = "Content"
members = ["writer"]
"#,
        )
        .expect("manifest"),
    );
    record.general_channel.members = vec!["writer".into()];
    record
}

fn config(
    record: &CompanyRecord,
    store: Arc<dyn HiveStore>,
    events: Arc<dyn EventLog>,
) -> HiveConfig {
    HiveConfig {
        company: record.id.clone(),
        runtime_id: "rt-test".into(),
        store,
        events,
        options: crate::hive::routing::coordinator_options(record),
        turn_timeout: crate::hive::routing::turn_timeout(record),
    }
}

#[tokio::test]
async fn one_hive_per_company_and_store() {
    let dir = tempfile::tempdir().unwrap();
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let record = record();
    let store: Arc<dyn HiveStore> = Arc::new(MemoryHiveStore::new());
    let first = for_company(config(&record, Arc::clone(&store), Arc::clone(&events)))
        .await
        .expect("hive");
    let again = for_company(config(&record, Arc::clone(&store), Arc::clone(&events)))
        .await
        .expect("hive");
    assert!(Arc::ptr_eq(&first, &again), "the same store shares one hive");
    let other = for_company(config(
        &record,
        Arc::new(MemoryHiveStore::new()),
        Arc::clone(&events),
    ))
    .await
    .expect("hive");
    assert!(!Arc::ptr_eq(&first, &other), "another store is another hive");
}

#[tokio::test]
async fn sync_creates_a_hive_per_desk_and_general() {
    let dir = tempfile::tempdir().unwrap();
    let events: Arc<dyn EventLog> = Arc::new(FsEventLog::new(dir.path()));
    let record = record();
    let hive = for_company(config(&record, Arc::new(MemoryHiveStore::new()), events))
        .await
        .expect("hive");
    hive.sync(&record).await.expect("sync");
    let mut hives: Vec<String> = hive
        .coordinator()
        .list_hives()
        .unwrap()
        .into_iter()
        .map(|info| info.hive_id)
        .collect();
    hives.sort();
    assert_eq!(hives, vec!["General".to_string(), "content".to_string()]);
    hive.sync(&record).await.expect("a second sync is a no-op");
    assert!(
        !hive.release("writer", None).await.expect("release"),
        "an unregistered agent is not released"
    );
    assert_eq!(hive.coordinator_id("writer"), None);
}
