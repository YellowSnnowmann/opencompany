use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use super::*;
use crate::ledger::LedgerEvent;
use crate::ports::ledgers::LedgerStore;
use crate::ports::types::CompanyId;
use crate::store::FsOps;

struct EventsFailAfter {
    inner: Arc<FsOps>,
    successes: usize,
    calls: AtomicUsize,
}

#[async_trait]
impl LedgerStore for EventsFailAfter {
    async fn list_specs(&self, company: &CompanyId) -> crate::Result<Vec<LedgerSpec>> {
        self.inner.list_specs(company).await
    }
    async fn put_spec(&self, company: &CompanyId, spec: &LedgerSpec) -> crate::Result<()> {
        self.inner.put_spec(company, spec).await
    }
    async fn delete_spec(&self, company: &CompanyId, slug: &str) -> crate::Result<bool> {
        self.inner.delete_spec(company, slug).await
    }
    async fn append(&self, company: &CompanyId, event: &LedgerEvent) -> crate::Result<()> {
        self.inner.append(company, event).await
    }
    async fn events(&self, company: &CompanyId, ledger: &str) -> crate::Result<Vec<LedgerEvent>> {
        if self.calls.fetch_add(1, Ordering::SeqCst) >= self.successes {
            return Err(crate::error::OpenCompanyError::Store("disk offline".into()));
        }
        self.inner.events(company, ledger).await
    }
    async fn purge_entry(
        &self,
        company: &CompanyId,
        ledger: &str,
        entry: &str,
    ) -> crate::Result<bool> {
        self.inner.purge_entry(company, ledger, entry).await
    }
    async fn purge_ledger(&self, company: &CompanyId, ledger: &str) -> crate::Result<bool> {
        self.inner.purge_ledger(company, ledger).await
    }
}

fn find<'a>(tools: &'a [Box<dyn Tool>], name: &str) -> &'a dyn Tool {
    tools
        .iter()
        .find(|tool| tool.name() == name)
        .map(AsRef::as_ref)
        .expect("tool registered")
}

async fn seeded_risks(home: &tempfile::TempDir) -> Arc<FsOps> {
    let ops = Arc::new(FsOps::new(home.path().to_path_buf()));
    let ctx = Ledgers::new(CompanyId::new("acme"), ops.clone());
    let tools = ledger_tools(ctx, "ceo".to_string(), None, true);
    let defined = find(&tools, DEFINE_LEDGER_TOOL)
        .execute(json!({
            "slug": "risks",
            "title": "Risks",
            "purpose": "What could go wrong.",
            "fields": [
                { "name": "id", "role": "id" },
                { "name": "risk", "role": "title" },
                { "name": "status", "role": "status" }
            ],
            "statuses": [{ "name": "open" }, { "name": "closed", "closed": true }]
        }))
        .await
        .unwrap();
    assert!(!defined.is_error, "{defined:?}");
    let recorded = find(&tools, RECORD_ENTRY_TOOL)
        .execute(json!({
            "ledger": "risks",
            "id": "vendor-slip",
            "fields": { "risk": "the vendor misses the date", "status": "open" }
        }))
        .await
        .unwrap();
    assert!(!recorded.is_error, "{recorded:?}");
    ops
}

fn failing_tools(ops: Arc<FsOps>, successes: usize) -> Vec<Box<dyn Tool>> {
    let store: Arc<dyn LedgerStore> = Arc::new(EventsFailAfter {
        inner: ops,
        successes,
        calls: AtomicUsize::new(0),
    });
    ledger_tools(
        Ledgers::new(CompanyId::new("acme"), store),
        "ceo".to_string(),
        None,
        true,
    )
}

#[tokio::test]
async fn list_ledgers_says_counts_are_unavailable_when_rows_cannot_be_read() {
    let home = tempfile::tempdir().unwrap();
    let ops = seeded_risks(&home).await;
    let tools = failing_tools(ops, 0);
    let out = find(&tools, LIST_LEDGERS_TOOL)
        .execute(json!({}))
        .await
        .unwrap();
    assert!(!out.is_error, "{out:?}");
    let text = out.output();
    assert!(text.contains("`risks`"), "{text}");
    assert!(
        text.contains("counts unavailable — rows could not be read"),
        "{text}"
    );
    assert!(text.contains("do not treat this ledger as empty"), "{text}");
    let risks = text.split("- `risks`").nth(1).expect("risks listed");
    let risks = risks.split("\n- ").next().unwrap();
    assert!(
        !risks
            .lines()
            .any(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit())),
        "no fabricated count: {risks}"
    );
}

#[tokio::test]
async fn read_ledger_counts_its_total_from_the_read_it_already_made() {
    let home = tempfile::tempdir().unwrap();
    let ops = seeded_risks(&home).await;
    let tools = failing_tools(ops, 1);
    let out = find(&tools, READ_LEDGER_TOOL)
        .execute(json!({ "ledger": "risks", "query": "no such text" }))
        .await
        .unwrap();
    assert!(!out.is_error, "{out:?}");
    let text = out.output();
    assert!(
        text.contains("`risks` has no rows matching that. It holds 1 in total."),
        "{text}"
    );
}
