use super::*;
use crate::ports::types::{CompanyRecord, CompanySummary, LedgerEntry, StoredEvent};
use futures::stream::{self, BoxStream};

struct FailingEvents;

#[async_trait]
impl EventLog for FailingEvents {
    async fn append(&self, _company: &CompanyId, _event: CompanyEvent) -> crate::Result<EventSeq> {
        unimplemented!("not exercised")
    }
    async fn read_from(
        &self,
        _company: &CompanyId,
        _seq: EventSeq,
        _limit: usize,
    ) -> crate::Result<Vec<StoredEvent>> {
        Err(OpenCompanyError::Store("journal offline".into()))
    }
    fn subscribe(
        &self,
        _company: &CompanyId,
    ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(stream::empty())
    }
}

struct FailingRecords;

#[async_trait]
impl CompanyStore for FailingRecords {
    async fn load(&self, _id: &CompanyId) -> crate::Result<Option<CompanyRecord>> {
        Err(OpenCompanyError::Store("records offline".into()))
    }
    async fn save(&self, _record: &CompanyRecord) -> crate::Result<()> {
        unimplemented!("not exercised")
    }
    async fn list(&self) -> crate::Result<Vec<CompanySummary>> {
        unimplemented!("not exercised")
    }
    async fn append_ledger(&self, _id: &CompanyId, _entry: LedgerEntry) -> crate::Result<()> {
        unimplemented!("not exercised")
    }
}

struct EmptyRecords;

#[async_trait]
impl CompanyStore for EmptyRecords {
    async fn load(&self, _id: &CompanyId) -> crate::Result<Option<CompanyRecord>> {
        Ok(None)
    }
    async fn save(&self, _record: &CompanyRecord) -> crate::Result<()> {
        unimplemented!("not exercised")
    }
    async fn list(&self) -> crate::Result<Vec<CompanySummary>> {
        unimplemented!("not exercised")
    }
    async fn append_ledger(&self, _id: &CompanyId, _entry: LedgerEntry) -> crate::Result<()> {
        unimplemented!("not exercised")
    }
}

fn payload(result: &ToolResult) -> Value {
    match &result.content[0] {
        openhuman_core::skills::types::ToolContent::Json { data } => data.clone(),
        other => panic!("expected a JSON content block, got {other:?}"),
    }
}

fn unreadable(result: &ToolResult) -> Vec<String> {
    payload(result)["unreadable"]
        .as_array()
        .expect("unreadable array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn query_company_says_activity_could_not_be_read() {
    let tool = QueryCompanyTool::new(
        CompanyId::new("acme"),
        None,
        Some(Arc::new(FailingEvents)),
        None,
        None,
        None,
    );
    let result = tool.execute(json!({})).await.unwrap();
    assert!(!result.is_error);
    let out = result.output_for_llm(true);
    assert!(out.contains("No durable facts recorded"), "{out}");
    assert!(out.contains("Recent activity could not be read"), "{out}");
    assert!(!out.contains("No recent activity"), "{out}");
    assert_eq!(unreadable(&result), ["recent_activity"]);
}

#[tokio::test]
async fn query_company_says_the_company_record_could_not_be_read() {
    let store: Arc<dyn CompanyStore> = Arc::new(FailingRecords);
    let tool = QueryCompanyTool::new(CompanyId::new("acme"), None, None, None, Some(store), None);
    let result = tool.execute(json!({})).await.unwrap();
    assert!(!result.is_error);
    let out = result.output_for_llm(true);
    for workflow in crate::globals::workflows() {
        assert!(
            out.contains(&workflow.id),
            "global workflows still list: {out}"
        );
    }
    assert!(
        out.contains("workflows created at runtime are missing"),
        "{out}"
    );
    assert!(!out.contains("Author one with"), "{out}");
    assert!(out.contains("The roster could not be read"), "{out}");
    assert!(!out.contains("Roster unavailable"), "{out}");
    assert!(out.contains("Desks could not be read"), "{out}");
    assert!(!out.contains("_No desks._"), "{out}");
    assert_eq!(unreadable(&result), ["saved_workflows", "team", "desks"]);
}

#[tokio::test]
async fn query_company_keeps_unavailable_wording_for_a_missing_record() {
    let store: Arc<dyn CompanyStore> = Arc::new(EmptyRecords);
    let tool = QueryCompanyTool::new(CompanyId::new("acme"), None, None, None, Some(store), None);
    let result = tool.execute(json!({})).await.unwrap();
    let out = result.output_for_llm(true);
    assert!(out.contains("Roster unavailable"), "{out}");
    assert!(!out.contains("could not be read"), "{out}");
    assert!(unreadable(&result).is_empty());
}

#[tokio::test]
async fn query_company_board_read_failure_points_at_list_tasks() {
    let tasks: Arc<dyn TaskStore> = Arc::new(BrokenTaskStore);
    let tool = QueryCompanyTool::new(CompanyId::new("acme"), None, None, None, None, Some(tasks));
    let result = tool.execute(json!({})).await.unwrap();
    let out = result.output_for_llm(true);
    assert!(out.contains("The board could not be read"), "{out}");
    assert!(out.contains("use list_tasks"), "{out}");
    assert_eq!(unreadable(&result), ["board"]);
}

#[tokio::test]
async fn query_company_unwired_board_says_no_board_is_wired() {
    let tool = QueryCompanyTool::new(CompanyId::new("acme"), None, None, None, None, None);
    let result = tool.execute(json!({})).await.unwrap();
    let out = result.output_for_llm(true);
    assert!(out.contains("No task board is wired on this host"), "{out}");
    assert!(unreadable(&result).is_empty());
}

fn cache_with(nodes: Value) -> RunOutputCache {
    let cache = RunOutputCache::default();
    cache.store("run-1", "demo", nodes, Vec::new());
    cache
}

#[tokio::test]
async fn read_run_output_refuses_a_node_without_an_items_list() {
    let cache = cache_with(json!({ "worker": { "text": "x" } }));
    let reader = ReadRunOutputTool::new(CompanyId::new("acme"), cache);
    let result = reader
        .execute(json!({ "run_id": "run-1", "node": "worker" }))
        .await
        .unwrap();
    assert!(result.is_error, "{result:?}");
    let out = result.output();
    assert!(out.contains("no `items`"), "{out}");
    assert!(out.contains("not the same as no output"), "{out}");
    assert!(!out.contains("produced no items"), "{out}");
}

#[tokio::test]
async fn read_run_output_still_reports_an_empty_items_list_as_no_items() {
    let cache = cache_with(json!({ "worker": { "items": [] } }));
    let reader = ReadRunOutputTool::new(CompanyId::new("acme"), cache);
    let result = reader
        .execute(json!({ "run_id": "run-1", "node": "worker" }))
        .await
        .unwrap();
    assert!(!result.is_error);
    assert!(result.output().contains("produced no items"));
}

#[tokio::test]
async fn read_run_output_unknown_node_listing_marks_unreadable_output() {
    let cache = cache_with(json!({
        "good": { "items": ["a", "b"] },
        "odd": { "text": "x" },
    }));
    let reader = ReadRunOutputTool::new(CompanyId::new("acme"), cache);
    let out = reader
        .execute(json!({ "run_id": "run-1", "node": "missing" }))
        .await
        .unwrap()
        .output();
    assert!(out.contains("`good` (2 item(s))"), "{out}");
    assert!(out.contains("`odd` (output unreadable)"), "{out}");
    assert!(!out.contains("`odd` (0 item(s))"), "{out}");
}
