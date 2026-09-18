pub(super) use super::*;
pub(super) use crate::error::OpenCompanyError;
pub(super) use crate::ports::types::{Actor, CompanyEvent, EventSeq, StoredEvent};
pub(super) use async_trait::async_trait;
pub(super) use futures::stream::BoxStream;
pub(super) use tinyhivemind::session::{
    Conversation, SESSION_WINDOW, SessionQuery, project_session,
};

/// A journal that answers `read_before` the way a production store does —
/// from the tail, by an exclusive cursor. The default fallback on the trait
/// would pass these tests too, and would hide a page-contract mistake that
/// only a real tail read makes: this port is specified newest-first, so the
/// double it is tested against has to be newest-first for real.
pub(super) struct Journal(pub(super) Vec<StoredEvent>);

#[async_trait]
impl EventLog for Journal {
    async fn append(
        &self,
        _id: &CompanyId,
        _event: CompanyEvent,
    ) -> Result<EventSeq, OpenCompanyError> {
        unreachable!("the adapter never appends")
    }
    async fn read_from(
        &self,
        _id: &CompanyId,
        seq: EventSeq,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, OpenCompanyError> {
        Ok(self
            .0
            .iter()
            .filter(|stored| stored.seq >= seq)
            .take(limit)
            .cloned()
            .collect())
    }
    async fn read_before(
        &self,
        _id: &CompanyId,
        before: Option<EventSeq>,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, OpenCompanyError> {
        let mut rows: Vec<StoredEvent> = self
            .0
            .iter()
            .filter(|stored| before.is_none_or(|cursor| stored.seq < cursor))
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
        Box::pin(futures::stream::empty())
    }
}

pub(super) fn company() -> CompanyId {
    CompanyId::new("acme")
}

pub(super) fn record() -> CompanyRecord {
    let src = "[company]\nname = \"Acme\"\n\n[policy]\nmode = \"full\"\n\
               \n[[agent]]\nid = \"engineer\"\nrole = \"Engineer\"\ntier = \"orchestrator\"\n\
               \n[[agent]]\nid = \"designer\"\nrole = \"Designer\"\ntier = \"orchestrator\"\n";
    let manifest: crate::company::CompanyManifest = toml::from_str(src).expect("manifest parses");
    CompanyRecord {
        id: company(),
        manifest,
        ledger: Vec::new(),
        lifecycle: "running".to_string(),
        overlay_agents: Vec::new(),
        overlay_retired_agents: Vec::new(),
        overlay_agent_edits: Vec::new(),
        overlay_desk_members: Vec::new(),
        overlay_desk_order: Vec::new(),
        overlay_desk_hive: Vec::new(),
        overlay_desks: Vec::new(),
        overlay_workflows: Vec::new(),
        overlay_budgets: Vec::new(),
        overlay_policy: None,
        overlay_tool_grants: None,
        overlay_desk_tools: Default::default(),
        disabled_workflows: Vec::new(),
        template_provenance: None,
        setup: None,
        name_confirmed: true,
        created_at_millis: None,
        activation_completed_at: None,
    }
}

pub(super) fn stored(seq: u64, event: CompanyEvent) -> StoredEvent {
    StoredEvent {
        seq: EventSeq::new(seq),
        company: company(),
        event,
        at_millis: 1_000 + seq,
    }
}

pub(super) fn reply(agent_id: &str, chat: &str, text: &str) -> CompanyEvent {
    CompanyEvent::AgentReply {
        mentions: Vec::new(),
        mention_depth: 0,
        parent: None,
        task_id: None,
        outputs: Vec::new(),
        chat_id: chat.to_string(),
        agent_id: agent_id.to_string(),
        text: text.to_string(),
        steps: Vec::new(),
        // Desk-visible: these fixtures exercise the desk fold, not asides.
        audience: Vec::new(),
    }
}

pub(super) fn message(by: Option<Actor>, chat: Option<&str>, text: &str) -> CompanyEvent {
    CompanyEvent::OperatorMessage {
        mentions: Vec::new(),
        parent: None,
        text: text.to_string(),
        by,
        chat: chat.map(str::to_string),
        deliverable: None,
        attachments: Vec::new(),
    }
}

/// An event that is emphatically not a line anybody said.
pub(super) fn noise(seq: u64) -> StoredEvent {
    stored(
        seq,
        CompanyEvent::WorkflowRunStarted {
            workflow_id: "wf".into(),
            run_id: format!("run-{seq}"),
            scheduled: false,
            started_by: None,
            resume_semantic: None,
        },
    )
}

pub(super) async fn read(journal: &Journal, before: Option<u64>, limit: usize) -> SessionPage {
    let record = record();
    let people = HashMap::new();
    let company = company();
    let log = JournalSessionLog::new(journal, &company, &record, &people);
    log.read_before(before.map(Sequence), limit)
        .await
        .expect("the journal reads")
}

pub(super) fn software_company() -> CompanyRecord {
    let src = "[company]\nname = \"Acme\"\n\n[policy]\nmode = \"full\"\n\
               \n[[agent]]\nid = \"software_engineer\"\nrole = \"Engineer\"\n\
               \n[[agent]]\nid = \"qa_engineer\"\nrole = \"QA\"\n\
               \n[[agent]]\nid = \"product_designer\"\nrole = \"Designer\"\n\
               \n[[group_chat]]\nid = \"engineering\"\nname = \"Engineering\"\n\
               members = [\"software_engineer\", \"qa_engineer\"]\n\
               \n[[group_chat]]\nid = \"product_design\"\nname = \"Product & Design\"\n\
               members = [\"product_designer\"]\n";
    let manifest: crate::company::CompanyManifest = toml::from_str(src).expect("manifest parses");
    CompanyRecord {
        manifest,
        ..record()
    }
}

pub(super) fn referral_input(
    content: &str,
    mentions: Vec<tinyhivemind_core::mention::Mention>,
) -> tinyhivemind_core::referral::ReferralInput {
    tinyhivemind_core::referral::ReferralInput {
        key: tinyhivemind_core::dispatch::DispatchKey {
            trigger_sequence: 42,
        },
        conversation: tinyhivemind_core::dispatch::DispatchConversation {
            desk_id: "engineering".to_string(),
            thread_root: None,
        },
        author_id: "software_engineer".to_string(),
        content: content.to_string(),
        mentions,
        hop: 0,
        origin: None,
    }
}
