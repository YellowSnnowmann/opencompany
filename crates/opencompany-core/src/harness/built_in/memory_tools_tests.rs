use super::*;
use crate::store::FsContextStore;

fn ctx(dir: &std::path::Path) -> Arc<dyn ContextStore> {
    Arc::new(FsContextStore::new(dir.to_path_buf()))
}

fn tools_for(
    dir: &std::path::Path,
    company: &str,
    agent: &str,
) -> (Box<dyn Tool>, Box<dyn Tool>, Box<dyn Tool>) {
    let mut v = memory_tools(ctx(dir), CompanyId::new(company), agent.to_string());
    let forget = v.pop().unwrap();
    let recall = v.pop().unwrap();
    let store = v.pop().unwrap();
    (store, recall, forget)
}

fn addr_from(reply: &str) -> String {
    // "... (addr <a>) ..."
    let i = reply.find("(addr ").unwrap() + 6;
    let j = reply[i..].find(')').unwrap();
    reply[i..i + j].to_string()
}

#[tokio::test]
async fn store_recall_forget_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (store, recall, forget) = tools_for(dir.path(), "acme", "ceo");

    let stored = store
        .execute(json!({"title": "Fiscal year", "body": "Acme's fiscal year starts in February."}))
        .await
        .unwrap();
    assert!(!stored.is_error, "{stored:?}");
    let addr = addr_from(&stored.text());

    let found = recall.execute(json!({"query": "fiscal"})).await.unwrap();
    assert!(
        found.text().contains(&addr),
        "recall must surface the stored addr"
    );
    assert!(found.text().contains("February"));

    let gone = forget.execute(json!({"addr": addr.clone()})).await.unwrap();
    assert!(!gone.is_error);
    assert!(gone.text().contains("Forgotten"));

    // Idempotent, as the tool description promises: nothing lives at the
    // address any more, so a repeated forget (a retry, a stale recall
    // listing) succeeds as a no-op instead of scolding the agent.
    let again = forget.execute(json!({"addr": addr})).await.unwrap();
    assert!(
        !again.is_error,
        "already-forgotten must be a no-op: {again:?}"
    );
    assert!(again.text().contains("already gone"));
}

#[tokio::test]
async fn memory_store_redacts_credentials_in_the_title() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let context = ctx(dir.path());
    let (store, _, _) = tools_for(dir.path(), "acme", "ceo");

    // A credential-shaped title must not persist verbatim anywhere: not in
    // the stored body, not in the label, not in the success echo.
    let stored = store
        .execute(json!({"title": "Bearer sk-longsecret", "body": "the api key for staging"}))
        .await
        .unwrap();
    assert!(!stored.is_error, "{stored:?}");
    assert!(
        stored.text().contains("[REDACTED]"),
        "success echo must carry the redacted title: {}",
        stored.text()
    );
    assert!(
        !stored.text().contains("sk-longsecret"),
        "{}",
        stored.text()
    );

    let addr = addr_from(&stored.text());
    let peeked = context
        .peek(&company, &ChunkAddr::new(addr), None)
        .await
        .unwrap();
    assert!(
        peeked.contains("Bearer [REDACTED]"),
        "stored title must be redacted: {peeked}"
    );
    assert!(!peeked.contains("sk-longsecret"), "{peeked}");
}

#[tokio::test]
async fn forget_cannot_touch_other_rows() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let context = ctx(dir.path());

    // A task-outcome row (the loop's) and another agent's memory.
    let outcome = context
        .put(
            &company,
            ContextChunk {
                label: "task-outcome/ceo".into(),
                body: "Task: x\nOutcome: y".into(),
            },
        )
        .await
        .unwrap();
    let theirs = context
        .put(
            &company,
            ContextChunk {
                label: format!("{AGENT_MEMORY_LABEL_PREFIX}/researcher/their-note"),
                body: "Their note\n\nbody".into(),
            },
        )
        .await
        .unwrap();

    let (_, _, forget) = tools_for(dir.path(), "acme", "ceo");
    for addr in [&outcome, &theirs] {
        let refused = forget
            .execute(json!({"addr": addr.as_ref()}))
            .await
            .unwrap();
        assert!(refused.is_error, "must refuse {addr:?}");
        // And the row must still be there.
        context.peek(&company, addr, None).await.unwrap();
    }
}

/// Content addressing means byte-identical bodies share ONE address. A
/// forget of a shared address removes exactly the caller's own claim
/// (label-scoped delete, #1300): the other agent's row and the body both
/// survive, and the caller is told the text lives on under other labels.
/// This replaces the old refusal, which let any agent make another's
/// memory permanently un-forgettable by storing identical text.
#[tokio::test]
async fn forget_of_a_shared_address_removes_only_the_callers_claim() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let context = ctx(dir.path());

    let (ceo_store, _, ceo_forget) = tools_for(dir.path(), "acme", "ceo");
    let (them_store, _, _) = tools_for(dir.path(), "acme", "researcher");
    let mine = ceo_store
        .execute(json!({"title": "Fiscal year", "body": "Starts in February."}))
        .await
        .unwrap();
    let addr = addr_from(&mine.text());
    them_store
        .execute(json!({"title": "Fiscal year", "body": "Starts in February."}))
        .await
        .unwrap();

    let forgotten = ceo_forget
        .execute(json!({"addr": addr.clone()}))
        .await
        .unwrap();
    assert!(
        !forgotten.is_error,
        "a shared address must forget the caller's own claim: {forgotten:?}"
    );
    assert!(
        forgotten.text().contains("other labels"),
        "the reply must say the text lives on elsewhere: {forgotten:?}"
    );
    // Exactly the researcher's row remains, and the body with it.
    let rows = context
        .list(&company, AGENT_MEMORY_LABEL_PREFIX)
        .await
        .unwrap();
    let labels: Vec<&str> = rows
        .iter()
        .filter(|m| m.addr.as_ref() == addr)
        .map(|m| m.label.as_str())
        .collect();
    assert_eq!(labels.len(), 1, "only the ceo's claim may go: {labels:?}");
    assert!(
        labels[0].starts_with("agent-memory/researcher/"),
        "{labels:?}"
    );
    context
        .peek(&company, &ChunkAddr::new(addr), None)
        .await
        .expect("the body must survive under the researcher's claim");
}

/// The regression lock the #1290 review found missing: deleting the
/// trailing slash from own_prefix() — the exact #936 Namespace class —
/// survived every existing test. `ann` and `anna` are both legal
/// snake_case agent ids; ann's guard must neither see nor delete anna's
/// rows.
#[tokio::test]
async fn the_prefix_boundary_is_a_namespace_not_a_string_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let context = ctx(dir.path());
    let (anna_store, _, _) = tools_for(dir.path(), "acme", "anna");
    let stored = anna_store
        .execute(json!({"title": "Annas note", "body": "hers alone"}))
        .await
        .unwrap();
    let addr = addr_from(&stored.text());

    let (_, _, ann_forget) = tools_for(dir.path(), "acme", "ann");
    let refused = ann_forget
        .execute(json!({"addr": addr.clone()}))
        .await
        .unwrap();
    assert!(
        refused.is_error,
        "ann must not reach agent-memory/anna/ rows: {refused:?}"
    );
    context
        .peek(&company, &crate::ports::types::ChunkAddr::new(addr), None)
        .await
        .expect("anna's row must survive ann's attempt");
}

#[tokio::test]
async fn tools_are_company_isolated() {
    // Two companies over one store root: what alpha stores, beta's tools
    // can neither recall nor forget — the CompanyId captured at build time
    // is the entire boundary, exactly as the port contract promises.
    let dir = tempfile::tempdir().unwrap();
    let (a_store, _, _) = tools_for(dir.path(), "alpha", "ceo");
    let (_, b_recall, b_forget) = tools_for(dir.path(), "beta", "ceo");

    let stored = a_store
        .execute(json!({"title": "Alpha secret plan", "body": "The plan is zig."}))
        .await
        .unwrap();
    let addr = addr_from(&stored.text());

    let seen = b_recall.execute(json!({"query": "zig"})).await.unwrap();
    // The no-match reply echoes the query word, so assert on what must be
    // absent: alpha's addr and alpha's body text ("The plan is zig." would
    // surface as a snippet), plus the no-match marker being present.
    assert!(
        !seen.text().contains(&addr) && !seen.text().contains("The plan is"),
        "beta recalled alpha's memory: {}",
        seen.text()
    );
    assert!(seen.text().contains("Nothing in memory matches"));
    // Beta's forget of alpha's addr: within beta's visibility nothing
    // lives at that address, so the answer is the idempotent no-op — the
    // same answer a never-existed addr gets, so the response is no
    // cross-company existence oracle. What MUST hold is that alpha's row
    // survives untouched.
    let noop = b_forget
        .execute(json!({"addr": addr.clone()}))
        .await
        .unwrap();
    assert!(
        !noop.is_error,
        "cross-company forget answers the no-op: {noop:?}"
    );
    assert!(noop.text().contains("already gone"));
    let (_, a_recall, _) = tools_for(dir.path(), "alpha", "ceo");
    let still = a_recall.execute(json!({"query": "zig"})).await.unwrap();
    assert!(
        still.text().contains(&addr),
        "alpha's memory must survive beta's forget: {}",
        still.text()
    );
}

/// FAIL-axis: the caps at the top of this module are refusals, not
/// truncations — an oversized title or body must be rejected whole, with
/// nothing persisted, so the agent never reads back a memory that was
/// silently cut.
#[tokio::test]
async fn store_refuses_oversized_title_and_body_and_persists_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let context = ctx(dir.path());
    let (store, _, _) = tools_for(dir.path(), "acme", "ceo");

    let over_title = "t".repeat(MAX_TITLE_BYTES + 1);
    let refused = store
        .execute(json!({"title": over_title, "body": "short body"}))
        .await
        .unwrap();
    assert!(refused.is_error, "oversized title must refuse: {refused:?}");
    assert!(
        refused.text().contains(&MAX_TITLE_BYTES.to_string()),
        "the refusal must name the title cap: {}",
        refused.text()
    );

    let over_body = "b".repeat(MAX_BODY_BYTES + 1);
    let refused = store
        .execute(json!({"title": "Fine title", "body": over_body}))
        .await
        .unwrap();
    assert!(refused.is_error, "oversized body must refuse: {refused:?}");
    assert!(
        refused.text().contains(&MAX_BODY_BYTES.to_string()),
        "the refusal must name the body cap: {}",
        refused.text()
    );

    // Neither refusal may leave a truncated row behind.
    let rows = context
        .list(&company, AGENT_MEMORY_LABEL_PREFIX)
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "a refused store must persist nothing: {rows:?}"
    );

    // Exactly at the cap is still accepted — the refusal is `>`, not `>=`.
    let ok = store
        .execute(json!({"title": "t".repeat(MAX_TITLE_BYTES), "body": "b".repeat(MAX_BODY_BYTES)}))
        .await
        .unwrap();
    assert!(!ok.is_error, "at-cap must be accepted: {ok:?}");
}

/// The read surface `memory_recall` actually has: company-wide, not
/// caller-scoped. Another agent's deliberate memory and the turn loop's
/// task outcomes both surface — which the tool description's plain
/// phrasing understates — while the company boundary stays absolute.
/// Locked so that widening (cross-company) or narrowing (own-prefix only)
/// both break a test rather than shipping silently.
#[tokio::test]
async fn recall_reads_the_whole_company_not_just_the_callers_own_memories() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let context = ctx(dir.path());

    let (their_store, _, _) = tools_for(dir.path(), "acme", "researcher");
    let theirs = their_store
        .execute(json!({"title": "Vendor terms", "body": "Netsuite renews in March."}))
        .await
        .unwrap();
    let their_addr = addr_from(&theirs.text());
    let outcome = context
        .put(
            &company,
            ContextChunk {
                label: "task-outcome/researcher".into(),
                body: "Task: audit vendors\nOutcome: Netsuite flagged".into(),
            },
        )
        .await
        .unwrap();

    let (_, ceo_recall, _) = tools_for(dir.path(), "acme", "ceo");
    let seen = ceo_recall
        .execute(json!({"query": "Netsuite"}))
        .await
        .unwrap();
    assert!(!seen.is_error, "{seen:?}");
    assert!(
        seen.text().contains(their_addr.as_str()),
        "recall must surface another agent's memory in the same company: {}",
        seen.text()
    );
    assert!(
        seen.text().contains(outcome.as_ref()),
        "recall must surface the loop's task outcomes: {}",
        seen.text()
    );

    // The one boundary that is absolute: another company sees none of it.
    let (_, other_recall, _) = tools_for(dir.path(), "zenith", "ceo");
    let blind = other_recall
        .execute(json!({"query": "Netsuite"}))
        .await
        .unwrap();
    assert!(
        !blind.text().contains(their_addr.as_str()) && !blind.text().contains(outcome.as_ref()),
        "the company boundary must hold: {}",
        blind.text()
    );
}

/// A store that answers `list` with an error, delegating everything else
/// to a real one. Records whether any delete reached the backend.
struct ListFailsStore {
    inner: Arc<dyn ContextStore>,
    deletes: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl ContextStore for ListFailsStore {
    async fn put(&self, id: &CompanyId, chunk: ContextChunk) -> crate::Result<ChunkAddr> {
        self.inner.put(id, chunk).await
    }
    async fn list(
        &self,
        _: &CompanyId,
        _: &str,
    ) -> crate::Result<Vec<crate::ports::types::ChunkMeta>> {
        Err(crate::error::OpenCompanyError::Store(
            "context index unavailable".to_string(),
        ))
    }
    async fn peek(
        &self,
        id: &CompanyId,
        addr: &ChunkAddr,
        range: Option<std::ops::Range<usize>>,
    ) -> crate::Result<String> {
        self.inner.peek(id, addr, range).await
    }
    async fn search(
        &self,
        id: &CompanyId,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<crate::ports::types::ChunkHit>> {
        self.inner.search(id, query, limit).await
    }
    async fn delete(&self, id: &CompanyId, addr: &ChunkAddr) -> crate::Result<bool> {
        self.deletes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.delete(id, addr).await
    }
    async fn delete_label(
        &self,
        id: &CompanyId,
        addr: &ChunkAddr,
        label: &str,
    ) -> crate::Result<bool> {
        self.deletes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.delete_label(id, addr, label).await
    }
}

/// FAIL-axis: `memory_forget`'s ownership guard is a read of the store's
/// index. When that read fails, the tool must fail closed — surface the
/// error and delete nothing — rather than falling through to a delete it
/// could not authorise.
#[tokio::test]
async fn forget_fails_closed_when_the_ownership_index_read_fails() {
    let dir = tempfile::tempdir().unwrap();
    let company = CompanyId::new("acme");
    let real = ctx(dir.path());

    let (store, _, _) = tools_for(dir.path(), "acme", "ceo");
    let stored = store
        .execute(json!({"title": "Fiscal year", "body": "Starts in February."}))
        .await
        .unwrap();
    let addr = addr_from(&stored.text());

    let deletes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failing: Arc<dyn ContextStore> = Arc::new(ListFailsStore {
        inner: real.clone(),
        deletes: deletes.clone(),
    });
    let mut v = memory_tools(failing, company.clone(), "ceo".to_string());
    let forget = v.pop().unwrap();

    let outcome = forget.execute(json!({"addr": addr.clone()})).await;
    assert!(
        outcome.is_err() || outcome.as_ref().unwrap().is_error,
        "a failed ownership read must not answer success: {outcome:?}"
    );
    assert_eq!(
        deletes.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "nothing may be deleted when ownership could not be established"
    );
    real.peek(&company, &ChunkAddr::new(addr), None)
        .await
        .expect("the memory must survive a failed forget");
}

#[tokio::test]
async fn slug_is_bounded_and_never_empty() {
    assert_eq!(slug("Fiscal Year!!"), "fiscal-year");
    assert_eq!(slug("///"), "note");
    assert!(slug(&"x".repeat(500)).len() <= 64);
}
