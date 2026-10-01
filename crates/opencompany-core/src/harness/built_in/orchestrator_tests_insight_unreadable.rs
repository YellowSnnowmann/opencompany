use super::*;
use crate::ports::facts::{FactKind, FactRecord};
use crate::ports::types::{CompanyRecord, CompanySummary, LedgerEntry, StoredEvent};
use futures::stream::{self, BoxStream};

struct FailingFacts;

#[async_trait]
impl FactStore for FailingFacts {
    async fn list(
        &self,
        _company: &CompanyId,
        _query: Option<&str>,
        _kind: Option<FactKind>,
    ) -> crate::Result<Vec<FactRecord>> {
        Err(OpenCompanyError::Store("facts offline".into()))
    }
    async fn upsert(&self, _company: &CompanyId, _fact: &FactRecord) -> crate::Result<()> {
        unimplemented!("not exercised")
    }
    async fn delete(&self, _company: &CompanyId, _id: &str) -> crate::Result<bool> {
        unimplemented!("not exercised")
    }
}

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
async fn query_company_says_facts_and_activity_could_not_be_read() {
    let tool = QueryCompanyTool::new(
        CompanyId::new("acme"),
        Some(Arc::new(FailingFacts)),
        Some(Arc::new(FailingEvents)),
        None,
        None,
        None,
    );
    let result = tool.execute(json!({})).await.unwrap();
    assert!(!result.is_error);
    let out = result.output_for_llm(true);
    assert!(out.contains("Facts could not be read"), "{out}");
    assert!(!out.contains("No durable facts recorded"), "{out}");
    assert!(out.contains("Recent activity could not be read"), "{out}");
    assert!(!out.contains("No recent activity"), "{out}");
    assert_eq!(unreadable(&result), ["facts", "recent_activity"]);
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
