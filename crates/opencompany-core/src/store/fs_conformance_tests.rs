use super::tests::tmp_root;
use super::*;
use crate::store::conformance;

#[tokio::test]
async fn conformance_paused_ordinary_save_preserves_activation_gate() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_paused_ordinary_save_preserves_activation_gate(Arc::new(
        FsCompanyStore::new(&root),
    ))
    .await;
}

#[tokio::test]
async fn conformance_append_only_event_and_ledger() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_append_only_event_and_ledger(
        Arc::new(FsCompanyStore::new(&root)),
        Arc::new(FsEventLog::new(&root)),
    )
    .await;
}

#[tokio::test]
async fn conformance_monotonic_event_seq() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_monotonic_event_seq(Arc::new(FsEventLog::new(&root))).await;
}

#[tokio::test]
async fn conformance_event_subscription_surfaces_gap() {
    let root_dir = tmp_root();
    conformance::assert_event_subscription_surfaces_gap(Arc::new(FsEventLog::new(root_dir.path())))
        .await;
}

#[tokio::test]
async fn conformance_event_read_before() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_event_read_before(Arc::new(FsEventLog::new(&root))).await;
}

#[tokio::test]
async fn conformance_event_retention() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_event_retention(Arc::new(FsEventLog::new(&root))).await;
}

#[tokio::test]
async fn conformance_inbox_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_inbox_store(Arc::new(FsInboxStore::new(&root))).await;
}

/// Issue #1505. The port holds this company's inference credential, its MCP
/// OAuth tokens and its SMTP password, and had no conformance case on any
/// backend until this one.
#[tokio::test]
async fn conformance_secret_store() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_secret_store(Arc::new(FsSecretStore::new(&root))).await;
}

/// Two event logs over one data root must not hand out the same sequence
/// number (issue #388).
///
/// `EventLog::append` computes the next `seq` by counting the lines already
/// in the file, then appends. That read-then-append is only atomic under a
/// lock, and the lock used to be a **field** on `FsEventLog` — so two
/// instances over one bundle serialised against nothing, both read the same
/// count, and both wrote the same `seq`. A duplicate sequence number breaks
/// every consumer that treats it as an identity: `read_from`'s `seq >=`
/// cursor silently replays, and the console's resume-from-seq skips.
///
/// Nothing stops a second instance being constructed — `FsEventLog::new`
/// takes a root and is called wherever one is needed — so this is reachable
/// without any exotic setup, which is exactly what makes it worth a lock in
/// the registry rather than a convention.
#[tokio::test]
async fn two_event_logs_over_one_root_never_reuse_a_sequence() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    // Two independently-constructed logs over the same data root — the shape
    // a second runtime, a maintenance task, or an export job produces.
    let first = Arc::new(FsEventLog::new(&root));
    let second = Arc::new(FsEventLog::new(&root));
    let id = CompanyId::new("acme");

    const N: u64 = 32;
    let mut set = tokio::task::JoinSet::new();
    for i in 0..N {
        let log = if i % 2 == 0 {
            first.clone()
        } else {
            second.clone()
        };
        let id = id.clone();
        set.spawn(async move {
            log.append(
                &id,
                CompanyEvent::OperatorMessage {
                    mentions: Vec::new(),
                    parent: None,
                    text: format!("event {i}"),
                    by: None,
                    chat: None,
                    deliverable: None,
                    attachments: Vec::new(),
                },
            )
            .await
            .expect("append succeeds")
        });
    }
    let mut handed_out = Vec::new();
    while let Some(res) = set.join_next().await {
        handed_out.push(res.expect("task joins").value());
    }

    handed_out.sort_unstable();
    assert_eq!(
        handed_out,
        (0..N).collect::<Vec<_>>(),
        "the sequences handed to callers must be unique and dense — a repeat \
             means two instances read the same line count before either appended"
    );

    // And the same must hold for what actually landed on disk.
    let stored = first.read_from(&id, EventSeq::new(0), 1024).await.unwrap();
    assert_eq!(stored.len() as u64, N, "every append is on disk");
    let mut persisted: Vec<u64> = stored.iter().map(|e| e.seq.value()).collect();
    persisted.sort_unstable();
    assert_eq!(
        persisted,
        (0..N).collect::<Vec<_>>(),
        "the persisted sequences must be unique and dense too"
    );
}

// The fs backend runs the identical port-conformance suite the sqlite
// backend runs under `--features sqlite`. Each test gets a fresh root so the
// stores start empty.
#[tokio::test]
async fn conformance_isolation_by_company() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_isolation_by_company(
        Arc::new(FsCompanyStore::new(&root)),
        Arc::new(FsEventLog::new(&root)),
        Arc::new(FsTraceStore::new(&root)),
    )
    .await;
}

#[tokio::test]
async fn conformance_export_totality() {
    let root_dir = tmp_root();
    let root = root_dir.path().to_path_buf();
    conformance::assert_export_totality(
        Arc::new(FsCompanyStore::new(&root)),
        Arc::new(FsEventLog::new(&root)),
        Arc::new(FsTraceStore::new(&root)),
    )
    .await;
}
