//! `PortStorage` drives a real Coordinator over the in-memory `HiveStore`:
//! state and transcript survive a reload, a stale writer conflicts, and a
//! transcript row written through the port decodes back into the state.

use std::sync::Arc;

use tinyhivemind_hives::{
    Coordinator, CoordinatorOptions, Destination, HiveInfo, Message, SendMessage,
};

use super::*;
use crate::store::MemoryHiveStore;

fn company() -> CompanyId {
    CompanyId::new("acme")
}

fn host_message(id: &str, body: &str) -> SendMessage {
    SendMessage {
        message_id: id.to_string(),
        sender: String::new(),
        destination: Destination::Hive("ops".to_string()),
        body: body.to_string(),
        thread: None,
        only_for: Vec::new(),
        starters: Vec::new(),
    }
}

#[test]
fn revisions_round_trip_through_the_store_key() {
    assert_eq!(revision_key(0), None);
    assert_eq!(revision_key(7).as_deref(), Some("7"));
    assert_eq!(revision_of(None), 0);
    assert_eq!(revision_of(Some("7")), 7);
    assert_eq!(revision_of(Some("not-a-number")), 0);
}

#[tokio::test]
async fn an_empty_store_loads_the_initial_state() {
    let storage = PortStorage::new(company(), Arc::new(MemoryHiveStore::new()));
    let state = storage.load().await.expect("load");
    assert_eq!(state.revision, 0);
    assert!(state.messages.is_empty());
}

#[tokio::test]
async fn a_coordinator_reloads_its_hives_and_transcript() {
    let store: Arc<dyn HiveStore> = Arc::new(MemoryHiveStore::new());
    let storage = Arc::new(PortStorage::new(company(), Arc::clone(&store)));
    let coordinator = Coordinator::new("rt".into(), storage, CoordinatorOptions::default())
        .await
        .expect("coordinator");
    coordinator
        .create_hive(HiveInfo {
            hive_id: "ops".into(),
            name: "Ops".into(),
            description: None,
            members: Vec::new(),
        })
        .await
        .expect("hive");
    let error = coordinator
        .send_as_host(host_message("op:1", "hello"))
        .await
        .expect_err("an empty hive has nobody to read the message");
    assert!(error.to_string().contains("empty hive"), "{error}");

    let reloaded = PortStorage::new(company(), store)
        .load()
        .await
        .expect("load");
    assert_eq!(reloaded.hives.len(), 1);
    assert!(reloaded.revision >= 1);
}

#[tokio::test]
async fn a_stale_writer_conflicts_and_changes_nothing() {
    let storage = PortStorage::new(company(), Arc::new(MemoryHiveStore::new()));
    let mut next = StoredState {
        revision: 1,
        next_sequence: 1,
        ..StoredState::default()
    };
    let row = TranscriptRow {
        message: Message {
            message_id: "m".into(),
            sequence: 0,
            sender: "a".into(),
            destination: Destination::Agent("b".into()),
            body: "hi".into(),
            thread: None,
            episode_id: None,
            only_for: Vec::new(),
        },
        accepted: None,
    };
    storage
        .commit(Commit {
            expected_revision: 0,
            state: &next,
            appended: std::slice::from_ref(&row),
        })
        .await
        .expect("first commit");
    next.revision = 1;
    let error = storage
        .commit(Commit {
            expected_revision: 0,
            state: &next,
            appended: &[],
        })
        .await
        .expect_err("a writer still at revision 0 is stale");
    assert!(
        matches!(
            error,
            HiveError::RevisionConflict {
                expected: 0,
                actual: 1
            }
        ),
        "{error:?}"
    );
    let state = storage.load().await.expect("load");
    assert_eq!(state.revision, 1);
    assert_eq!(state.messages.len(), 1);
    assert_eq!(state.messages[0].body, "hi");
}

#[tokio::test]
async fn a_seeded_transcript_row_decodes_into_the_state() {
    let store: Arc<dyn HiveStore> = Arc::new(MemoryHiveStore::new());
    let state = StoredState {
        revision: 1,
        next_sequence: 1,
        ..StoredState::default()
    };
    let rows = vec![HiveMessageRow {
        sequence: 0,
        body: serde_json::to_value(TranscriptRow {
            message: Message {
                message_id: "kept".into(),
                sequence: 0,
                sender: "a".into(),
                destination: Destination::Agent("b".into()),
                body: "kept".into(),
                thread: None,
                episode_id: None,
                only_for: Vec::new(),
            },
            accepted: None,
        })
        .unwrap(),
    }];
    store
        .commit_hive(
            &company(),
            None,
            HiveStateDoc {
                revision: "1".into(),
                next_sequence: 1,
                body: serde_json::to_value(&state).unwrap(),
            },
            rows,
        )
        .await
        .expect("seed");
    let loaded = PortStorage::new(company(), store)
        .load()
        .await
        .expect("load");
    assert_eq!(loaded.messages.len(), 1);
    assert_eq!(loaded.messages[0].message_id, "kept");
}
