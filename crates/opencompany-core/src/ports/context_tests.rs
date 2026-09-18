use super::*;
use std::sync::Mutex;

/// A backend that implements the port as it stood BEFORE label-scoped
/// delete existed — no `delete_label` override. This is the shape an
/// operator-supplied store injected through `RuntimeBuilder::with_context`
/// has, and the reason the method is defaulted rather than required.
#[derive(Default)]
struct LegacyStore {
    rows: Mutex<Vec<(ChunkAddr, String)>>,
}

#[async_trait]
impl ContextStore for LegacyStore {
    async fn put(&self, _: &CompanyId, chunk: ContextChunk) -> Result<ChunkAddr> {
        let addr = ChunkAddr::new(crate::store::content_address(&chunk.body));
        self.rows.lock().unwrap().push((addr.clone(), chunk.label));
        Ok(addr)
    }
    async fn list(&self, _: &CompanyId, prefix: &str) -> Result<Vec<ChunkMeta>> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, label)| label.starts_with(prefix))
            .map(|(addr, label)| ChunkMeta {
                addr: addr.clone(),
                label: label.clone(),
                len: 0,
                stored_at_millis: 0,
            })
            .collect())
    }
    async fn peek(&self, _: &CompanyId, _: &ChunkAddr, _: Option<Range<usize>>) -> Result<String> {
        Ok(String::new())
    }
    async fn search(&self, _: &CompanyId, _: &str, _: usize) -> Result<Vec<ChunkHit>> {
        Ok(Vec::new())
    }
    async fn delete(&self, _: &CompanyId, addr: &ChunkAddr) -> Result<bool> {
        let mut rows = self.rows.lock().unwrap();
        let before = rows.len();
        rows.retain(|(have, _)| have != addr);
        Ok(rows.len() < before)
    }
}

fn company() -> CompanyId {
    CompanyId::new("acme")
}

async fn put(store: &LegacyStore, label: &str, body: &str) -> ChunkAddr {
    store
        .put(
            &company(),
            ContextChunk {
                label: label.to_string(),
                body: body.to_string(),
            },
        )
        .await
        .unwrap()
}

/// The sole claim is removed with its body — the same outcome an
/// overriding backend gives, so a legacy store keeps working.
#[tokio::test]
async fn the_default_removes_a_sole_claim() {
    let store = LegacyStore::default();
    let addr = put(&store, "notes/one", "body").await;
    assert!(
        store
            .delete_label(&company(), &addr, "notes/one")
            .await
            .unwrap()
    );
    assert!(store.list(&company(), "").await.unwrap().is_empty());
}

/// A pairing that is not there answers `false`, not an error — the
/// idempotent-retry contract every backend shares.
#[tokio::test]
async fn the_default_answers_false_for_an_absent_pairing() {
    let store = LegacyStore::default();
    let addr = put(&store, "notes/one", "body").await;
    assert!(
        !store
            .delete_label(&company(), &addr, "notes/other")
            .await
            .unwrap()
    );
    assert_eq!(store.list(&company(), "").await.unwrap().len(), 1);
}

/// The case the griefing vector lives in: a shared address REFUSES rather
/// than taking the other label's row with it. Never worse than the
/// pre-#1300 callers, which refused here too — and emphatically not the
/// over-delete a `delete`-shaped default would have produced.
#[tokio::test]
async fn the_default_refuses_a_shared_address_rather_than_over_deleting() {
    let store = LegacyStore::default();
    let addr = put(&store, "notes/mine", "same body").await;
    assert_eq!(put(&store, "notes/theirs", "same body").await, addr);

    let refused = store.delete_label(&company(), &addr, "notes/mine").await;
    assert!(
        refused.is_err(),
        "a shared address must refuse: {refused:?}"
    );
    assert_eq!(
        store.list(&company(), "").await.unwrap().len(),
        2,
        "neither claim may be removed by a refusal"
    );
}
