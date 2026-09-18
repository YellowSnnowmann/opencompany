use super::*;
use crate::ports::ContextStore;
use crate::ports::types::{CompanyId, ContextChunk};
use crate::store::FsContextStore;
use std::sync::Arc;

/// Deleting a fact reaps its mirror. A mirror whose content is shared
/// with another label loses exactly the mirror's own claim (label-scoped
/// delete, #1300): the other label keeps the body, and — unlike the old
/// shared-address skip — the mirror row itself no longer lingers in the
/// index serving a deleted fact.
#[tokio::test]
async fn the_mirror_is_reaped_and_a_shared_body_survives_under_its_other_label() {
    let dir = tempfile::tempdir().unwrap();
    let context: Arc<dyn ContextStore> = Arc::new(FsContextStore::new(dir.path().to_path_buf()));
    let company = CompanyId::new("acme");

    let lone = context
        .put(
            &company,
            ContextChunk {
                label: "operator-fact/f1".into(),
                body: "fact one".into(),
            },
        )
        .await
        .unwrap();
    reap_fact_mirror(context.as_ref(), &company, "f1")
        .await
        .unwrap();
    assert!(
        context.peek(&company, &lone, None).await.is_err(),
        "the unshared mirror must be reaped"
    );

    // Identical bodies share one address: the reap removes the mirror's
    // claim, and the agent's row keeps the body.
    let shared = context
        .put(
            &company,
            ContextChunk {
                label: "operator-fact/f2".into(),
                body: "shared text".into(),
            },
        )
        .await
        .unwrap();
    context
        .put(
            &company,
            ContextChunk {
                label: "agent-memory/ceo/note".into(),
                body: "shared text".into(),
            },
        )
        .await
        .unwrap();
    reap_fact_mirror(context.as_ref(), &company, "f2")
        .await
        .unwrap();
    context
        .peek(&company, &shared, None)
        .await
        .expect("the body must survive under the agent's label");
    let labels: Vec<String> = context
        .list(&company, "")
        .await
        .unwrap()
        .into_iter()
        .filter(|m| m.addr == shared)
        .map(|m| m.label)
        .collect();
    assert_eq!(
        labels,
        ["agent-memory/ceo/note"],
        "the mirror's claim must be gone; only the agent's remains"
    );
}
