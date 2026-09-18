use super::*;
use crate::ports::types::StoredEvent;
use futures::stream::{self, BoxStream};
use std::sync::Mutex;

/// A log that records what was appended, so a test can ask what actually
/// reached the journal rather than what the tool said it did.
pub(super) struct RecordingLog(pub(super) Mutex<Vec<CompanyEvent>>);

/// Like [`RecordingLog`], but refuses to append to one named channel —
/// for proving a `desk_dm` to several recipients does not treat one
/// recipient's journal failure as a reason to report the whole call
/// failed after earlier recipients already got a durable row.
pub(super) struct FlakyLog {
    pub(super) events: Mutex<Vec<CompanyEvent>>,
    pub(super) refuses: &'static str,
}

#[async_trait]
impl EventLog for FlakyLog {
    async fn append(&self, _id: &CompanyId, event: CompanyEvent) -> crate::Result<EventSeq> {
        if let CompanyEvent::AgentReply { chat_id, .. } = &event
            && chat_id == self.refuses
        {
            return Err(crate::error::OpenCompanyError::Conflict(format!(
                "journal unavailable for {chat_id}"
            )));
        }
        let mut appended = self.events.lock().expect("test log lock");
        appended.push(event);
        Ok(EventSeq::new(appended.len() as u64))
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        _seq: EventSeq,
        _limit: usize,
    ) -> crate::Result<Vec<StoredEvent>> {
        Ok(Vec::new())
    }
    fn subscribe(
        &self,
        _id: &CompanyId,
    ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(stream::empty())
    }
}

/// A journal that actually answers `read_before`, for exercising
/// `desk_read`'s scan — `RecordingLog` and `FlakyLog` both hardcode
/// `read_from` to `Ok(Vec::new())`, which is fine for the tools that only
/// append, but makes them useless for testing a tool whose entire job is
/// reading history back.
#[derive(Default)]
pub(super) struct HistoryLog(Mutex<Vec<StoredEvent>>);

#[async_trait]
impl EventLog for HistoryLog {
    async fn append(&self, _id: &CompanyId, event: CompanyEvent) -> crate::Result<EventSeq> {
        let mut rows = self.0.lock().expect("test log lock");
        let seq = EventSeq::new(rows.len() as u64 + 1);
        rows.push(StoredEvent {
            seq,
            company: CompanyId::new("acme"),
            event,
            at_millis: 0,
        });
        Ok(seq)
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        seq: EventSeq,
        limit: usize,
    ) -> crate::Result<Vec<StoredEvent>> {
        Ok(self
            .0
            .lock()
            .expect("test log lock")
            .iter()
            .filter(|stored| stored.seq.value() >= seq.value())
            .take(limit)
            .cloned()
            .collect())
    }
    async fn read_before(
        &self,
        _id: &CompanyId,
        before: Option<EventSeq>,
        limit: usize,
    ) -> crate::Result<Vec<StoredEvent>> {
        let mut rows: Vec<StoredEvent> = self
            .0
            .lock()
            .expect("test log lock")
            .iter()
            .filter(|stored| before.is_none_or(|cursor| stored.seq.value() < cursor.value()))
            .cloned()
            .collect();
        rows.reverse();
        rows.truncate(limit);
        Ok(rows)
    }
    fn subscribe(
        &self,
        _id: &CompanyId,
    ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(stream::empty())
    }
}

#[async_trait]
impl EventLog for RecordingLog {
    async fn append(&self, _id: &CompanyId, event: CompanyEvent) -> crate::Result<EventSeq> {
        let mut appended = self.0.lock().expect("test log lock");
        appended.push(event);
        Ok(EventSeq::new(appended.len() as u64))
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        _seq: EventSeq,
        _limit: usize,
    ) -> crate::Result<Vec<StoredEvent>> {
        Ok(Vec::new())
    }
    fn subscribe(
        &self,
        _id: &CompanyId,
    ) -> BoxStream<'static, crate::ports::events::EventStreamItem> {
        Box::pin(stream::empty())
    }
}

pub(super) fn context() -> (SpeechContext, Arc<RecordingLog>, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("speech-tools-")
        .tempdir()
        .expect("tempdir");
    let store: Arc<dyn crate::ports::store::CompanyStore> =
        Arc::new(crate::store::FsCompanyStore::new(dir.path()));
    let events = Arc::new(RecordingLog(Mutex::new(Vec::new())));
    let context = SpeechContext::new(
        CompanyId::new("acme"),
        "designer".to_string(),
        events.clone() as Arc<dyn EventLog>,
        store,
    );
    (context, events, dir)
}

/// A roster with one desk `designer` sits on and one it does not.
pub(super) async fn context_with_desks() -> (SpeechContext, Arc<RecordingLog>, tempfile::TempDir) {
    let (context, events, dir) = context();
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
[company]
name = "Acme"

[policy]
mode = "full"

[[agent]]
id = "designer"
role = "Designer"
description = "Draws things."

[[agent]]
id = "engineer"
role = "Engineer"
description = "Builds things."

[[group_chat]]
id = "brand"
name = "Brand"
members = ["designer"]

[[group_chat]]
id = "platform"
name = "Platform"
members = ["engineer"]
"#,
    )
    .expect("valid manifest");
    let record = crate::ports::types::CompanyRecord {
        id: context.company.clone(),
        manifest,
        ledger: Vec::new(),
        lifecycle: "running".to_string(),
        setup: None,
        name_confirmed: false,
        activation_completed_at: None,
        created_at_millis: None,
        overlay_agents: Vec::new(),
        overlay_desk_members: Vec::new(),
        overlay_desk_order: Vec::new(),
        overlay_desks: Vec::new(),
        overlay_desk_hive: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        overlay_tool_grants: None,
        overlay_workflows: Vec::new(),
        overlay_budgets: Vec::new(),
        overlay_policy: None,
        overlay_desk_tools: Default::default(),
        disabled_workflows: Vec::new(),
        template_provenance: None,
    };
    context.store.save(&record).await.expect("the record saves");
    (context, events, dir)
}

/// A roster with two operator-added teammates: `nova`, whose display name
/// `Nova` is unique, and two sharing the display name `Rivers` — the two
/// [`crate::ports::types::TeammateResolution`] arms `desk_dm` must not
/// collapse into "found something, ship it".
pub(super) async fn context_with_overlay_teammates()
-> (SpeechContext, Arc<RecordingLog>, tempfile::TempDir) {
    let (context, events, dir) = context();
    let manifest: crate::company::CompanyManifest = toml::from_str(
        r#"
[company]
name = "Acme"

[policy]
mode = "full"

[[agent]]
id = "designer"
role = "Designer"
description = "Draws things."

[[agent]]
id = "copy"
role = "Copywriter"
description = "Writes things."

[[agent]]
id = "researcher"
role = "Researcher"
description = "Finds things."
"#,
    )
    .expect("valid manifest");
    let mut record = crate::ports::types::CompanyRecord {
        id: context.company.clone(),
        manifest,
        ledger: Vec::new(),
        lifecycle: "running".to_string(),
        setup: None,
        name_confirmed: false,
        activation_completed_at: None,
        created_at_millis: None,
        overlay_agents: Vec::new(),
        overlay_desk_members: Vec::new(),
        overlay_desk_order: Vec::new(),
        overlay_desks: Vec::new(),
        overlay_desk_hive: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        overlay_tool_grants: None,
        overlay_workflows: Vec::new(),
        overlay_budgets: Vec::new(),
        overlay_policy: None,
        overlay_desk_tools: Default::default(),
        disabled_workflows: Vec::new(),
        template_provenance: None,
    };
    for (id, name) in [
        ("nova", "Nova"),
        ("rivers-1", "Rivers"),
        ("rivers-2", "Rivers"),
    ] {
        record
            .overlay_agents
            .push(crate::ports::types::OverlayAgent {
                provider: None,
                id: id.to_string(),
                name: name.to_string(),
                role: "Growth".to_string(),
                description: None,
                tools: None,
                model: None,
                harness: None,
            });
    }
    context.store.save(&record).await.expect("the record saves");
    (context, events, dir)
}
